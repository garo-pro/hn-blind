//! The menu bar's declarative structure: a second, discoverable way to reach every command, for a user who does not already know the letter keys by heart.
//!
//! This is now a real `wxMenuBar` built in `main.rs` — wxWidgets owns the open/close state, the highlighting, and Left/Right/Up/Down/Enter/Escape navigation, and exposes all of it to a screen reader on its own. What stays here is the one thing that must not drift out of sync with the keyboard: which command lives under which entry, and the stable numeric id `main.rs` uses to build the native menu and to dispatch its events back to a [`Command`].

use crate::hn::Feed;

/// Something the menu can do. Each names the exact effect an existing key already has, so choosing an item and pressing its key are the same action reaching the application through two different doors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    SelectFeed(Feed),
    Reload,
    OpenComments,
    OpenLink,
    OpenDiscussion,
    ReadInFull,
    StopSpeaking,
    ToggleSpeech,
    OpenSettings,
    OpenHelp,
    Quit,
}

/// How a command's menu item should be built: a plain item, a radio item (feeds — exactly one is ever current), or a check item (the speech toggle). Read by `app::command_marked`/`command_state_text`, which is also where the *value* of the mark comes from, since that lives in application state this module knows nothing about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Normal,
    Radio,
    Checkbox,
}

/// Base ids for each fixed command, spaced out to leave room for `SelectFeed` (one id per entry in `Feed::ALL`) without colliding with the rest.
const ID_FEED_BASE: i32 = 100;
const ID_RELOAD: i32 = 200;
const ID_OPEN_COMMENTS: i32 = 201;
const ID_OPEN_LINK: i32 = 202;
const ID_OPEN_DISCUSSION: i32 = 203;
const ID_READ_IN_FULL: i32 = 204;
const ID_STOP_SPEAKING: i32 = 205;
const ID_TOGGLE_SPEECH: i32 = 206;
const ID_OPEN_SETTINGS: i32 = 207;
const ID_OPEN_HELP: i32 = 208;
const ID_QUIT: i32 = 209;

impl Command {
    /// The label for commands whose wording is not user-editable. `None` for `SelectFeed`, whose text is a template (see `Templates::feed_title`) like every other user-facing word in this application.
    pub fn label(self) -> Option<&'static str> {
        match self {
            Command::SelectFeed(_) => None,
            Command::Reload => Some("Reload"),
            Command::OpenComments => Some("Open Comments"),
            Command::OpenLink => Some("Open Story Link"),
            Command::OpenDiscussion => Some("Open Discussion Page"),
            Command::ReadInFull => Some("Read in Full"),
            Command::StopSpeaking => Some("Stop Speaking"),
            Command::ToggleSpeech => Some("Prism Speech"),
            Command::OpenSettings => Some("Settings"),
            Command::OpenHelp => Some("Keyboard Help"),
            Command::Quit => Some("Quit"),
        }
    }

    pub fn kind(self) -> Kind {
        match self {
            Command::SelectFeed(_) => Kind::Radio,
            Command::ToggleSpeech => Kind::Checkbox,
            _ => Kind::Normal,
        }
    }

    /// The stable wx menu item id for this command.
    pub fn id(self) -> i32 {
        match self {
            Command::SelectFeed(feed) => {
                let index = Feed::ALL.iter().position(|f| *f == feed).unwrap_or(0);
                ID_FEED_BASE + index as i32
            }
            Command::Reload => ID_RELOAD,
            Command::OpenComments => ID_OPEN_COMMENTS,
            Command::OpenLink => ID_OPEN_LINK,
            Command::OpenDiscussion => ID_OPEN_DISCUSSION,
            Command::ReadInFull => ID_READ_IN_FULL,
            Command::StopSpeaking => ID_STOP_SPEAKING,
            Command::ToggleSpeech => ID_TOGGLE_SPEECH,
            Command::OpenSettings => ID_OPEN_SETTINGS,
            Command::OpenHelp => ID_OPEN_HELP,
            Command::Quit => ID_QUIT,
        }
    }

    /// Recover a command from a wx menu event's id. The inverse of [`id`].
    pub fn from_id(id: i32) -> Option<Command> {
        if (ID_FEED_BASE..ID_FEED_BASE + Feed::ALL.len() as i32).contains(&id) {
            return Some(Command::SelectFeed(Feed::ALL[(id - ID_FEED_BASE) as usize]));
        }
        Some(match id {
            ID_RELOAD => Command::Reload,
            ID_OPEN_COMMENTS => Command::OpenComments,
            ID_OPEN_LINK => Command::OpenLink,
            ID_OPEN_DISCUSSION => Command::OpenDiscussion,
            ID_READ_IN_FULL => Command::ReadInFull,
            ID_STOP_SPEAKING => Command::StopSpeaking,
            ID_TOGGLE_SPEECH => Command::ToggleSpeech,
            ID_OPEN_SETTINGS => Command::OpenSettings,
            ID_OPEN_HELP => Command::OpenHelp,
            ID_QUIT => Command::Quit,
            _ => return None,
        })
    }
}

