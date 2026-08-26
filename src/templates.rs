//! Every phrase this application says out loud, as an editable template.
//!
//! Nothing here is cosmetic. A screen reader user hears these strings hundreds
//! of times an hour, and what is a sensible default for one listener is
//! intolerable padding for another: some want the domain and the score on every
//! row, some want the headline and nothing else, and some want it in a language
//! this application does not ship. So no announcement is built by string
//! concatenation in the code — each one is a named template the user can edit
//! in the settings dialog (see `settings.rs`) and which is persisted to disk
//! (see `config.rs`).
//!
//! # Template syntax
//!
//! - `{name}` is replaced by a value. Which names are available differs per
//!   template and is listed in [`Template::placeholders`]; an unknown name
//!   renders as nothing.
//! - `[...]` marks an optional group: it disappears entirely if any placeholder
//!   directly inside it is empty. This is what lets one template cover a story
//!   that has a link, a score and an author and one that has none of the three,
//!   without leaving a trail of stray commas for the synthesizer to read.
//!   Groups nest, and each nested group is judged on its own placeholders.
//! - `\n` and `\t` produce a line break and a tab, which matters for the
//!   read-in-full templates; the settings editor is single-line, so this is the
//!   only way to type one.
//! - A backslash makes the next character literal, so `\[`, `\{` and `\\` are
//!   how those characters are written. Doubling them would read more nicely but
//!   cannot work: a group closing inside another ends in `]]`, which would be
//!   indistinguishable from an escaped bracket.
//!
//! Rendering never fails. A template with an unbalanced bracket produces its
//! own text rather than an error, because a user halfway through an edit still
//! needs the application to talk to them.

use crate::hn::Feed;

/// A broad category, used to group the fields in the settings dialog so the
/// list stays navigable as the number of templates grows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Stories,
    Comments,
    Help,
    Settings,
    Status,
    Window,
    Time,
    Words,
    /// Preferences that are not wording at all, such as what Escape does at
    /// the story list. Its one field lives in `settings::Field::EscapeExits`
    /// rather than in the template table below, since it has no text to edit.
    General,
    Menu,
}

impl Group {
    pub fn name(self) -> &'static str {
        match self {
            Group::Stories => "Story list",
            Group::Comments => "Comments",
            Group::Help => "Help",
            Group::Settings => "Settings dialog",
            Group::Status => "Status and announcements",
            Group::Window => "Window title",
            Group::Time => "Times",
            Group::Words => "Individual words",
            Group::General => "General",
            Group::Menu => "Menu bar",
        }
    }
}

/// Declares the template registry: the enum, the iteration order, and the
/// per-template metadata the settings dialog reads.
///
/// One macro rather than five parallel tables, because a template whose default
/// text and placeholder list drifted apart would be a silent wording bug.
macro_rules! templates {
    ($($variant:ident, $id:literal, $group:expr, $label:literal, $default:literal, [$($ph:literal),*];)*) => {
        /// One named, user-editable announcement.
        ///
        /// Declaration order is the order the settings dialog presents, so
        /// related templates are kept adjacent and grouped.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Template {
            $($variant,)*
        }

        impl Template {
            /// Every template, in presentation order.
            pub const ALL: &'static [Template] = &[$(Template::$variant,)*];

            /// Stable key used in the settings file. Unlike the variant name
            /// this must not change, or a user's edits stop loading.
            pub fn id(self) -> &'static str {
                match self { $(Template::$variant => $id,)* }
            }

            pub fn group(self) -> Group {
                match self { $(Template::$variant => $group,)* }
            }

            /// Human name, spoken as the label of the settings field.
            pub fn label(self) -> &'static str {
                match self { $(Template::$variant => $label,)* }
            }

            pub fn default_text(self) -> &'static str {
                match self { $(Template::$variant => $default,)* }
            }

            /// The placeholder names this template may use.
            pub fn placeholders(self) -> &'static [&'static str] {
                match self { $(Template::$variant => &[$($ph,)*],)* }
            }
        }
    };
}

