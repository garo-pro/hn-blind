//! Application state and the wording it produces.
//!
//! There is no accessibility tree to build any more: `main.rs` owns real wxWidgets controls (a `ListCtrl`, a `TreeCtrl`, a native `MenuBar`, a settings `Dialog`) and the platform's own MSAA/UIA support is what a screen reader reads. This module's job shrinks to exactly what it should always have been about: deciding *what* the application is showing and *what to say* about it.
//!
//! No wording is built by concatenation here either. Every string that reaches the user's ears comes from `templates.rs`, so this module's job is to decide *which* template applies and to hand it the values it needs.

use std::collections::HashSet;

use crate::hn::{CommentRow, Feed, Item, domain_of, relative_time};
use crate::html;
use crate::menu::{Command, Kind};
use crate::preferences::Preferences;
use crate::settings::{Field, Settings, TABS};
use crate::templates::{Template, Templates};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Stories,
    Comments,
    Help,
    Settings,
}

/// The keyboard map, also rendered as the help view so it is discoverable without sight or documentation.
pub const HELP: &[(&str, &str)] = &[
    ("Up / Down, J / K", "move through the list"),
    ("Home / End", "first or last item"),
    ("Page Up / Page Down", "move ten items"),
    (
        "Left / Right",
        "in the comments, hide or show the replies to a comment, or move to its parent or first reply",
    ),
    ("Enter", "open the comments for the selected story"),
    ("O", "open the story link in your browser"),
    ("C", "open the Hacker News discussion page in your browser"),
    ("Backspace or Escape", "go back to the story list"),
    ("R", "reload the current list"),
    ("1 to 6", "switch feed: top, new, best, ask, show, jobs"),
    ("P", "read the selected item in full"),
    ("S", "stop speaking"),
    ("V", "turn Prism speech on or off"),
    ("Alt", "open the menu bar, a second way to reach every command"),
    (
        "Comma",
        "settings, where the wording of every announcement and other preferences are set",
    ),
    ("H or F1", "this help"),
    ("Q", "quit"),
];

