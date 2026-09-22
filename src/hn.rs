//! Hacker News Firebase API client.
//!
//! The public API is unauthenticated but chatty: every story and comment is a separate request. Everything here is blocking and meant to run on a worker thread; batches are fetched with a pool of scoped threads so a list of 30 stories costs a couple of round trips' latency rather than 30.

use serde::Deserialize;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use crate::templates::{Template, Templates};

const API: &str = "https://hacker-news.firebaseio.com/v0";

/// How many requests we allow in flight at once. The Firebase API documents no rate limit and HN clients commonly run this many or more; a comment thread is hundreds of tiny requests, so latency is almost entirely a matter of how many are waiting at once.
const PARALLELISM: usize = 24;

/// The story lists HN exposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feed {
    Top,
    New,
    Best,
    Ask,
    Show,
    Job,
}

impl Feed {
    pub const ALL: [Feed; 6] = [
        Feed::Top,
        Feed::New,
        Feed::Best,
        Feed::Ask,
        Feed::Show,
        Feed::Job,
    ];

    fn endpoint(self) -> &'static str {
        match self {
            Feed::Top => "topstories",
            Feed::New => "newstories",
            Feed::Best => "beststories",
            Feed::Ask => "askstories",
            Feed::Show => "showstories",
            Feed::Job => "jobstories",
        }
    }

    /// Human-readable name, used in announcements and the accessibility tree.
    pub fn title(self) -> &'static str {
        match self {
            Feed::Top => "Top stories",
            Feed::New => "New stories",
            Feed::Best => "Best stories",
            Feed::Ask => "Ask HN",
            Feed::Show => "Show HN",
            Feed::Job => "Jobs",
        }
    }
}

/// A single HN item. Every field beyond `id` is optional because the API omits them freely: deleted comments have no author, job posts have no score, and self-posts have no URL.
#[derive(Debug, Clone, Deserialize)]
pub struct Item {
    pub id: u64,
    #[serde(default)]
    pub by: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub score: Option<i64>,
    #[serde(default)]
    pub descendants: Option<i64>,
    #[serde(default)]
    pub kids: Vec<u64>,
    #[serde(default)]
    pub time: Option<i64>,
    #[serde(default)]
    pub deleted: bool,
    #[serde(default)]
    pub dead: bool,
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
}

impl Item {
    /// The HN discussion page for this item, which is the only link that always exists (self-posts have no external URL).
    pub fn hn_url(&self) -> String {
        format!("https://news.ycombinator.com/item?id={}", self.id)
    }

    /// Deleted and flagged-dead items still appear in `kids` lists; we keep them out of the reading order rather than announcing empty rows.
    pub fn is_readable(&self) -> bool {
        !self.deleted && !self.dead
    }
}

/// A comment plus its indentation depth within the thread.
#[derive(Debug, Clone)]
pub struct CommentRow {
    pub item: Item,
    pub depth: usize,
}

pub struct Client {
    agent: ureq::Agent,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    pub fn new() -> Self {
        let config = ureq::Agent::config_builder()
            .user_agent("hn-blind (accessible Hacker News client)")
            .timeout_global(Some(Duration::from_secs(20)))
            // Every request goes to the same host, and ureq's default keeps only three idle connections per host. With more workers than that, each surplus worker's connection was closed after every request and the next one paid a fresh TLS handshake — for a comment thread, hundreds of them.
            .max_idle_connections_per_host(PARALLELISM)
            .max_idle_connections(PARALLELISM)
            .build();
        Client {
            agent: ureq::Agent::new_with_config(config),
        }
    }

    fn get_json<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T, String> {
        self.agent
            .get(url)
            .call()
            .map_err(|e| format!("request failed: {e}"))?
            .body_mut()
            .read_json::<T>()
            .map_err(|e| format!("bad response: {e}"))
    }

    /// Fetch the first `limit` ids of a feed.
    pub fn story_ids(&self, feed: Feed, limit: usize) -> Result<Vec<u64>, String> {
        let url = format!("{API}/{}.json", feed.endpoint());
        let mut ids: Vec<u64> = self.get_json(&url)?;
        ids.truncate(limit);
        Ok(ids)
    }