/// One entry of a menu: a command, or a visual break with nothing to focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    Entry(Command),
    Separator,
}

/// A top-level menu and the entries it opens onto.
pub struct Bar {
    pub name: &'static str,
    pub items: &'static [Item],
}

impl Bar {
    /// The entries a user can actually land on. Separators are skipped because nothing focuses them, so counting them would make every spoken position wrong.
    pub fn commands(&self) -> impl Iterator<Item = Command> + '_ {
        self.items.iter().filter_map(|item| match item {
            Item::Entry(command) => Some(*command),
            Item::Separator => None,
        })
    }
}

/// Where a command sits in its menu: its 1-based position, and how many entries that menu holds.
///
/// wxWidgets tells us *which* item is highlighted but not where it is in the menu, and this is the only place that knows — so with no screen reader running to say it, this is where "3 of 8" comes from.
pub fn position_of(command: Command) -> Option<(usize, usize)> {
    BARS.iter().find_map(|bar| {
        let count = bar.commands().count();
        bar.commands()
            .position(|entry| entry == command)
            .map(|index| (index + 1, count))
    })
}

pub const BARS: &[Bar] = &[
    Bar {
        name: "Feed",
        items: &[
            Item::Entry(Command::SelectFeed(Feed::Top)),
            Item::Entry(Command::SelectFeed(Feed::New)),
            Item::Entry(Command::SelectFeed(Feed::Best)),
            Item::Entry(Command::SelectFeed(Feed::Ask)),
            Item::Entry(Command::SelectFeed(Feed::Show)),
            Item::Entry(Command::SelectFeed(Feed::Job)),
            Item::Separator,
            Item::Entry(Command::Reload),
        ],
    },
    Bar {
        name: "Story",
        items: &[
            Item::Entry(Command::OpenComments),
            Item::Entry(Command::OpenLink),
            Item::Entry(Command::OpenDiscussion),
            Item::Entry(Command::ReadInFull),
        ],
    },
    Bar {
        name: "Speech",
        items: &[
            Item::Entry(Command::StopSpeaking),
            Item::Entry(Command::ToggleSpeech),
        ],
    },
    Bar {
        name: "Application",
        items: &[
            Item::Entry(Command::OpenSettings),
            Item::Entry(Command::OpenHelp),
            Item::Separator,
            Item::Entry(Command::Quit),
        ],
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_in_the_bars_round_trips_through_its_id() {
        for bar in BARS {
            for item in bar.items {
                if let Item::Entry(command) = item {
                    assert_eq!(Command::from_id(command.id()), Some(*command));
                }
            }
        }
    }

    #[test]
    fn unknown_ids_map_to_nothing() {
        assert_eq!(Command::from_id(-1), None);
    }

    #[test]
    fn a_commands_position_counts_entries_and_not_separators() {
        // Reload sits after a separator, which nothing focuses, so it is the seventh entry of the Feed menu rather than the eighth.
        assert_eq!(position_of(Command::Reload), Some((7, 7)));
        assert_eq!(position_of(Command::SelectFeed(Feed::Top)), Some((1, 7)));
        assert_eq!(position_of(Command::Quit), Some((3, 3)));
    }
}