templates! {
    // ---- Story list ------------------------------------------------------
    StoryRow, "story_row", Group::Stories, "Story row",
        "{index}. {title}[, {domain}][, {score} points][, by {author}], {age}, {comments}",
        ["index", "title", "domain", "url", "score", "author", "age", "comments", "count", "kind"];
    StoryDetail, "story_detail", Group::Stories, "Story, read in full",
        "[{kind}. ]{title}[, {score} points][, by {author}], {age}[\\nLink: {url}][\\n{body}]",
        ["index", "title", "domain", "url", "score", "author", "age", "comments", "count", "kind", "body"];
    StoriesTitle, "stories_title", Group::Stories, "Story list title",
        "{feed}, {count} stories",
        ["feed", "count"];
    CommentsNone, "comments_none", Group::Stories, "Comment count, none",
        "no comments",
        ["count"];
    CommentsOne, "comments_one", Group::Stories, "Comment count, one",
        "1 comment",
        ["count"];
    CommentsMany, "comments_many", Group::Stories, "Comment count, many",
        "{count} comments",
        ["count"];

    // ---- Comments --------------------------------------------------------
    CommentRow, "comment_row", Group::Comments, "Comment row",
        "{author}, {age}: {body}",
        ["index", "level", "author", "age", "body", "count"];
    CommentDetail, "comment_detail", Group::Comments, "Comment, read in full",
        "Level {level}. {author}, {age}.\\n{body}",
        ["index", "level", "author", "age", "body", "count"];
    CommentsTitle, "comments_title", Group::Comments, "Comment list title",
        "{count} comments on {title}",
        ["count", "title", "author", "score"];

    // ---- Help ------------------------------------------------------------
    HelpRow, "help_row", Group::Help, "Help row",
        "{keys}: {what}",
        ["index", "keys", "what", "count"];
    HelpTitle, "help_title", Group::Help, "Help title",
        "Keyboard help",
        ["count"];
    HelpDetail, "help_detail", Group::Help, "Help, read in full",
        "{rows}",
        ["rows", "count"];

    // ---- Settings dialog -------------------------------------------------
    SettingsTitle, "settings_title", Group::Settings, "Settings title",
        "Settings",
        ["tab", "index", "count"];
    SettingsIntro, "settings_intro", Group::Settings, "Settings, spoken on opening",
        "Settings, {tab}. F1 for keys, Escape to close.",
        ["tab", "index", "count"];
    SettingsKeys, "settings_keys", Group::Settings, "Settings keys, spoken on F1",
        "Tab or arrows move between controls. Control with Down or Up jumps to the next group. Left and right move through the text. Space or Enter toggles a checkbox. F5 restores this field's default. Escape closes and saves.",
        ["tab", "index", "count"];
    SettingsGroupLabel, "settings_group", Group::Settings, "Settings group label",
        "{name}",
        ["name", "count"];
    SettingsTabLabel, "settings_tab", Group::Settings, "Settings tab label",
        "{name}",
        ["name", "index", "count"];
    SettingsFieldLabel, "settings_field", Group::Settings, "Settings field label",
        "{name}",
        ["name", "group", "index", "count"];
    SettingsFieldHelp, "settings_field_help", Group::Settings, "Settings field description",
        "[Placeholders: {placeholders}.][ {changed}]",
        ["name", "group", "placeholders", "changed", "default", "index", "count"];
    SettingsFieldChanged, "settings_field_changed", Group::Settings, "Settings field, edited marker",
        "Changed from the default.",
        [];
    SettingsFieldFocus, "settings_field_focus", Group::Settings, "Settings field, spoken on focus",
        "{name}, edit, {value}",
        ["name", "group", "value", "placeholders", "index", "count"];
    SettingsTabFocus, "settings_tab_focus", Group::Settings, "Settings tab, spoken on focus",
        "{name} tab, {index} of {count}",
        ["name", "index", "count"];
    SettingsSaved, "settings_saved", Group::Settings, "Settings saved",
        "Settings saved",
        ["path"];
    SettingsSaveFailed, "settings_save_failed", Group::Settings, "Settings could not be saved",
        "Could not save settings: {error}",
        ["error", "path"];
    TemplateReset, "template_reset", Group::Settings, "Template restored",
        "{name} restored to its default",
        ["name", "default"];
    TemplateInvalid, "template_invalid", Group::Settings, "Template has a problem",
        "{name}: {problem}",
        ["name", "problem"];
    EditCaret, "edit_caret", Group::Settings, "Character under the cursor",
        "{char}",
        ["char", "position", "count"];
    EditDelete, "edit_delete", Group::Settings, "Character deleted",
        "{char}",
        ["char", "position", "count"];
    EditEnd, "edit_end", Group::Settings, "Cursor at the end of a field",
        "End",
        ["count"];
    EditSpace, "edit_space", Group::Settings, "Name of the space character",
        "space",
        [];
    EditBlank, "edit_blank", Group::Settings, "Empty field",
        "Blank",
        [];

    // ---- Menu bar ----------------------------------------------------------
    MenuOpened, "menu_opened", Group::Menu, "Menu opened",
        "Menu opened. Escape to close.[ {label}]",
        ["label"];
    MenuBarFocus, "menu_bar_focus", Group::Menu, "Menu, spoken on a top-level item",
        "{name} menu, {index} of {count}",
        ["name", "index", "count"];
    MenuItemFocus, "menu_item_focus", Group::Menu, "Menu item, spoken on focus",
        "{name}[, {state}], {index} of {count}",
        ["name", "state", "index", "count"];

    // ---- Status and announcements ---------------------------------------
    AnnounceView, "announce_view", Group::Status, "Entering a view",
        "{status}[. {label}]",
        ["status", "label", "position", "count"];
    AnnounceRow, "announce_row", Group::Status, "Moving to a row",
        "{label}",
        ["label", "position", "count"];
    StatusStarting, "status_starting", Group::Status, "Starting up",
        "Starting",
        [];
    StatusLoadingFeed, "status_loading_feed", Group::Status, "Loading a feed",
        "Loading {feed}",
        ["feed"];
    StatusLoadingComments, "status_loading_comments", Group::Status, "Loading comments",
        "Loading comments",
        ["title", "count"];
    StatusFeedLoaded, "status_feed_loaded", Group::Status, "Feed loaded",
        "{feed}, {count} stories",
        ["feed", "count"];
    StatusCommentsLoaded, "status_comments_loaded", Group::Status, "Comments loaded",
        "{count} comments",
        ["count", "title"];
    StatusFeedError, "status_feed_error", Group::Status, "Feed failed to load",
        "Could not load {feed}: {error}",
        ["feed", "error"];
    StatusCommentsError, "status_comments_error", Group::Status, "Comments failed to load",
        "Could not load comments: {error}",
        ["error", "title"];
    StatusNothingSelected, "status_nothing_selected", Group::Status, "Nothing selected",
        "Nothing selected",
        [];
    StatusNoComments, "status_no_comments", Group::Status, "Story has no comments",
        "This story has no comments",
        ["title"];
    StatusAtTop, "status_at_top", Group::Status, "Already at the story list",
        "Already at the story list",
        [];
    StatusOpened, "status_opened", Group::Status, "Opened in the browser",
        "Opened {what} in your browser",
        ["what", "url"];
    StatusOpenFailed, "status_open_failed", Group::Status, "Could not open in the browser",
        "Could not open {what}: {error}",
        ["what", "url", "error"];
    StatusNoLink, "status_no_link", Group::Status, "No link for this item",
        "No {what} for this item",
        ["what"];
    StatusSpeechOn, "status_speech_on", Group::Status, "Prism speech turned on",
        "Prism speech on",
        ["backend"];
    StatusSpeechOff, "status_speech_off", Group::Status, "Prism speech turned off",
        "Prism speech off",
        ["backend"];
    StatusHelp, "status_help", Group::Status, "Entering help",
        "Keyboard help. Output: {output}",
        ["output", "count"];

    // ---- Window title ----------------------------------------------------
    WindowTitle, "window_title", Group::Window, "Window title",
        "hn-blind — {title}[ ({position} of {count})]",
        ["title", "position", "count"];

    // ---- Times -----------------------------------------------------------
    TimeUnknown, "time_unknown", Group::Time, "Unknown time",
        "unknown time",
        [];
    TimeNow, "time_now", Group::Time, "Less than a minute ago",
        "just now",
        [];
    TimeMinute, "time_minute", Group::Time, "One minute ago",
        "1 minute ago",
        ["count"];
    TimeMinutes, "time_minutes", Group::Time, "Several minutes ago",
        "{count} minutes ago",
        ["count"];
    TimeHour, "time_hour", Group::Time, "One hour ago",
        "1 hour ago",
        ["count"];
    TimeHours, "time_hours", Group::Time, "Several hours ago",
        "{count} hours ago",
        ["count"];
    TimeDay, "time_day", Group::Time, "One day ago",
        "1 day ago",
        ["count"];
    TimeDays, "time_days", Group::Time, "Several days ago",
        "{count} days ago",
        ["count"];

    // ---- Individual words -------------------------------------------------
    WordUntitled, "word_untitled", Group::Words, "A story with no title",
        "(untitled)",
        [];
    WordUnknownAuthor, "word_unknown_author", Group::Words, "An item with no author",
        "unknown",
        [];
    WordNoText, "word_no_text", Group::Words, "A comment with no text",
        "(no text)",
        [];
    WordLink, "word_link", Group::Words, "The story's own link",
        "link",
        [];
    WordDiscussion, "word_discussion", Group::Words, "The Hacker News page",
        "discussion",
        [];
    WordComment, "word_comment", Group::Words, "A single comment's page",
        "comment",
        [];
    WordJob, "word_job", Group::Words, "A job post",
        "Job post",
        [];
    WordPoll, "word_poll", Group::Words, "A poll",
        "Poll",
        [];
    WordOn, "word_on", Group::Words, "A checkbox that is checked",
        "on",
        [];
    WordOff, "word_off", Group::Words, "A checkbox that is not checked",
        "off",
        [];
    WordCurrentFeed, "word_current_feed", Group::Words, "The feed currently showing, in the Feed menu",
        "current feed",
        [];
    FeedTop, "feed_top", Group::Words, "Feed name: top",
        "Top stories",
        [];
    FeedNew, "feed_new", Group::Words, "Feed name: new",
        "New stories",
        [];
    FeedBest, "feed_best", Group::Words, "Feed name: best",
        "Best stories",
        [];
    FeedAsk, "feed_ask", Group::Words, "Feed name: ask",
        "Ask HN",
        [];
    FeedShow, "feed_show", Group::Words, "Feed name: show",
        "Show HN",
        [];
    FeedJob, "feed_job", Group::Words, "Feed name: jobs",
        "Jobs",
        [];
}