    /// Fetch one item. HN returns literal `null` for ids that no longer exist, which deserializes to `None` rather than erroring.
    pub fn item(&self, id: u64) -> Result<Option<Item>, String> {
        self.get_json(&format!("{API}/item/{id}.json"))
    }

    /// Fetch many items concurrently, preserving the order of `ids`.
    ///
    /// Individual failures are dropped rather than failing the whole batch: one dead story should not cost the reader the other twenty-nine.
    pub fn items(&self, ids: &[u64]) -> Vec<Item> {
        if ids.is_empty() {
            return Vec::new();
        }

        let workers = PARALLELISM.min(ids.len());
        // One shared cursor rather than a fixed share per worker: with fixed shares, a worker held up by one slow response sits on the rest of its share while the others run out of work, and the batch takes as long as its unluckiest worker.
        let next = AtomicUsize::new(0);
        let mut indexed: Vec<(usize, Item)> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..workers)
                .map(|_| {
                    scope.spawn(|| {
                        let mut out = Vec::new();
                        loop {
                            let i = next.fetch_add(1, Ordering::Relaxed);
                            let Some(id) = ids.get(i) else { break };
                            if let Ok(Some(item)) = self.item(*id) {
                                out.push((i, item));
                            }
                        }
                        out
                    })
                })
                .collect();

            handles
                .into_iter()
                .filter_map(|h| h.join().ok())
                .flatten()
                .collect()
        });

        indexed.sort_by_key(|(i, _)| *i);
        indexed.into_iter().map(|(_, item)| item).collect()
    }

    /// Load a story's comment thread, flattened into reading (depth-first) order.
    ///
    /// See [`fetch_thread`] for how the work is scheduled. `max` bounds the work for threads with thousands of comments.
    pub fn comment_thread(&self, root_kids: &[u64], max: usize) -> Vec<CommentRow> {
        fetch_thread(root_kids, max, PARALLELISM, |id| self.item(id).ok().flatten())
    }
}

/// Where a comment falls in reading order: its index among its parent's replies, for each ancestor down from the story. Compared lexicographically these are exactly depth-first order, so a heap of them always hands out whatever the reader will reach soonest.
type Place = Vec<u32>;

/// The work shared between [`fetch_thread`]'s workers, all behind one lock so that "nothing left to claim" and "nothing still in flight" are always judged together.
struct Queue {
    pending: BinaryHeap<Reverse<(Place, u64)>>,
    fetched: HashMap<u64, Item>,
    claimed: usize,
    in_flight: usize,
}

