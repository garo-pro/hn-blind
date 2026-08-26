//! Where the user's edited templates live between runs.
//!
//! The file holds only what the user has actually changed. Anything untouched falls back to the compiled-in default, so improvements to the wording of a default announcement still reach people who once opened the settings dialog and changed something else.
//!
//! A missing, unreadable or corrupt file is never fatal: the application starts with its defaults and says why, because being unable to talk about a broken settings file is worse than the broken settings file.

use std::path::PathBuf;

use serde_json::{Map, Value};

use crate::templates::{Template, Templates};

/// Directory and file name under the platform's per-user configuration root.
const DIR: &str = "hn-blind";
const FILE: &str = "templates.json";

/// The platform's per-user directory for this application's files, or `None` if the platform gave us no home to put one in. Shared with `preferences.rs`, which keeps its own file alongside `templates.json` here.
pub fn config_dir() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|home| {
            PathBuf::from(home)
                .join("Library")
                .join("Application Support")
        })
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
    }?;

    Some(base.join(DIR))
}

/// The settings file's path, or `None` if the platform gave us no home to put it in.
pub fn path() -> Option<PathBuf> {
    Some(config_dir()?.join(FILE))
}

/// Load the user's templates, falling back to defaults for anything missing.
///
/// Returns the templates and, when something was wrong with the file, a sentence about it fit to be spoken as the first status message.
pub fn load() -> (Templates, Option<String>) {
    let mut templates = Templates::default();

    let Some(path) = path() else {
        return (templates, None);
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        // No file yet is the normal case on a first run, not a problem.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return (templates, None),
        Err(err) => return (templates, Some(format!("Could not read settings: {err}"))),
    };

    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(map)) => {
            let unknown = apply(&mut templates, &map);
            let note = (!unknown.is_empty()).then(|| {
                format!(
                    "Settings file has {} entry that is not a template",
                    unknown.len()
                )
            });
            (templates, note)
        }
        Ok(_) => (
            templates,
            Some("Settings file is not a set of templates; using defaults".to_string()),
        ),
        Err(err) => (
            templates,
            Some(format!("Could not read settings: {err}; using defaults")),
        ),
    }
}

/// Apply the file's entries, returning the ids that matched no template.
///
/// Unknown ids are reported but kept out of the way rather than treated as an error: they are what a settings file written by a newer version looks like.
fn apply(templates: &mut Templates, map: &Map<String, Value>) -> Vec<String> {
    let mut unknown = Vec::new();
    for (id, value) in map {
        let Some(text) = value.as_str() else {
            unknown.push(id.clone());
            continue;
        };
        match Template::ALL.iter().find(|t| t.id() == id) {
            Some(template) => templates.set(*template, text),
            None => unknown.push(id.clone()),
        }
    }
    unknown
}

/// Write the user's edits, creating the directory if need be.
pub fn save(templates: &Templates) -> Result<PathBuf, String> {
    let path = path().ok_or_else(|| "no configuration directory on this system".to_string())?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("{}: {err}", parent.display()))?;
    }

    let map: Map<String, Value> = templates
        .changed()
        .map(|(template, text)| (template.id().to_string(), Value::String(text.to_string())))
        .collect();

    let mut text = serde_json::to_string_pretty(&Value::Object(map))
        .map_err(|err| format!("could not encode settings: {err}"))?;
    text.push('\n');

    std::fs::write(&path, text).map_err(|err| format!("{}: {err}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_entries_are_applied_and_unknown_ones_reported() {
        let mut templates = Templates::default();
        let map: Map<String, Value> = serde_json::from_str(
            r#"{ "story_row": "{title}", "from_the_future": "x", "wrong_type": 7 }"#,
        )
        .unwrap();

        let unknown = apply(&mut templates, &map);

        assert_eq!(templates.get(Template::StoryRow), "{title}");
        assert_eq!(
            templates.get(Template::CommentRow),
            Template::CommentRow.default_text(),
            "untouched templates keep the default"
        );
        assert_eq!(unknown.len(), 2);
    }

    #[test]
    fn the_settings_file_holds_only_what_changed() {
        let mut templates = Templates::default();
        templates.set(Template::HelpRow, "{keys}");

        let map: Map<String, Value> = templates
            .changed()
            .map(|(t, text)| (t.id().to_string(), Value::String(text.to_string())))
            .collect();
        assert_eq!(map.len(), 1);
        assert_eq!(map["help_row"], Value::String("{keys}".to_string()));

        // And it round-trips.
        let mut loaded = Templates::default();
        assert!(apply(&mut loaded, &map).is_empty());
        assert_eq!(loaded, templates);
    }
}