/// The full set of templates in force, defaults plus the user's edits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Templates {
    /// Indexed by `Template as usize`, so lookup is an array index rather than
    /// a hash on a path that runs for every row of every list.
    values: Vec<String>,
}

impl Default for Templates {
    fn default() -> Self {
        Templates {
            values: Template::ALL
                .iter()
                .map(|t| t.default_text().to_string())
                .collect(),
        }
    }
}

impl Templates {
    pub fn get(&self, template: Template) -> &str {
        &self.values[template as usize]
    }

    pub fn set(&mut self, template: Template, text: impl Into<String>) {
        self.values[template as usize] = text.into();
    }

    pub fn reset(&mut self, template: Template) {
        self.values[template as usize] = template.default_text().to_string();
    }

    pub fn is_default(&self, template: Template) -> bool {
        self.values[template as usize] == template.default_text()
    }

    /// The templates the user has actually changed. Only these are written to
    /// the settings file, so improvements to the defaults still reach anyone
    /// who has not overridden them.
    pub fn changed(&self) -> impl Iterator<Item = (Template, &str)> {
        Template::ALL
            .iter()
            .copied()
            .filter(|t| !self.is_default(*t))
            .map(|t| (t, self.get(t)))
    }

    /// Fill in a template's placeholders.
    pub fn render(&self, template: Template, args: &[(&str, &str)]) -> String {
        render_text(self.get(template), args)
    }