/// Fetch up to `max` comments of a thread with `workers` requests in flight, and flatten them into reading order.
///
/// There are no levels to wait on: a reply is queued the moment its parent arrives, so one slow response delays only its own subtree instead of holding every worker at the end of a level. The queue is ordered by [`Place`], which spends the `max` budget on the comments the reader reaches first. Separate from [`Client`] so the scheduling can be tested without a network.
fn fetch_thread<F>(root_kids: &[u64], max: usize, workers: usize, fetch: F) -> Vec<CommentRow>
where
    F: Fn(u64) -> Option<Item> + Sync,
{
    let queue = Mutex::new(Queue {
        pending: root_kids
            .iter()
            .enumerate()
            .map(|(i, id)| Reverse((vec![i as u32], *id)))
            .collect(),
        fetched: HashMap::new(),
        claimed: 0,
        in_flight: 0,
    });
    // Signalled whenever a fetch finishes, which is the only thing that can give an idle worker something to do — or tell it there never will be.
    let settled = Condvar::new();

    std::thread::scope(|scope| {
        for _ in 0..workers.min(max) {
            scope.spawn(|| {
                loop {
                    let (place, id) = {
                        let mut q = queue.lock().unwrap();
                        loop {
                            if q.claimed >= max {
                                return;
                            }
                            if let Some(Reverse(next)) = q.pending.pop() {
                                q.claimed += 1;
                                q.in_flight += 1;
                                break next;
                            }
                            // An empty queue is only final once nothing is in flight: a reply still on its way may bring a whole subthread with it.
                            if q.in_flight == 0 {
                                return;
                            }
                            q = settled.wait(q).unwrap();
                        }
                    };

                    // A panicking fetch is treated as a failed one. Otherwise its `in_flight` would never be given back, every other worker would wait on it forever, and the thread would never finish loading.
                    let item = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fetch(id)))
                        .ok()
                        .flatten();

                    let mut q = queue.lock().unwrap();
                    q.in_flight -= 1;
                    if let Some(item) = item {
                        if item.is_readable() {
                            for (i, kid) in item.kids.iter().enumerate() {
                                let mut child = place.clone();
                                child.push(i as u32);
                                q.pending.push(Reverse((child, *kid)));
                            }
                        }
                        q.fetched.insert(item.id, item);
                    }
                    drop(q);
                    settled.notify_all();
                }
            });
        }
    });

    let Queue { pending, mut fetched, .. } = queue.into_inner().unwrap();

    // The budget is spent in reading order, but workers run ahead of one another, so when it ran out some later comments had already arrived while earlier ones were never asked for. Stopping at the first comment that was never asked for makes what the reader gets an unbroken beginning of the thread — one that simply ends early — instead of one with replies silently missing from its middle. A comment whose fetch *failed* is not a stopping point; it is skipped, as it always was, so one bad request cannot cost the rest of the thread.
    let unclaimed: HashSet<u64> = pending.into_iter().map(|Reverse((_, id))| id).collect();

    // Walk depth-first with an explicit stack so deep threads cannot blow the real stack.
    let mut rows = Vec::new();
    let mut stack: Vec<(u64, usize)> = root_kids.iter().rev().map(|id| (*id, 0usize)).collect();

    // Take each item out of the map rather than cloning it. Every id is reachable once, so the row can simply own the strings the map was holding; cloning instead would mean the whole thread existed twice at the moment it is largest, and comment bodies are the bulk of what this application keeps in memory. Taking also makes a repeated id skip on its second visit rather than duplicating a subtree.
    while let Some((id, depth)) = stack.pop() {
        if unclaimed.contains(&id) {
            break;
        }
        let Some(item) = fetched.remove(&id) else { continue };
        if !item.is_readable() {
            continue;
        }
        for kid in item.kids.iter().rev() {
            stack.push((*kid, depth + 1));
        }
        rows.push(CommentRow { item, depth });
    }

    rows
}