/// Arguments for a template, owned because most of them are freshly formatted.
type Args = Vec<(&'static str, String)>;

/// Borrow an argument list in the shape `Templates::render` takes.
fn borrow(args: &Args) -> Vec<(&str, &str)> {
    args.iter().map(|(k, v)| (*k, v.as_str())).collect()
}

pub struct App {
    pub feed: Feed,
    pub stories: Vec<Item>,
    pub story_cursor: usize,
    pub comments: Vec<CommentRow>,
    pub comment_cursor: usize,
    /// Comment rows whose replies are hidden, as indices into `comments`.
    ///
    /// A tree whose subthreads cannot actually be skipped is only an indented list, so this is the thing that makes the comments view a tree rather than a flat one: Left and Right move rows in and out of it, and every other movement key steps over what it hides.
    pub comment_collapsed: HashSet<usize>,
    /// The story whose thread is loaded, so the comments view can title itself.
    pub comment_story: Option<Item>,
    pub view: View,
    /// The view to return to when leaving help or settings.
    pub previous_view: View,
    pub status: String,
    pub loading: bool,
    /// The wording of everything this application says.
    pub templates: Templates,
    pub settings: Settings,
    /// Yes/no switches, such as whether Escape quits from the story list.
    pub preferences: Preferences,
}

impl App {
    pub fn new(feed: Feed, templates: Templates, preferences: Preferences) -> Self {
        App {
            feed,
            stories: Vec::new(),
            story_cursor: 0,
            comments: Vec::new(),
            comment_cursor: 0,
            comment_collapsed: HashSet::new(),
            comment_story: None,
            view: View::Stories,
            previous_view: View::Stories,
            status: templates.render(Template::StatusStarting, &[]),
            loading: false,
            templates,
            settings: Settings::new(),
            preferences,
        }
    }

    /// Render a template. A thin wrapper, but it keeps call sites reading as "say this" rather than as string plumbing.
    pub fn text(&self, template: Template, args: &[(&str, &str)]) -> String {
        self.templates.render(template, args)
    }

    pub fn feed_title(&self) -> String {
        self.templates.feed_title(self.feed)
    }

    /// How many rows the user can currently move between.
    ///
    /// For a collapsed comment thread this is fewer than the number of comments loaded: hidden replies are not somewhere the cursor can go, so counting them would announce a position the user cannot reach. The unfiltered total is still what titles the view.
    pub fn row_count(&self) -> usize {
        match self.view {
            View::Stories => self.stories.len(),
            View::Comments => self.visible_comments().count(),
            View::Help => HELP.len(),
            View::Settings => 0,
        }
    }

    /// The row the cursor is on, as an index into the view's own list. For comments this indexes the whole thread, collapsed replies included, because that is what `comments` and the tree's item handles are keyed by; [`App::position`] is the one to announce.
    pub fn cursor(&self) -> usize {
        match self.view {
            View::Stories => self.story_cursor,
            View::Comments => self.comment_cursor,
            View::Help | View::Settings => 0,
        }
    }

    /// Where the cursor sits among the rows the user can reach — the "3" in "3 of 46". Only a collapsed comment thread makes this differ from [`App::cursor`].
    pub fn position(&self) -> usize {
        match self.view {
            View::Comments => self
                .visible_comments()
                .position(|index| index == self.comment_cursor)
                .unwrap_or(0),
            _ => self.cursor(),
        }
    }

    // ---- The comment tree --------------------------------------------------

    /// The comment rows the tree is showing, in reading order.
    ///
    /// The thread arrives flattened with a depth on each row, so everything below a collapsed row until the depth comes back up is what that row hides. Nested collapses need no special case: the outermost one is already skipping their rows.
    pub fn visible_comments(&self) -> impl Iterator<Item = usize> + '_ {
        let mut hiding_below: Option<usize> = None;
        self.comments.iter().enumerate().filter_map(move |(index, row)| {
            if let Some(depth) = hiding_below {
                if row.depth > depth {
                    return None;
                }
                hiding_below = None;
            }
            if self.comment_collapsed.contains(&index) {
                hiding_below = Some(row.depth);
            }
            Some(index)
        })
    }

    /// Does this comment have replies?
    ///
    /// The thread is flat and in reading order, so it does exactly when the row after it sits one level deeper.
    pub fn comment_has_replies(&self, index: usize) -> bool {
        match (self.comments.get(index), self.comments.get(index + 1)) {
            (Some(row), Some(next)) => next.depth > row.depth,
            _ => false,
        }
    }

    /// How many rows are nested under this one, at any depth.
    pub fn comment_reply_count(&self, index: usize) -> usize {
        let Some(row) = self.comments.get(index) else {
            return 0;
        };
        self.comments[index + 1..]
            .iter()
            .take_while(|reply| reply.depth > row.depth)
            .count()
    }

    pub fn comment_is_collapsed(&self, index: usize) -> bool {
        self.comment_collapsed.contains(&index)
    }

    /// The comment this one replies to: the nearest row above it that sits one level shallower.
    pub fn comment_parent(&self, index: usize) -> Option<usize> {
        let depth = self.comments.get(index)?.depth;
        if depth == 0 {
            return None;
        }
        self.comments[..index].iter().rposition(|row| row.depth < depth)
    }

    /// Hide or show one comment's replies. Returns whether anything changed, so a caller can stay silent when the key did nothing.
    pub fn set_comment_collapsed(&mut self, index: usize, collapsed: bool) -> bool {
        if !self.comment_has_replies(index) {
            return false;
        }
        let changed = if collapsed {
            self.comment_collapsed.insert(index)
        } else {
            self.comment_collapsed.remove(&index)
        };

        // Closing a thread the cursor was inside would otherwise leave it on a row that is no longer there — which the tree reports as no selection at all, and which movement would then have to guess its way out of. The comment that was closed is where it belongs.
        if changed && collapsed && !self.visible_comments().any(|row| row == self.comment_cursor) {
            self.comment_cursor = index;
        }
        changed
    }

    /// Forget which threads were collapsed. Called when the thread itself changes, since the indices are only meaningful against one `comments`.
    pub fn clear_comment_collapsed(&mut self) {
        self.comment_collapsed.clear();
    }

    fn set_cursor(&mut self, index: usize) {
        match self.view {
            View::Stories => self.story_cursor = index,
            View::Comments => self.comment_cursor = index,
            // Neither has a cursor of its own: help is read as a whole, and the settings dialog moves focus through its own tree/text controls.
            View::Help | View::Settings => {}
        }
    }

    /// Move the selection by `delta`, clamping at both ends.
    ///
    /// Clamping rather than wrapping is deliberate: hitting a hard stop tells a listener they are at the edge of the list without needing an extra announcement.
    ///
    /// `delta` counts rows the user can see, so in a comment thread it steps over collapsed replies rather than through them. Landing on a hidden row would be worse than useless: the tree would scroll it back into view by reopening the thread the user just closed.
    pub fn move_cursor(&mut self, delta: isize) -> bool {
        if self.view == View::Settings {
            return false;
        }
        if self.view == View::Comments {
            let rows: Vec<usize> = self.visible_comments().collect();
            if rows.is_empty() {
                return false;
            }
            let at = self.position() as isize;
            let next = rows[(at + delta).clamp(0, rows.len() as isize - 1) as usize];
            if next == self.comment_cursor {
                return false;
            }
            self.comment_cursor = next;
            return true;
        }

        let count = self.row_count();
        if count == 0 {
            return false;
        }
        let current = self.cursor() as isize;
        let next = (current + delta).clamp(0, count as isize - 1) as usize;
        if next == self.cursor() {
            return false;
        }
        self.set_cursor(next);
        true
    }

    /// Move to one particular row, by its index into the view's own list — which is what a click on a list row or a tree item reports. A comment hidden inside a collapsed thread is not somewhere to move to.
    pub fn move_to(&mut self, index: usize) -> bool {
        if self.view == View::Settings || index == self.cursor() {
            return false;
        }
        if self.view == View::Comments {
            if !self.visible_comments().any(|visible| visible == index) {
                return false;
            }
            self.comment_cursor = index;
            return true;
        }
        if index >= self.row_count() {
            return false;
        }
        self.set_cursor(index);
        true
    }

    /// Move to the first or last row the user can reach — Home and End. Stated this way rather than as an index because in a collapsed thread the last reachable row is not the last comment.
    pub fn move_to_edge(&mut self, last: bool) -> bool {
        let row = if self.view == View::Comments {
            // Walked forwards either way: the iterator carries the state of which collapsed thread it is inside, so it only reads correctly from the front.
            let mut rows = self.visible_comments();
            if last { rows.last() } else { rows.next() }
        } else {
            match self.row_count() {
                0 => None,
                count if last => Some(count - 1),
                _ => Some(0),
            }
        };
        match row {
            Some(row) => self.move_to(row),
            None => false,
        }
    }

    pub fn selected_story(&self) -> Option<&Item> {
        match self.view {
            View::Stories => self.stories.get(self.story_cursor),
            View::Comments => self.comment_story.as_ref(),
            View::Help | View::Settings => None,
        }
    }

    /// The item the user is pointed at, whichever list they are in.
    pub fn selected_item(&self) -> Option<&Item> {
        match self.view {
            View::Stories => self.stories.get(self.story_cursor),
            View::Comments => self.comments.get(self.comment_cursor).map(|r| &r.item),
            View::Help | View::Settings => None,
        }
    }

    /// Title for the container, which is what a screen reader announces on entering it.
    pub fn list_title(&self) -> String {
        match self.view {
            View::Stories => self.text(
                Template::StoriesTitle,
                &[
                    ("feed", &self.feed_title()),
                    ("count", &self.stories.len().to_string()),
                ],
            ),
            View::Comments => {
                let story = self.comment_story.as_ref();
                self.text(
                    Template::CommentsTitle,
                    &[
                        ("count", &self.comments.len().to_string()),
                        ("title", &self.story_title(story)),
                        (
                            "author",
                            story.and_then(|s| s.by.as_deref()).unwrap_or_default(),
                        ),
                        (
                            "score",
                            &story
                                .and_then(|s| s.score)
                                .map(|s| s.to_string())
                                .unwrap_or_default(),
                        ),
                    ],
                )
            }
            View::Help => self.text(Template::HelpTitle, &[("count", &HELP.len().to_string())]),
            View::Settings => self.text(
                Template::SettingsTitle,
                &[
                    ("tab", self.settings.tab_name()),
                    ("index", &(self.settings.tab() + 1).to_string()),
                    ("count", &TABS.len().to_string()),
                ],
            ),
        }
    }

    /// The one-line label for a row of the story, comment or help list.
    pub fn row_label(&self, index: usize) -> String {
        match self.view {
            View::Stories => self
                .stories
                .get(index)
                .map(|story| {
                    let args = self.story_args(index, story);
                    self.text(Template::StoryRow, &borrow(&args))
                })
                .unwrap_or_default(),
            View::Comments => self
                .comments
                .get(index)
                .map(|row| {
                    let args = self.comment_args(index, row);
                    self.text(Template::CommentRow, &borrow(&args))
                })
                .unwrap_or_default(),
            View::Help => HELP
                .get(index)
                .map(|(keys, what)| {
                    self.text(
                        Template::HelpRow,
                        &[
                            ("index", &(index + 1).to_string()),
                            ("keys", keys),
                            ("what", what),
                            ("count", &HELP.len().to_string()),
                        ],
                    )
                })
                .unwrap_or_default(),
            View::Settings => String::new(),
        }
    }

    /// What a listener hears when a comment's replies are hidden or shown.
    ///
    /// Only spoken when nothing else is speaking: a screen reader reads the expanded/collapsed state of a tree item itself, and saying it twice is worse than not saying it at all.
    pub fn collapse_text(&self, index: usize, collapsed: bool) -> String {
        let template = if collapsed {
            Template::CommentCollapsed
        } else {
            Template::CommentExpanded
        };
        let row = self.comments.get(index);
        self.text(
            template,
            &[
                ("replies", &self.comment_reply_count(index).to_string()),
                ("level", &row.map(|row| row.depth + 1).unwrap_or(1).to_string()),
                (
                    "author",
                    row.and_then(|row| row.item.by.as_deref()).unwrap_or_default(),
                ),
                ("label", &self.row_label(index)),
            ],
        )
    }

    /// The full text of the selected row, spoken on demand. Unlike the label this is not truncated and keeps paragraph breaks.
    pub fn selected_detail(&self) -> String {
        match self.view {
            View::Stories => match self.stories.get(self.story_cursor) {
                Some(story) => {
                    let args = self.story_args(self.story_cursor, story);
                    self.text(Template::StoryDetail, &borrow(&args))
                }
                None => self.text(Template::StatusNothingSelected, &[]),
            },
            View::Comments => match self.comments.get(self.comment_cursor) {
                Some(row) => {
                    let args = self.comment_args(self.comment_cursor, row);
                    self.text(Template::CommentDetail, &borrow(&args))
                }
                None => self.text(Template::StatusNothingSelected, &[]),
            },
            View::Help => {
                let rows: Vec<String> = (0..HELP.len()).map(|i| self.row_label(i)).collect();
                self.text(
                    Template::HelpDetail,
                    &[
                        ("rows", &rows.join(". ")),
                        ("count", &HELP.len().to_string()),
                    ],
                )
            }
            View::Settings => match self.settings.focused_field() {
                Some(field) => self.field_value_and_placeholders(field).0,
                None => self.text(Template::StatusNothingSelected, &[]),
            },
        }
    }

    // ---- Template arguments ----------------------------------------------

    fn story_title(&self, story: Option<&Item>) -> String {
        match story.and_then(|s| s.title.as_deref()) {
            Some(title) => title.to_string(),
            None => self.text(Template::WordUntitled, &[]),
        }
    }

    /// Everything a story template may ask for. Shared between the row label and the read-in-full text so the two cannot drift apart.
    fn story_args(&self, index: usize, story: &Item) -> Args {
        let kind = match story.kind.as_deref() {
            Some("job") => self.text(Template::WordJob, &[]),
            Some("poll") => self.text(Template::WordPoll, &[]),
            _ => String::new(),
        };

        vec![
            ("index", (index + 1).to_string()),
            ("title", self.story_title(Some(story))),
            (
                "domain",
                story.url.as_deref().and_then(domain_of).unwrap_or_default(),
            ),
            ("url", story.url.clone().unwrap_or_default()),
            (
                "score",
                story.score.map(|s| s.to_string()).unwrap_or_default(),
            ),
            ("author", story.by.clone().unwrap_or_default()),
            ("age", relative_time(story.time, &self.templates)),
            (
                "comments",
                self.comment_count_text(story.descendants.unwrap_or(0)),
            ),
            ("count", self.stories.len().to_string()),
            ("kind", kind),
            (
                "body",
                story.text.as_deref().map(html::to_text).unwrap_or_default(),
            ),
        ]
    }

    fn comment_args(&self, index: usize, row: &CommentRow) -> Args {
        let body = html::to_text(row.item.text.as_deref().unwrap_or(""));
        let body = if body.is_empty() {
            self.text(Template::WordNoText, &[])
        } else {
            body
        };

        vec![
            ("index", (index + 1).to_string()),
            // Depth is 0-based; what the user hears is 1-based.
            ("level", (row.depth + 1).to_string()),
            (
                "author",
                match row.item.by.as_deref() {
                    Some(author) => author.to_string(),
                    None => self.text(Template::WordUnknownAuthor, &[]),
                },
            ),
            ("age", relative_time(row.item.time, &self.templates)),
            ("body", html::single_line(&body)),
            ("count", self.comments.len().to_string()),
        ]
    }

    /// The comment count is three templates rather than one, because a language that pluralizes differently cannot be served by appending an "s".
    fn comment_count_text(&self, count: i64) -> String {
        let template = match count {
            0 => Template::CommentsNone,
            1 => Template::CommentsOne,
            _ => Template::CommentsMany,
        };
        self.text(template, &[("count", &count.to_string())])
    }

    // ---- Settings dialog --------------------------------------------------

    pub fn field_focus_text(&self, index: usize) -> String {
        let Some(field) = self.settings.fields().get(index).copied() else {
            return String::new();
        };
        let (value, placeholders) = self.field_value_and_placeholders(field);

        self.text(
            Template::SettingsFieldFocus,
            &[
                ("name", field.label()),
                ("group", field.group().name()),
                ("value", &value),
                ("placeholders", &placeholders),
                ("index", &(index + 1).to_string()),
                ("count", &self.settings.fields().len().to_string()),
            ],
        )
    }

    /// A field's current value as spoken text, and its placeholder list — empty for the checkbox, which takes no placeholders.
    pub fn field_value_and_placeholders(&self, field: Field) -> (String, String) {
        match field {
            Field::Template(template) => {
                let value = self.templates.get(template);
                let value = if value.is_empty() {
                    self.text(Template::EditBlank, &[])
                } else {
                    value.to_string()
                };
                (value, template.placeholders().join(", "))
            }
            Field::EscapeExits => {
                let word = if self.preferences.escape_exits {
                    Template::WordOn
                } else {
                    Template::WordOff
                };
                (self.text(word, &[]), String::new())
            }
        }
    }

    pub fn field_label_text(&self, field: Field, index: usize, count: usize) -> String {
        self.text(
            Template::SettingsFieldLabel,
            &[
                ("name", field.label()),
                ("group", field.group().name()),
                ("index", &(index + 1).to_string()),
                ("count", &count.to_string()),
            ],
        )
    }

    pub fn field_help_text(&self, field: Field, index: usize, count: usize) -> String {
        let (placeholders, changed, default) = match field {
            Field::Template(template) => (
                template.placeholders().join(", "),
                !self.templates.is_default(template),
                template.default_text().to_string(),
            ),
            Field::EscapeExits => (
                String::new(),
                self.preferences.escape_exits,
                self.text(Template::WordOff, &[]),
            ),
        };
        let changed = if changed {
            self.text(Template::SettingsFieldChanged, &[])
        } else {
            String::new()
        };

        self.text(
            Template::SettingsFieldHelp,
            &[
                ("name", field.label()),
                ("group", field.group().name()),
                ("placeholders", &placeholders),
                ("changed", &changed),
                ("default", &default),
                ("index", &(index + 1).to_string()),
                ("count", &count.to_string()),
            ],
        )
    }

    // ---- Menu bar -----------------------------------------------------------

    /// A command's label — a plain word for most, but the feed's own user-editable name for `SelectFeed`, since a feed's name is a template like every other spoken word, not something this module invents.
    pub fn command_label(&self, command: Command) -> String {
        match command {
            Command::SelectFeed(feed) => self.templates.feed_title(feed),
            _ => command.label().unwrap_or_default().to_string(),
        }
    }

    /// Whether a checkbox or radio item should read as checked or selected. Always `false` for a plain command, which has no such state.
    pub fn command_marked(&self, command: Command, speech_active: bool) -> bool {
        match command {
            Command::SelectFeed(feed) => feed == self.feed,
            Command::ToggleSpeech => speech_active,
            _ => false,
        }
    }

    /// The spoken word for a checkbox or radio item's state, for a bare TTS engine to say explicitly. Blank for a plain command and for an unmarked radio item — a screen reader announces role and state from the native menu item itself, so this exists only for when nothing else is listening.
    pub fn command_state_text(&self, command: Command, speech_active: bool) -> String {
        let marked = self.command_marked(command, speech_active);
        match command.kind() {
            Kind::Normal => String::new(),
            Kind::Checkbox => {
                let word = if marked { Template::WordOn } else { Template::WordOff };
                self.text(word, &[])
            }
            Kind::Radio if marked => self.text(Template::WordCurrentFeed, &[]),
            Kind::Radio => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn story(id: u64, title: &str) -> Item {
        Item {
            id,
            by: Some("alice".into()),
            title: Some(title.into()),
            url: Some("https://www.example.com/post".into()),
            text: None,
            score: Some(42),
            descendants: Some(7),
            kids: vec![],
            time: None,
            deleted: false,
            dead: false,
            kind: Some("story".into()),
        }
    }

    fn app_with_stories(n: usize) -> App {
        let mut app = App::new(Feed::Top, Templates::default(), Preferences::default());
        app.stories = (0..n)
            .map(|i| story(i as u64, &format!("Story {i}")))
            .collect();
        app
    }

    #[test]
    fn cursor_clamps_at_both_ends() {
        let mut app = app_with_stories(3);
        assert!(!app.move_cursor(-1), "already at the top");
        assert!(app.move_cursor(1));
        assert_eq!(app.cursor(), 1);
        assert!(app.move_cursor(100));
        assert_eq!(app.cursor(), 2, "clamped to the last row");
        assert!(!app.move_cursor(5), "no movement means no announcement");
    }

    #[test]
    fn empty_list_has_no_cursor_movement() {
        let mut app = App::new(Feed::Top, Templates::default(), Preferences::default());
        assert!(!app.move_cursor(1));
        assert_eq!(app.row_count(), 0);
    }

    #[test]
    fn story_label_reads_position_title_and_metadata() {
        let app = app_with_stories(1);
        let label = app.row_label(0);
        assert!(label.starts_with("1. Story 0"));
        assert!(label.contains("example.com"));
        assert!(label.contains("42 points"));
        assert!(label.contains("by alice"));
        assert!(label.contains("7 comments"));
    }

    #[test]
    fn a_story_missing_its_metadata_does_not_read_stray_punctuation() {
        let mut app = app_with_stories(1);
        let bare = &mut app.stories[0];
        bare.url = None;
        bare.score = None;
        bare.by = None;
        bare.descendants = None;

        let label = app.row_label(0);
        assert_eq!(label, "1. Story 0, unknown time, no comments");
    }

    #[test]
    fn editing_a_template_changes_what_is_announced() {
        let mut app = app_with_stories(1);
        app.templates.set(Template::StoryRow, "{title} only");
        assert_eq!(app.row_label(0), "Story 0 only");

        app.templates.set(Template::CommentsMany, "{count} replies");
        app.templates.reset(Template::StoryRow);
        assert!(app.row_label(0).ends_with("7 replies"));
    }

    #[test]
    fn comments_carry_a_1_based_level_derived_from_depth() {
        let mut app = App::new(Feed::Top, Templates::default(), Preferences::default());
        app.view = View::Comments;
        app.comments = vec![CommentRow {
            item: story(2, "c2"),
            depth: 2,
        }];
        let args = app.comment_args(0, &app.comments[0]);
        assert!(args.iter().any(|(k, v)| *k == "level" && v == "3"));
    }

    /// A thread shaped like the real thing: flat, in reading order, with a depth per row. `depths` reads as the indentation it stands for.
    fn app_with_comments(depths: &[usize]) -> App {
        let mut app = App::new(Feed::Top, Templates::default(), Preferences::default());
        app.view = View::Comments;
        app.comments = depths
            .iter()
            .enumerate()
            .map(|(i, depth)| CommentRow {
                item: story(i as u64, &format!("c{i}")),
                depth: *depth,
            })
            .collect();
        app
    }

    #[test]
    fn collapsing_a_comment_hides_every_reply_below_it() {
        let mut app = app_with_comments(&[0, 1, 2, 1, 0]);
        assert_eq!(app.row_count(), 5);

        assert!(app.set_comment_collapsed(0, true));
        assert_eq!(app.visible_comments().collect::<Vec<_>>(), vec![0, 4]);
        assert_eq!(app.row_count(), 2, "the hidden replies are not rows to move to");
    }

    #[test]
    fn a_comment_with_no_replies_cannot_be_collapsed() {
        let mut app = app_with_comments(&[0, 1, 0]);
        assert!(!app.comment_has_replies(1), "a leaf");
        assert!(!app.set_comment_collapsed(1, true));
        assert_eq!(app.row_count(), 3);
    }

    #[test]
    fn moving_down_steps_over_a_collapsed_thread() {
        let mut app = app_with_comments(&[0, 1, 2, 0]);
        app.set_comment_collapsed(0, true);

        assert!(app.move_cursor(1));
        assert_eq!(app.cursor(), 3, "past the two hidden replies, not into them");
        assert!(!app.move_cursor(1), "already at the last row that is showing");
    }

    #[test]
    fn end_lands_on_the_last_row_showing_not_the_last_comment() {
        let mut app = app_with_comments(&[0, 0, 1, 2]);
        app.set_comment_collapsed(1, true);

        assert!(app.move_to_edge(true));
        assert_eq!(app.cursor(), 1);
        assert!(app.move_to_edge(false));
        assert_eq!(app.cursor(), 0);
    }

    #[test]
    fn position_counts_what_is_showing_and_cursor_indexes_the_thread() {
        let mut app = app_with_comments(&[0, 1, 1, 0]);
        app.set_comment_collapsed(0, true);
        app.move_cursor(1);

        assert_eq!(app.cursor(), 3, "the fourth comment of the thread");
        assert_eq!(app.position(), 1, "but the second row a listener can reach");
    }

    #[test]
    fn a_hidden_comment_is_not_somewhere_the_cursor_can_be_put() {
        let mut app = app_with_comments(&[0, 1, 0]);
        app.set_comment_collapsed(0, true);
        assert!(!app.move_to(1));
        assert_eq!(app.cursor(), 0);
    }

    #[test]
    fn a_reply_knows_the_comment_it_answers() {
        let app = app_with_comments(&[0, 1, 2, 2, 1, 0]);
        assert_eq!(app.comment_parent(0), None, "a top-level comment");
        assert_eq!(app.comment_parent(2), Some(1));
        assert_eq!(app.comment_parent(3), Some(1));
        assert_eq!(app.comment_parent(4), Some(0));
        assert_eq!(app.comment_reply_count(0), 4);
        assert_eq!(app.comment_reply_count(1), 2);
    }

    #[test]
    fn nesting_collapses_does_not_double_count_hidden_rows() {
        let mut app = app_with_comments(&[0, 1, 2, 0]);
        app.set_comment_collapsed(1, true);
        app.set_comment_collapsed(0, true);
        assert_eq!(app.visible_comments().collect::<Vec<_>>(), vec![0, 3]);

        app.set_comment_collapsed(0, false);
        assert_eq!(
            app.visible_comments().collect::<Vec<_>>(),
            vec![0, 1, 3],
            "the inner thread is still closed"
        );
    }

    #[test]
    fn closing_a_thread_the_cursor_was_inside_brings_it_back_out() {
        let mut app = app_with_comments(&[0, 1, 2, 0]);
        app.move_to(2);
        assert_eq!(app.cursor(), 2);

        app.set_comment_collapsed(0, true);
        assert_eq!(app.cursor(), 0, "onto the comment that was closed");
        assert_eq!(app.position(), 0);
    }

    #[test]
    fn a_new_thread_starts_with_every_reply_showing() {
        let mut app = app_with_comments(&[0, 1]);
        app.set_comment_collapsed(0, true);
        app.clear_comment_collapsed();
        assert_eq!(app.row_count(), 2);
    }

    #[test]
    fn settings_field_wording_reports_group_and_default_state() {
        let mut app = app_with_stories(1);
        app.view = View::Settings;
        assert!(app.field_focus_text(0).contains(Template::ALL[0].label()));

        app.templates.set(Template::ALL[0], "changed");
        assert!(
            app.field_help_text(Field::Template(Template::ALL[0]), 0, 1)
                .contains("Changed from the default")
        );
    }

    #[test]
    fn the_speech_toggle_reflects_speech_active() {
        let app = app_with_stories(1);
        assert!(app.command_marked(Command::ToggleSpeech, true));
        assert!(!app.command_marked(Command::ToggleSpeech, false));
    }

    #[test]
    fn only_the_current_feed_reads_as_marked() {
        let mut app = app_with_stories(1);
        app.feed = Feed::New;
        assert!(!app.command_marked(Command::SelectFeed(Feed::Top), true));
        assert!(app.command_marked(Command::SelectFeed(Feed::New), true));
    }
}