    /// The name of a feed, which is spoken often enough to be worth templating.
    pub fn feed_title(&self, feed: Feed) -> String {
        let template = match feed {
            Feed::Top => Template::FeedTop,
            Feed::New => Template::FeedNew,
            Feed::Best => Template::FeedBest,
            Feed::Ask => Template::FeedAsk,
            Feed::Show => Template::FeedShow,
            Feed::Job => Template::FeedJob,
        };
        self.render(template, &[])
    }
}

/// One nesting level of an optional `[...]` group while rendering.
struct Frame {
    text: String,
    /// Cleared by an empty placeholder, which drops the whole group.
    keep: bool,
}

impl Frame {
    fn new() -> Self {
        Frame {
            text: String::new(),
            keep: true,
        }
    }
}

/// Substitute `args` into a template string.
///
/// Deliberately total: any input produces some output. Malformed syntax is
/// rendered as the literal text the user typed, which they will hear and can
/// correct, rather than an error that leaves them with silence.
pub fn render_text(source: &str, args: &[(&str, &str)]) -> String {
    let mut frames = vec![Frame::new()];
    let mut chars = source.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                let escaped = match chars.next() {
                    Some('n') => '\n',
                    Some('t') => '\t',
                    Some(other) => other,
                    None => '\\',
                };
                push(&mut frames, escaped);
            }
            '{' => {
                let mut name = String::new();
                let mut closed = false;
                for c in chars.by_ref() {
                    if c == '}' {
                        closed = true;
                        break;
                    }
                    name.push(c);
                }

                let frame = frames.last_mut().expect("the outermost frame remains");
                if closed {
                    let value = args
                        .iter()
                        .find(|(key, _)| *key == name)
                        .map(|(_, value)| *value)
                        .unwrap_or("");
                    if value.is_empty() {
                        frame.keep = false;
                    }
                    frame.text.push_str(value);
                } else {
                    // No closing brace: this was never a placeholder.
                    frame.text.push('{');
                    frame.text.push_str(&name);
                }
            }
            '[' => frames.push(Frame::new()),
            ']' => {
                if frames.len() > 1 {
                    let frame = frames.pop().expect("checked above");
                    if frame.keep {
                        push_str(&mut frames, &frame.text);
                    }
                } else {
                    push(&mut frames, ']');
                }
            }
            c => push(&mut frames, c),
        }
    }

    // Unclosed groups are kept: the user is mid-edit, not asking for silence.
    while frames.len() > 1 {
        let frame = frames.pop().expect("checked above");
        push_str(&mut frames, &frame.text);
    }
    frames.pop().expect("the outermost frame remains").text
}

