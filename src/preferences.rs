//! Yes/no preferences, persisted between runs.
//!
//! Kept out of `config.rs`: that file's contract is "one string per changed
//! template," and a switch such as whether Escape quits the application is
//! neither a template nor text a user edits, so it gets its own small file
//! rather than stretching that contract to fit.
//!
//! As with the templates file, a missing, unreadable or corrupt file is never
//! fatal: the application starts with its defaults and says why.

use std::path::PathBuf;

use serde_json::{Map, Value};

use crate::config;

const FILE: &str = "preferences.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Preferences {
    /// Whether Escape, pressed at the story list, quits the application
    /// instead of announcing that there is nowhere further back to go.
    pub escape_exits: bool,
}

/// The preferences file's path, or `None` if the platform gave us no home to
/// put it in.
pub fn path() -> Option<PathBuf> {
    Some(config::config_dir()?.join(FILE))
}

/// Load the user's preferences, falling back to defaults for anything missing.
///
/// Returns the preferences and, when something was wrong with the file, a
/// sentence about it fit to be spoken as the first status message.
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

/// Apply the file's `escape_exits` entry, reporting anything wrong with it.
fn apply(preferences: &mut Preferences, map: &Map<String, Value>) -> Option<String> {
    match map.get("escape_exits") {
        None => None,
        Some(Value::Bool(value)) => {
            preferences.escape_exits = *value;
            None
        }
        Some(_) => Some("Preferences file has escape_exits that is not on or off".to_string()),
    }
}

/// Write the user's preferences, creating the directory if need be.
pub fn save(preferences: &Preferences) -> Result<PathBuf, String> {
    let path = path().ok_or_else(|| "no configuration directory on this system".to_string())?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("{}: {err}", parent.display()))?;
    }

    let value = serde_json::json!({ "escape_exits": preferences.escape_exits });
    let mut text = serde_json::to_string_pretty(&value)
        .map_err(|err| format!("could not encode preferences: {err}"))?;
    text.push('\n');

    std::fs::write(&path, text).map_err(|err| format!("{}: {err}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_leave_the_story_list_escape_alone() {
        assert!(!Preferences::default().escape_exits);
    }

    #[test]
    fn a_valid_entry_is_applied_and_an_invalid_one_is_reported() {
        let mut preferences = Preferences::default();
        let map: Map<String, Value> =
            serde_json::from_str(r#"{ "escape_exits": true }"#).unwrap();
        assert_eq!(apply(&mut preferences, &map), None);
        assert!(preferences.escape_exits);

        let map: Map<String, Value> =
            serde_json::from_str(r#"{ "escape_exits": "yes" }"#).unwrap();
        assert!(apply(&mut preferences, &map).is_some());
    }
}
