//! The settings dialog's data: its tabs, its fields, and their grouping.
//!
//! The dialog itself is a real `wxDialog` (built in `main.rs`) with a `Notebook` of tabs, and each tab a `TreeCtrl` (one parent node per [`Group`], the group's fields as children) next to a single `TextCtrl` — or, for the one non-text field, a `CheckBox` — that edits whichever field is currently selected in the tree. Typing, the caret, and backspace/delete are the native `TextCtrl`'s job now; what stays here is knowing which fields exist, which tab and group they belong to, and which one is currently selected, so `app.rs` can turn that into wording and `main.rs` can turn it into tree nodes.

use crate::templates::{Group, Template, Templates};

/// A tab in the dialog.
pub struct Tab {
    pub name: &'static str,
}

pub const TABS: &[Tab] = &[Tab { name: "Templates" }, Tab { name: "General" }];

/// A control in the settings dialog: a template's editable text, or a checkbox for a preference that has no text at all.
///
/// Only one checkbox exists today, so it is named directly rather than wrapped in a generic "toggle id" — that generality can be added the day a second one shows up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Template(Template),
    /// Whether Escape, pressed at the story list, quits the application.
    EscapeExits,
}

impl Field {
    pub fn group(self) -> Group {
        match self {
            Field::Template(template) => template.group(),
            Field::EscapeExits => Group::General,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Field::Template(template) => template.label(),
            Field::EscapeExits => "Escape quits the application from the story list",
        }
    }
}

pub struct Settings {
    tab: usize,
    /// Index into the active tab's `fields()`, or `None` while the dialog has just opened and nothing has been picked in its field tree yet.
    selected: Option<usize>,
}

impl Default for Settings {
    fn default() -> Self {
        Self::new()
    }
}

impl Settings {
    pub fn new() -> Self {
        Settings {
            tab: 0,
            selected: None,
        }
    }

    /// Reset to the state the dialog should open in.
    pub fn open(&mut self) {
        self.tab = 0;
        self.selected = None;
    }

    pub fn tab(&self) -> usize {
        self.tab
    }

    pub fn tab_name(&self) -> &'static str {
        TABS[self.tab].name
    }

    /// The controls of the active tab.
    pub fn fields(&self) -> Vec<Field> {
        fields_of(self.tab)
    }

    pub fn focused_field(&self) -> Option<Field> {
        self.fields().get(self.selected?).copied()
    }

    /// Whether the selected control is a checkbox, which is toggled rather than typed into.
    pub fn is_toggle(&self) -> bool {
        matches!(self.focused_field(), Some(Field::EscapeExits))
    }


    /// Select a field of the active tab, in response to the tree control's own selection-changed event.
    pub fn select_field(&mut self, index: usize) -> bool {
        if index >= self.fields().len() || self.selected == Some(index) {
            return false;
        }
        self.selected = Some(index);
        true
    }

    /// Switch tabs, in response to the notebook's own page-changed event.
    pub fn select_tab(&mut self, index: usize) -> bool {
        if index >= TABS.len() || index == self.tab {
            return false;
        }
        self.tab = index;
        self.selected = None;
        true
    }

    /// Restore the selected field to its compiled-in default. A no-op on the checkbox, which has no text to restore.
    pub fn reset_field(&mut self, templates: &mut Templates) -> Option<Template> {
        let Field::Template(template) = self.focused_field()? else {
            return None;
        };
        templates.reset(template);
        Some(template)
    }
}

/// The controls that belong to a tab, independent of any dialog state — used both by `Settings::fields` and directly by `main.rs` when a tab other than the currently selected one needs to be populated.
pub fn fields_of(tab: usize) -> Vec<Field> {
    match tab {
        0 => Template::ALL.iter().copied().map(Field::Template).collect(),
        _ => vec![Field::EscapeExits],
    }
}

/// The same fields, cut into their groups: each entry is a group and the stretch of `fields` that belongs to it.
///
/// Fields are declared in group order, so this is a scan rather than a sort. A screen reader announces entering and leaving a group, which is what makes seventy fields a list a person can move through rather than a wall — so the grouping is a real part of the interface, not decoration.
pub fn groups(fields: &[Field]) -> Vec<(Group, std::ops::Range<usize>)> {
    let mut groups: Vec<(Group, std::ops::Range<usize>)> = Vec::new();
    for (index, field) in fields.iter().enumerate() {
        match groups.last_mut() {
            Some((group, range)) if *group == field.group() => range.end = index + 1,
            _ => groups.push((field.group(), index..index + 1)),
        }
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_selected_when_the_dialog_opens() {
        let settings = Settings::new();
        assert_eq!(settings.focused_field(), None);
    }

    #[test]
    fn selecting_a_field_updates_focus() {
        let mut settings = Settings::new();
        assert!(settings.select_field(2));
        assert_eq!(settings.focused_field(), Some(Field::Template(Template::ALL[2])));
        assert!(!settings.select_field(2), "already selected");
    }

    #[test]
    fn switching_tabs_clears_the_selection_and_changes_the_field_set() {
        let mut settings = Settings::new();
        settings.select_field(0);
        assert!(settings.select_tab(1));
        assert_eq!(settings.focused_field(), None);
        assert_eq!(settings.fields(), vec![Field::EscapeExits]);
        assert!(!settings.select_tab(1), "already on that tab");
        assert!(!settings.select_tab(TABS.len()), "out of range");
    }

    #[test]
    fn groups_cover_the_field_list_in_order_without_gaps() {
        let fields = fields_of(0);
        let groups = groups(&fields);
        assert!(groups.len() > 1, "the fields fall into several groups");
        assert_eq!(groups[0].1.start, 0);
        assert_eq!(groups.last().unwrap().1.end, fields.len());
        for pair in groups.windows(2) {
            assert_eq!(pair[0].1.end, pair[1].1.start, "no field belongs to no group");
            assert_ne!(pair[0].0, pair[1].0, "a group is not split in two");
        }
    }

    #[test]
    fn the_general_tab_holds_the_checkbox() {
        let mut settings = Settings::new();
        settings.select_tab(1);
        settings.select_field(0);
        assert!(settings.is_toggle());
    }

    #[test]
    fn a_field_can_be_restored_to_its_default() {
        let mut settings = Settings::new();
        let mut templates = Templates::default();
        let field = Template::ALL[0];
        templates.set(field, "mangled");
        settings.select_field(0);

        assert_eq!(settings.reset_field(&mut templates), Some(field));
        assert!(templates.is_default(field));
    }
}