fn push(frames: &mut [Frame], c: char) {
    frames
        .last_mut()
        .expect("the outermost frame remains")
        .text
        .push(c);
}

fn push_str(frames: &mut [Frame], text: &str) {
    frames
        .last_mut()
        .expect("the outermost frame remains")
        .text
        .push_str(text);
}

/// Describe the first problem with a template, in a sentence fit to be spoken.
///
/// Used when the settings dialog is closed rather than on every keystroke:
/// a half-typed placeholder is not a mistake, it is the middle of a word.
pub fn validate(template: Template, source: &str) -> Option<String> {
    let allowed = template.placeholders();
    let mut depth = 0usize;
    let mut chars = source.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                chars.next();
            }
            '{' => {
                let mut name = String::new();
                let mut closed = false;
                for c in chars.by_ref() {
                    if c == '}' {
                        closed = true;
                        break;
                    }
                    name.push(c);
                }
                if !closed {
                    return Some(format!("the placeholder {name} has no closing brace"));
                }
                if !allowed.contains(&name.as_str()) {
                    return Some(match allowed {
                        [] => format!("{name} is not available here; this template takes none"),
                        _ => format!("{name} is not available here; try {}", allowed.join(", ")),
                    });
                }
            }
            '[' => depth += 1,
            ']' => {
                if depth == 0 {
                    return Some("there is a closing bracket with no opening one".to_string());
                }
                depth -= 1;
            }
            _ => {}
        }
    }

    (depth > 0).then(|| "an optional group has no closing bracket".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_defaults_are_well_formed() {
        let mut ids: Vec<&str> = Template::ALL.iter().map(|t| t.id()).collect();
        ids.sort_unstable();
        let count = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), count, "template ids must be unique");

        // A default that used a placeholder it does not declare would be a
        // wording bug nobody would notice until it was spoken.
        for template in Template::ALL {
            assert_eq!(
                validate(*template, template.default_text()),
                None,
                "default for {} is not valid",
                template.id()
            );
        }
    }

    #[test]
    fn variant_order_matches_the_index_used_for_storage() {
        let templates = Templates::default();
        for (index, template) in Template::ALL.iter().enumerate() {
            assert_eq!(index, *template as usize);
            assert_eq!(templates.get(*template), template.default_text());
        }
    }

    #[test]
    fn placeholders_are_substituted() {
        assert_eq!(
            render_text("{a} and {b}", &[("a", "one"), ("b", "two")]),
            "one and two"
        );
        assert_eq!(render_text("{missing}", &[]), "");
    }

    #[test]
    fn optional_groups_disappear_when_a_placeholder_is_empty() {
        let template = "{title}[, {score} points][, by {author}]";
        assert_eq!(
            render_text(template, &[("title", "T"), ("score", "9"), ("author", "a")]),
            "T, 9 points, by a"
        );
        assert_eq!(
            render_text(template, &[("title", "T"), ("author", "a")]),
            "T, by a"
        );
        assert_eq!(render_text(template, &[("title", "T")]), "T");
    }

    #[test]
    fn nested_groups_are_judged_on_their_own_placeholders() {
        // The inner group goes; the outer one stays, because `a` is present.
        assert_eq!(
            render_text("[{a}[ ({b})]]", &[("a", "one")]),
            "one",
            "an empty inner group must not take the outer one with it"
        );
        assert_eq!(
            render_text("[{a}[ ({b})]]", &[("a", "one"), ("b", "two")]),
            "one (two)"
        );
        assert_eq!(render_text("[{a}[ ({b})]]", &[("b", "two")]), "");
    }

    #[test]
    fn a_group_without_placeholders_is_always_kept() {
        assert_eq!(render_text("[always]", &[]), "always");
    }

    #[test]
    fn escapes_produce_literal_characters() {
        assert_eq!(render_text(r"\{a\} \[b\]", &[("a", "x")]), "{a} [b]");
        assert_eq!(render_text(r"one\ntwo", &[]), "one\ntwo");
        assert_eq!(render_text(r"a\\b", &[]), r"a\b");
        // An escaped brace is text, so it can neither name a placeholder nor
        // empty a group.
        assert_eq!(render_text(r"[\{a\} {b}]", &[("b", "y")]), "{a} y");
    }

    #[test]
    fn malformed_templates_still_render_something() {
        assert_eq!(render_text("{unclosed", &[]), "{unclosed");
        assert_eq!(render_text("a]b", &[]), "a]b");
        assert_eq!(render_text("[{a} unclosed", &[("a", "x")]), "x unclosed");
        assert_eq!(render_text("", &[]), "");
    }

    #[test]
    fn validation_reports_the_problems_a_user_can_make() {
        assert!(validate(Template::StoryRow, "{title}").is_none());
        assert!(
            validate(Template::StoryRow, "{ttile}")
                .expect("a typo is a problem")
                .contains("ttile")
        );
        assert!(validate(Template::StoryRow, "[{title}").is_some());
        assert!(validate(Template::StoryRow, "{title}]").is_some());
        assert!(validate(Template::StoryRow, "{title").is_some());
        // Escaped brackets and braces are text, not syntax.
        assert!(validate(Template::StoryRow, r"\{title\} \[ \]").is_none());
    }

    #[test]
    fn only_edited_templates_are_reported_as_changed() {
        let mut templates = Templates::default();
        assert_eq!(templates.changed().count(), 0);

        templates.set(Template::StoryRow, "{title}");
        assert_eq!(
            templates.changed().collect::<Vec<_>>(),
            vec![(Template::StoryRow, "{title}")]
        );

        templates.reset(Template::StoryRow);
        assert_eq!(templates.changed().count(), 0);
    }
}
