//! Yes/no preferences, persisted between runs.
//!
//! Kept out of `config.rs`: that file's contract is "one string per changed template," and a switch such as whether Escape quits the application is neither a template nor text a user edits, so it gets its own small file rather than stretching that contract to fit.
//!
//! As with the templates file, a missing, unreadable or corrupt file is never fatal: the application starts with its defaults and says why.

use std::path::PathBuf;

use serde_json::{Map, Value};

use crate::config;

const FILE: &str = "preferences.json";

/// The update channel this binary was built for: `dev` from the workflow that publishes every push to main, unset for a tagged release or a local build. Set by `build.rs`.
const BUILD_CHANNEL: &str = env!("HN_BLIND_CHANNEL");

/// One on/off preference. Each is a checkbox in the settings dialog and a boolean in the preferences file, and this is the single list both are built from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toggle {
    /// Whether Escape, pressed at the story list, quits the application instead of announcing that there is nowhere further back to go.
    EscapeExits,
    /// Whether to look for a new version, quietly, each time the application starts.
    CheckForUpdatesOnStartup,
    /// Whether updates come from the rolling development build of main rather than from tagged releases.
    DevUpdates,
}

impl Toggle {
    pub const ALL: &[Toggle] = &[Toggle::EscapeExits, Toggle::CheckForUpdatesOnStartup, Toggle::DevUpdates];

    /// The key in the preferences file. Never change one: a renamed key silently resets every user's choice.
    pub fn id(self) -> &'static str {
        match self {
            Toggle::EscapeExits => "escape_exits",
            Toggle::CheckForUpdatesOnStartup => "check_for_updates_on_startup",
            Toggle::DevUpdates => "dev_updates",
        }
    }

    /// The value a user who has never touched this preference gets.
    ///
    /// Startup checks are on because a screen reader user has no toolbar badge or tray balloon to notice a new version by; being asked is the only way they would ever hear of one. A development build defaults to the development channel, since someone who went out of their way to fetch one would otherwise hear nothing new until the next tagged release.
    pub fn default_value(self) -> bool {
        match self {
            Toggle::EscapeExits => false,
            Toggle::CheckForUpdatesOnStartup => true,
            Toggle::DevUpdates => BUILD_CHANNEL == "dev",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Preferences {
    pub escape_exits: bool,
    pub check_for_updates_on_startup: bool,
    pub dev_updates: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Preferences {
            escape_exits: Toggle::EscapeExits.default_value(),
            check_for_updates_on_startup: Toggle::CheckForUpdatesOnStartup.default_value(),
            dev_updates: Toggle::DevUpdates.default_value(),
        }
    }
}

impl Preferences {
    pub fn get(&self, toggle: Toggle) -> bool {
        match toggle {
            Toggle::EscapeExits => self.escape_exits,
            Toggle::CheckForUpdatesOnStartup => self.check_for_updates_on_startup,
            Toggle::DevUpdates => self.dev_updates,
        }
    }

    pub fn set(&mut self, toggle: Toggle, value: bool) {
        match toggle {
            Toggle::EscapeExits => self.escape_exits = value,
            Toggle::CheckForUpdatesOnStartup => self.check_for_updates_on_startup = value,
            Toggle::DevUpdates => self.dev_updates = value,
        }
    }
}

/// The preferences file's path, or `None` if the platform gave us no home to put it in.
pub fn path() -> Option<PathBuf> {
    Some(config::config_dir()?.join(FILE))
}

/// Load the user's preferences, falling back to defaults for anything missing.
///
/// Returns the preferences and, when something was wrong with the file, a sentence about it fit to be spoken as the first status message.
pub fn load() -> (Preferences, Option<String>) {
    let mut preferences = Preferences::default();

    let Some(path) = path() else {
        return (preferences, None);
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        // No file yet is the normal case on a first run, not a problem.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return (preferences, None),
        Err(err) => return (preferences, Some(format!("Could not read preferences: {err}"))),
    };

    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(map)) => {
            let note = apply(&mut preferences, &map);
            (preferences, note)
        }
        Ok(_) => (
            preferences,
            Some("Preferences file is not a set of preferences; using defaults".to_string()),
        ),
        Err(err) => (
            preferences,
            Some(format!("Could not read preferences: {err}; using defaults")),
        ),
    }
}

/// Apply the file's entries, reporting any that are not on or off.
///
/// An entry the file lacks keeps its default, which is how a preference added in a newer version reaches someone whose file predates it. Keys this version does not know are ignored: they are what a file written by a newer version looks like.
fn apply(preferences: &mut Preferences, map: &Map<String, Value>) -> Option<String> {
    let mut bad = Vec::new();
    for toggle in Toggle::ALL {
        match map.get(toggle.id()) {
            None => {}
            Some(Value::Bool(value)) => preferences.set(*toggle, *value),
            Some(_) => bad.push(toggle.id()),
        }
    }
    (!bad.is_empty()).then(|| format!("Preferences file has {} that is not on or off", bad.join(", ")))
}

/// Write the user's preferences, creating the directory if need be.
///
/// Every preference is written, not only the changed ones as with templates: the update channel's default depends on which build is running, and a choice the user made should not flip because they later ran a different build.
pub fn save(preferences: &Preferences) -> Result<PathBuf, String> {
    let path = path().ok_or_else(|| "no configuration directory on this system".to_string())?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("{}: {err}", parent.display()))?;
    }

    let map: Map<String, Value> = Toggle::ALL
        .iter()
        .map(|toggle| (toggle.id().to_string(), Value::Bool(preferences.get(*toggle))))
        .collect();
    let mut text = serde_json::to_string_pretty(&Value::Object(map))
        .map_err(|err| format!("could not encode preferences: {err}"))?;
    text.push('\n');

    std::fs::write(&path, text).map_err(|err| format!("{}: {err}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_leave_the_story_list_escape_alone_and_check_for_updates() {
        let preferences = Preferences::default();
        assert!(!preferences.escape_exits);
        assert!(preferences.check_for_updates_on_startup);
    }

    #[test]
    fn a_valid_entry_is_applied_and_an_invalid_one_is_reported() {
        let mut preferences = Preferences::default();
        let map: Map<String, Value> =
            serde_json::from_str(r#"{ "escape_exits": true, "dev_updates": true }"#).unwrap();
        assert_eq!(apply(&mut preferences, &map), None);
        assert!(preferences.escape_exits);
        assert!(preferences.dev_updates);
        assert!(preferences.check_for_updates_on_startup, "a missing entry keeps its default");

        let map: Map<String, Value> =
            serde_json::from_str(r#"{ "escape_exits": "yes" }"#).unwrap();
        assert!(apply(&mut preferences, &map).is_some());
    }

    #[test]
    fn every_toggle_round_trips_through_get_and_set() {
        let mut preferences = Preferences::default();
        for toggle in Toggle::ALL {
            let flipped = !preferences.get(*toggle);
            preferences.set(*toggle, flipped);
            assert_eq!(preferences.get(*toggle), flipped);
        }
    }
}
