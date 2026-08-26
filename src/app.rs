//! Application state and the wording it produces.
//!
//! There is no accessibility tree to build any more: `main.rs` owns real
//! wxWidgets controls (a `ListCtrl`, a `TreeCtrl`, a native `MenuBar`, a
//! settings `Dialog`) and the platform's own MSAA/UIA support is what a
//! screen reader reads. This module's job shrinks to exactly what it should
//! always have been about: deciding *what* the application is showing and
//! *what to say* about it.
//!
//! No wording is built by concatenation here either. Every string that
//! reaches the user's ears comes from `templates.rs`, so this module's job is
//! to decide *which* template applies and to hand it the values it needs.

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

/// The keyboard map, also rendered as the help view so it is discoverable
/// without sight or documentation.
pub const HELP: &[(&str, &str)] = &[
    ("Up / Down, J / K", "move through the list"),
    ("Home / End", "first or last item"),
    ("Page Up / Page Down", "move ten items"),
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

    /// Render a template. A thin wrapper, but it keeps call sites reading as
    /// "say this" rather than as string plumbing.
    pub fn text(&self, template: Template, args: &[(&str, &str)]) -> String {
        self.templates.render(template, args)
    }

    pub fn feed_title(&self) -> String {
        self.templates.feed_title(self.feed)
    }

    pub fn row_count(&self) -> usize {
        match self.view {
            View::Stories => self.stories.len(),
            View::Comments => self.comments.len(),
            View::Help => HELP.len(),
            View::Settings => 0,
        }
    }

    pub fn cursor(&self) -> usize {
        match self.view {
            View::Stories => self.story_cursor,
            View::Comments => self.comment_cursor,
            View::Help | View::Settings => 0,
        }
    }

    fn set_cursor(&mut self, index: usize) {
        match self.view {
            View::Stories => self.story_cursor = index,
            View::Comments => self.comment_cursor = index,
            // Neither has a cursor of its own: help is read as a whole, and the
            // settings dialog moves focus through its own tree/text controls.
            View::Help | View::Settings => {}
        }
    }

    /// Move the selection by `delta`, clamping at both ends.
    ///
    /// Clamping rather than wrapping is deliberate: hitting a hard stop tells a
    /// listener they are at the edge of the list without needing an extra
    /// announcement.
    pub fn move_cursor(&mut self, delta: isize) -> bool {
        let count = self.row_count();
        if count == 0 || self.view == View::Settings {
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

    pub fn move_to(&mut self, index: usize) -> bool {
        let count = self.row_count();
        if count == 0 || index >= count || index == self.cursor() || self.view == View::Settings {
            return false;
        }
        self.set_cursor(index);
        true
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

    /// Title for the container, which is what a screen reader announces on
    /// entering it.
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

    /// The full text of the selected row, spoken on demand. Unlike the label
    /// this is not truncated and keeps paragraph breaks.
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

    /// Everything a story template may ask for. Shared between the row label
    /// and the read-in-full text so the two cannot drift apart.
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

    /// The comment count is three templates rather than one, because a language
    /// that pluralizes differently cannot be served by appending an "s".
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

    /// A field's current value as spoken text, and its placeholder list — empty
    /// for the checkbox, which takes no placeholders.
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

    /// A command's label — a plain word for most, but the feed's own
    /// user-editable name for `SelectFeed`, since a feed's name is a template
    /// like every other spoken word, not something this module invents.
    pub fn command_label(&self, command: Command) -> String {
        match command {
            Command::SelectFeed(feed) => self.templates.feed_title(feed),
            _ => command.label().unwrap_or_default().to_string(),
        }
    }

    /// Whether a checkbox or radio item should read as checked or selected.
    /// Always `false` for a plain command, which has no such state.
    pub fn command_marked(&self, command: Command, speech_active: bool) -> bool {
        match command {
            Command::SelectFeed(feed) => feed == self.feed,
            Command::ToggleSpeech => speech_active,
            _ => false,
        }
    }

    /// The spoken word for a checkbox or radio item's state, for a bare TTS
    /// engine to say explicitly. Blank for a plain command and for an
    /// unmarked radio item — a screen reader announces role and state from
    /// the native menu item itself, so this exists only for when nothing
    /// else is listening.
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
