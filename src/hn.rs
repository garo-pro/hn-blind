//! Hacker News Firebase API client.
//!
//! The public API is unauthenticated but chatty: every story and comment is a separate request. Everything here is blocking and meant to run on a worker thread; batches are fetched with a small pool of scoped threads so a list of 30 stories costs one round trip's latency rather than 30.

use serde::Deserialize;
use std::time::Duration;

use crate::templates::{Template, Templates};

const API: &str = "https://hacker-news.firebaseio.com/v0";

/// How many requests we allow in flight at once. HN tolerates this comfortably and it keeps a cold comment thread under a couple of seconds.
const PARALLELISM: usize = 8;

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
        let mut indexed: Vec<(usize, Item)> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..workers)
                .map(|w| {
                    // Stride assignment keeps each worker's share even without needing a shared work queue.
                    scope.spawn(move || {
                        let mut out = Vec::new();
                        for (i, id) in ids.iter().enumerate().skip(w).step_by(workers) {
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
    /// Fetching is done one level at a time so each level's requests run in parallel, then the tree is walked depth-first to produce the order a reader actually wants. `max` bounds the work for threads with thousands of comments.
    pub fn comment_thread(&self, root_kids: &[u64], max: usize) -> Vec<CommentRow> {
        use std::collections::HashMap;

        let mut by_id: HashMap<u64, Item> = HashMap::new();
        let mut level: Vec<u64> = root_kids.to_vec();

        while !level.is_empty() && by_id.len() < max {
            let budget = max - by_id.len();
            if level.len() > budget {
                level.truncate(budget);
            }

            let items = self.items(&level);
            let mut next = Vec::new();
            for item in items {
                if item.is_readable() {
                    next.extend_from_slice(&item.kids);
                }
                by_id.insert(item.id, item);
            }
            level = next;
        }

        // Walk depth-first with an explicit stack so deep threads cannot blow the real stack.
        let mut rows = Vec::new();
        let mut stack: Vec<(u64, usize)> = root_kids.iter().rev().map(|id| (*id, 0usize)).collect();

        while let Some((id, depth)) = stack.pop() {
            let Some(item) = by_id.get(&id) else { continue };
            if !item.is_readable() {
                continue;
            }
            for kid in item.kids.iter().rev() {
                stack.push((*kid, depth + 1));
            }
            rows.push(CommentRow {
                item: item.clone(),
                depth,
            });
        }

        rows
    }
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