/// Format a Unix timestamp as an approximate age, e.g. "3 hours ago".
///
/// Singular and plural are separate templates rather than an appended "s", because that trick only works in English and this wording is the user's to change.
pub fn relative_time(unix: Option<i64>, templates: &Templates) -> String {
    let Some(unix) = unix else {
        return templates.render(Template::TimeUnknown, &[]);
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(unix);
    let secs = (now - unix).max(0);

    const MINUTE: i64 = 60;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;

    let (n, one, many) = match secs {
        s if s < MINUTE => return templates.render(Template::TimeNow, &[]),
        s if s < HOUR => (s / MINUTE, Template::TimeMinute, Template::TimeMinutes),
        s if s < DAY => (s / HOUR, Template::TimeHour, Template::TimeHours),
        s => (s / DAY, Template::TimeDay, Template::TimeDays),
    };
    let template = if n == 1 { one } else { many };
    templates.render(template, &[("count", &n.to_string())])
}

/// The bare domain of a URL, which is the part a reader wants announced alongside a headline.
pub fn domain_of(url: &str) -> Option<String> {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let host = rest.split('/').next()?;
    let host = host.split('@').next_back()?;
    let host = host.split(':').next()?;
    let host = host.strip_prefix("www.").unwrap_or(host);
    (!host.is_empty()).then(|| host.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_strips_scheme_www_port_and_path() {
        assert_eq!(
            domain_of("https://www.example.com/a/b").as_deref(),
            Some("example.com")
        );
        assert_eq!(
            domain_of("http://example.com:8080/x").as_deref(),
            Some("example.com")
        );
        assert_eq!(domain_of("example.org").as_deref(), Some("example.org"));
        assert_eq!(domain_of(""), None);
    }

    #[test]
    fn relative_time_pluralizes_and_handles_missing() {
        let t = Templates::default();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert_eq!(relative_time(None, &t), "unknown time");
        assert_eq!(relative_time(Some(now), &t), "just now");
        assert_eq!(relative_time(Some(now - 3600), &t), "1 hour ago");
        assert_eq!(relative_time(Some(now - 7200), &t), "2 hours ago");
        assert_eq!(relative_time(Some(now - 86_400 * 3), &t), "3 days ago");
        // Clock skew must not produce a negative age.
        assert_eq!(relative_time(Some(now + 500), &t), "just now");
    }

    fn comment(id: u64, kids: &[u64]) -> Item {
        Item {
            id,
            by: Some("alice".into()),
            title: None,
            url: None,
            text: Some(format!("comment {id}")),
            score: None,
            descendants: None,
            kids: kids.to_vec(),
            time: None,
            deleted: false,
            dead: false,
            kind: Some("comment".into()),
        }
    }

    /// A story with replies 1 and 2; 1 has replies 3 and 4, 3 has reply 5, and 2 has reply 6. Returns the story's own kids and a fake network that answers from the tree.
    fn thread() -> (Vec<u64>, HashMap<u64, Item>) {
        let items = [
            comment(1, &[3, 4]),
            comment(2, &[6]),
            comment(3, &[5]),
            comment(4, &[]),
            comment(5, &[]),
            comment(6, &[]),
        ];
        (vec![1, 2], items.into_iter().map(|item| (item.id, item)).collect())
    }

    fn ids_and_depths(rows: &[CommentRow]) -> Vec<(u64, usize)> {
        rows.iter().map(|row| (row.item.id, row.depth)).collect()
    }

    const READING_ORDER: [(u64, usize); 6] = [(1, 0), (3, 1), (5, 2), (4, 1), (2, 0), (6, 1)];

    #[test]
    fn a_thread_comes_back_in_reading_order_however_many_workers_fetch_it() {
        let (kids, tree) = thread();
        for workers in [1, 2, 8] {
            let rows = fetch_thread(&kids, 100, workers, |id| tree.get(&id).cloned());
            assert_eq!(ids_and_depths(&rows), READING_ORDER, "{workers} workers");
        }
    }

    #[test]
    fn the_limit_cuts_the_end_of_a_thread_not_its_middle() {
        let (kids, tree) = thread();
        // One worker makes the order of claims deterministic: exactly the first three in reading order, not the whole top level.
        let rows = fetch_thread(&kids, 3, 1, |id| tree.get(&id).cloned());
        assert_eq!(ids_and_depths(&rows), &READING_ORDER[..3]);

        // Several workers run ahead of each other, but whatever they fetched, the reader still gets an unbroken beginning.
        for max in 1..=6 {
            let rows = fetch_thread(&kids, max, 4, |id| tree.get(&id).cloned());
            let got = ids_and_depths(&rows);
            assert!(got.len() <= max);
            assert_eq!(got, &READING_ORDER[..got.len()], "limit {max}");
        }
    }

    #[test]
    fn a_failed_comment_costs_its_own_replies_and_nothing_else() {
        let (kids, tree) = thread();
        let rows = fetch_thread(&kids, 100, 4, |id| if id == 3 { None } else { tree.get(&id).cloned() });
        assert_eq!(ids_and_depths(&rows), [(1, 0), (4, 1), (2, 0), (6, 1)]);
    }

    #[test]
    fn a_panicking_fetch_is_a_failure_not_a_hang() {
        let (kids, tree) = thread();
        let rows = fetch_thread(&kids, 100, 4, |id| {
            assert_ne!(id, 2, "simulated crash in the request");
            tree.get(&id).cloned()
        });
        assert_eq!(ids_and_depths(&rows), &READING_ORDER[..4]);
    }

    #[test]
    fn dead_comments_are_neither_read_nor_followed() {
        let (kids, mut tree) = thread();
        tree.get_mut(&1).unwrap().dead = true;
        let asked = Mutex::new(Vec::new());
        let rows = fetch_thread(&kids, 100, 2, |id| {
            asked.lock().unwrap().push(id);
            tree.get(&id).cloned()
        });
        assert_eq!(ids_and_depths(&rows), [(2, 0), (6, 1)]);
        assert!(!asked.into_inner().unwrap().contains(&3), "a dead comment's replies are never fetched");
    }

    #[test]
    fn the_wording_of_an_age_is_the_users_to_change() {
        let mut t = Templates::default();
        t.set(Template::TimeHours, "{count}h");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert_eq!(relative_time(Some(now - 7200), &t), "2h");
    }
}
