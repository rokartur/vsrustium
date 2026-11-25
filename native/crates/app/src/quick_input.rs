//! The quick input model: platform/quickinput/browser/quickInput.ts (`QuickPick`, `InputBox`) and the list
//! state of quickInputList.ts (filtering, focus, selection, checkboxes). The view draws it
//! (workbench/quick_input_view.rs); MainThreadQuickOpen and the workbench pickers drive it.
//!
//! Events go out in `events`, in the order upstream fires them; the owner drains them after each call.

use std::path::{Path, PathBuf};

use config::workspace::Workspace;
use serde_json::Value;

use crate::filters::{self, Matches};
use crate::input_box::InputBox;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    QuickPick,
    InputBox,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Event {
    ChangeActive(Vec<i64>),
    ChangeSelection(Vec<i64>),
    /// `onDidAccept`; true when accepted in the background, the picker staying open.
    Accept(bool),
    ChangeValue(String),
    Hide,
    /// A title bar button by handle (-1 is the back button), with the new state of a toggle.
    TriggerButton(i64, Option<bool>),
    /// An item's button: the item handle and the button's index.
    TriggerItemButton(i64, i64, Option<bool>),
}

/// A `TransferQuickInputButton`; its icon is a codicon id or an image file.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Button {
    pub handle: i64,
    pub codicon: Option<String>,
    pub icon_path: Option<IconPath>,
    pub tooltip: Option<String>,
    pub toggle: Option<bool>,
    /// `QuickInputButtonLocation`: 2 next to the input box, 3 inside it, else the title bar.
    pub location: Option<u64>,
    /// `alwaysVisible`: an item button shown on rows that are neither focused nor hovered too.
    pub always_visible: bool,
}

/// `QuickPickFocus`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus {
    First,
    Second,
    Last,
    Next,
    Previous,
    NextPage,
    PreviousPage,
    NextSeparator,
    PreviousSeparator,
}

/// `ItemActivation`: which item the list focuses when items are set.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ItemActivation {
    // Set by goto symbol and the simple file dialog upstream, neither ported yet; the #207832 test covers it.
    #[cfg_attr(not(test), allow(dead_code))]
    None,
    #[default]
    First,
}

/// `Severity` of a validation message.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Severity {
    #[default]
    Ignore,
    Info,
    Warning,
    Error,
}

#[derive(Clone, PartialEq, Debug, Default)]
pub struct Item {
    pub handle: i64,
    pub label: String,
    pub description: Option<String>,
    pub detail: Option<String>,
    pub always_show: bool,
    /// `pickable !== false`.
    pub pickable: bool,
    pub picked: bool,
    /// The codicon of `iconPathDto` when it is a theme icon (`expandIconPath`).
    pub icon: Option<String>,
    pub icon_path: Option<IconPath>,
    /// `iconClasses` of a file (`getIconClasses(resource)`): the file icon theme draws its icon.
    pub file_icon: Option<PathBuf>,
    /// `iconClasses` of a language (`getIconClassesForLanguageId`).
    pub language_icon: Option<String>,
    pub buttons: Vec<Button>,
    /// The keybinding shown at the row's end (Command Palette): the key caps of each chord.
    pub keybinding: Vec<Vec<String>>,
    /// `highlights`: matches a picker computed itself, kept while the list does not filter.
    pub label_highlights: Matches,
    pub description_highlights: Matches,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Entry {
    Item(Item),
    /// A separator; unlabelled ones still start a section.
    Separator(Option<String>),
}

/// A `QuickPickItemElement`: an item with its list state.
#[derive(Clone, Debug)]
pub struct Row {
    pub item: Item,
    /// The section the separators of the input put the item in, and that section's label.
    section: usize,
    section_label: Option<String>,
    /// The separator drawn above the row, on the first visible row of a section; its label may be empty.
    pub separator: Option<String>,
    pub hidden: bool,
    pub label_highlights: Matches,
    pub description_highlights: Matches,
    pub detail_highlights: Matches,
    pub checked: bool,
}

/// An item of one `items` assignment; a new assignment makes new items, as new objects upstream.
type ItemId = (u64, usize);

#[derive(Clone)]
pub struct QuickInput {
    pub id: i64,
    pub kind: Kind,
    pub visible: bool,
    pub title: Option<String>,
    pub description: Option<String>,
    pub step: Option<u64>,
    pub total_steps: Option<u64>,
    pub enabled: bool,
    pub busy: bool,
    pub ignore_focus_out: bool,
    pub placeholder: Option<String>,
    pub prompt: Option<String>,
    pub password: bool,
    pub validation_message: Option<String>,
    pub severity: Severity,
    /// The value as the input element shows it, with its selection now.
    pub input_box: InputBox,
    /// `valueSelection` as char offsets; `None` selects all of the value when the input shows.
    pub value_selection: Option<(usize, usize)>,
    pub can_select_many: bool,
    pub can_accept_in_background: bool,
    pub match_on_label: bool,
    pub match_on_description: bool,
    pub match_on_detail: bool,
    pub match_on_label_contiguous: bool,
    pub sort_by_label: bool,
    pub hide_input: bool,
    pub item_activation: ItemActivation,
    pub buttons: Vec<Button>,
    items: Vec<Entry>,
    generation: u64,
    /// The list's elements and the order it shows the visible ones in.
    pub rows: Vec<Row>,
    pub order: Vec<usize>,
    pub focused: Option<usize>,
    list_selection: Vec<usize>,
    should_loop: bool,
    checkboxes_shown: bool,
    active_items: Vec<ItemId>,
    selected_items: Vec<ItemId>,
    items_updated: bool,
    active_items_updated: bool,
    selected_items_updated: bool,
    confirming_active: bool,
    confirming_selection: bool,
    pub events: Vec<Event>,
}

impl QuickInput {
    pub fn new(id: i64, kind: Kind) -> QuickInput {
        QuickInput {
            id,
            kind,
            visible: false,
            title: None,
            description: None,
            step: None,
            total_steps: None,
            enabled: true,
            busy: false,
            ignore_focus_out: false,
            placeholder: None,
            prompt: None,
            password: false,
            validation_message: None,
            severity: Severity::Ignore,
            input_box: InputBox::default(),
            value_selection: None,
            can_select_many: false,
            can_accept_in_background: false,
            match_on_label: true,
            match_on_description: false,
            match_on_detail: false,
            match_on_label_contiguous: false,
            sort_by_label: true,
            hide_input: false,
            item_activation: ItemActivation::First,
            buttons: Vec::new(),
            items: Vec::new(),
            generation: 0,
            rows: Vec::new(),
            order: Vec::new(),
            focused: None,
            list_selection: Vec::new(),
            should_loop: true,
            checkboxes_shown: false,
            active_items: Vec::new(),
            selected_items: Vec::new(),
            items_updated: false,
            active_items_updated: false,
            selected_items_updated: false,
            confirming_active: false,
            confirming_selection: false,
            events: Vec::new(),
        }
    }

    #[cfg(test)]
    pub fn items(&self) -> &[Entry] {
        &self.items
    }

    pub fn set_items(&mut self, items: Vec<Entry>) {
        self.items = items;
        self.items_updated = true;
        self.update();
    }

    /// `TriggerAction.REMOVE_ITEM` of pickerQuickAccess.ts: the items without it, the other active items kept.
    pub fn remove_item(&mut self, handle: i64) {
        let active: Vec<i64> = self.active_handles().into_iter().filter(|active| *active != handle).collect();
        self.items.retain(|entry| !matches!(entry, Entry::Item(item) if item.handle == handle));
        self.items_updated = true;
        self.update();
        self.set_active_items(&active);
    }

    /// `activeItems = ...`: handles of the current items; unknown handles drop out.
    pub fn set_active_items(&mut self, handles: &[i64]) {
        self.active_items = self.ids_of_handles(handles);
        self.active_items_updated = true;
        self.update();
    }

    pub fn set_selected_items(&mut self, handles: &[i64]) {
        self.selected_items = self.ids_of_handles(handles);
        self.selected_items_updated = true;
        self.update();
    }

    pub fn active_handles(&self) -> Vec<i64> {
        self.handles(&self.active_items)
    }

    pub fn selected_handles(&self) -> Vec<i64> {
        self.handles(&self.selected_items)
    }

    /// `value = ...` from the API: filters and fires the change back.
    pub fn set_value(&mut self, value: String) {
        if self.input_box.value == value {
            return;
        }
        self.input_box.set_value(value);
        self.update();
        self.value_changed();
    }

    /// An edit in the input element; one that changes the value sets it as `doSetValue` from the UI does.
    pub fn edit_input(&mut self, edit: impl FnOnce(&mut InputBox)) {
        let before = self.input_box.value.clone();
        edit(&mut self.input_box);
        if self.input_box.value != before {
            self.value_changed();
        }
    }

    /// The rest of `doSetValue`: a visible quick pick filters, and the change goes out.
    fn value_changed(&mut self) {
        let value = self.input_box.value.clone();
        if self.visible && self.kind == Kind::QuickPick && self.filter(&value) {
            self.try_select_first();
        }
        self.events.push(Event::ChangeValue(value));
    }

    /// `show()`; the owner hides the input that was visible before.
    pub fn show(&mut self) {
        if self.visible {
            return;
        }
        self.visible = true;
        // The controller empties the list for the input it shows; the items come back when they are set again.
        self.rows.clear();
        self.order.clear();
        self.focused = None;
        let end = self.input_box.len();
        self.input_box.select(self.value_selection.map_or(0, |(start, _)| start), self.value_selection.map_or(end, |(_, stop)| stop));
        self.update();
    }

    /// `valueSelection = ...`: selects the range now when visible, else when the input shows.
    pub fn set_value_selection(&mut self, selection: Option<(usize, usize)>) {
        self.value_selection = selection;
        if self.visible {
            let end = self.input_box.len();
            self.input_box.select(selection.map_or(0, |(start, _)| start), selection.map_or(end, |(_, stop)| stop));
        }
    }

    /// The title bar text: `getTitle()`, the title with its step.
    pub fn title_text(&self) -> String {
        let steps = match (self.step, self.total_steps) {
            (Some(step), Some(total)) => format!("{step}/{total}"),
            (Some(step), None) => step.to_string(),
            _ => String::new(),
        };
        match (self.title.as_deref().filter(|title| !title.is_empty()), steps.is_empty()) {
            (Some(title), false) => format!("{title} ({steps})"),
            (Some(title), true) => title.to_owned(),
            (None, _) => steps,
        }
    }

    /// The message under the input: the validation message, else the prompt; an input box always has one.
    pub fn message(&self) -> Option<String> {
        if let Some(message) = self.validation_message.as_ref().filter(|message| !message.is_empty()) {
            return Some(message.clone());
        }
        let prompt = self.prompt.as_ref().filter(|prompt| !prompt.is_empty());
        match self.kind {
            Kind::QuickPick => prompt.cloned(),
            Kind::InputBox => Some(match prompt {
                Some(prompt) => format!("{prompt} (Press 'Enter' to confirm or 'Escape' to cancel)"),
                None => "Press 'Enter' to confirm your input or 'Escape' to cancel".to_owned(),
            }),
        }
    }

    /// `didHide()`: the owner hid the widget or showed another input in it.
    pub fn did_hide(&mut self) {
        if !self.visible {
            return;
        }
        self.visible = false;
        self.events.push(Event::Hide);
    }

    /// The widget's accept (Enter, `workbench.action.acceptSelectedQuickOpenItem`): the handler `show()` installs.
    pub fn accept_from_ui(&mut self) {
        if self.kind == Kind::QuickPick {
            if self.can_select_many {
                if !self.rows.iter().any(|row| row.checked) {
                    self.selected_items.clear();
                    self.events.push(Event::ChangeSelection(vec![]));
                }
            } else if let Some(active) = self.active_items.first().copied() {
                self.selected_items = vec![active];
                self.events.push(Event::ChangeSelection(self.handles(&self.selected_items)));
            }
        }
        self.events.push(Event::Accept(false));
    }

    /// `accept(inBackground)` of the current input (`quickInput.accept`, `quickInput.acceptInBackground`).
    pub fn accept(&mut self, in_background: bool) {
        if self.kind == Kind::InputBox {
            self.events.push(Event::Accept(in_background));
            return;
        }
        if in_background && !self.can_accept_in_background {
            return;
        }
        if let Some(active) = self.active_items.first().copied().filter(|_| !self.can_select_many) {
            self.selected_items = vec![active];
            self.events.push(Event::ChangeSelection(self.handles(&self.selected_items)));
        }
        self.events.push(Event::Accept(in_background));
    }

    /// `QuickPick.focus(focus)`: moves the list focus.
    pub fn focus(&mut self, what: Focus, page_size: usize) {
        if self.kind != Kind::QuickPick || self.rows.is_empty() {
            return;
        }
        let what = if what == Focus::Second && self.rows.len() < 2 { Focus::First } else { what };
        let position = self.focused.and_then(|row| self.order.iter().position(|shown| *shown == row));
        let last = self.order.len().checked_sub(1);
        let target = match what {
            Focus::First => self.order.first().copied(),
            Focus::Second => self.order.get(1).copied(),
            Focus::Last => self.order.last().copied(),
            Focus::Next => match (position, last) {
                (_, None) => None,
                (None, Some(_)) => Some(self.order[0]),
                (Some(at), Some(last)) if at < last => Some(self.order[at + 1]),
                (Some(_), Some(_)) => self.should_loop.then(|| self.order[0]),
            },
            Focus::Previous => match (position, last) {
                (_, None) => None,
                (None, Some(last)) => Some(self.order[last]),
                (Some(0), Some(last)) => self.should_loop.then(|| self.order[last]),
                (Some(at), Some(_)) => Some(self.order[at - 1]),
            },
            Focus::NextPage => last.map(|last| self.order[position.map_or(page_size.saturating_sub(1), |at| at + page_size.max(1) - 1).min(last)]),
            Focus::PreviousPage => last.map(|_| self.order[position.map_or(0, |at| at.saturating_sub(page_size.max(1) - 1))]),
            Focus::NextSeparator | Focus::PreviousSeparator => self.separator_target(what == Focus::NextSeparator, position),
        };
        if let Some(row) = target {
            self.focused = Some(row);
            self.on_focus_change();
        }
    }

    /// `quickInputService.toggle()`: flips the checkbox of the focused item.
    pub fn toggle(&mut self) {
        if !self.can_select_many {
            return;
        }
        let Some(row) = self.focused else { return };
        let all_checked = self.rows[row].hidden || !self.rows[row].item.pickable || self.rows[row].checked;
        if self.rows[row].item.pickable && self.rows[row].checked != !all_checked {
            self.rows[row].checked = !all_checked;
            self.on_checked_change();
        }
    }

    /// A click on a row: its checkbox with checkboxes, else the list selection.
    pub fn click_row(&mut self, row: usize) {
        if self.can_select_many {
            if self.rows[row].item.pickable {
                self.rows[row].checked = !self.rows[row].checked;
                self.on_checked_change();
            }
            return;
        }
        self.focused = Some(row);
        self.on_focus_change();
        self.list_selection = vec![row];
        self.on_selection_change();
    }

    /// The "check all" checkbox: checks every visible pickable item, or unchecks them when all are checked.
    pub fn toggle_all(&mut self) {
        let visible: Vec<usize> = self.order.iter().copied().filter(|row| self.rows[*row].item.pickable).collect();
        let check = !visible.iter().all(|row| self.rows[*row].checked);
        for row in visible {
            self.rows[row].checked = check;
        }
        self.on_checked_change();
    }

    pub fn checked_count(&self) -> usize {
        self.rows.iter().filter(|row| row.checked).count()
    }

    /// `update()`: pushes the model's pending changes into the list. Owners call it after setting fields.
    pub fn update(&mut self) {
        if !self.visible || self.kind != Kind::QuickPick {
            return;
        }
        if self.items_updated {
            self.items_updated = false;
            let had_focus = self.focused.is_some();
            self.set_elements();
            self.should_loop = !self.can_select_many;
            self.filter(&self.input_box.value.clone());
            // The list buffers its focus events while it is filled and fires only the last one.
            match self.item_activation {
                ItemActivation::None => {
                    self.item_activation = ItemActivation::First;
                    if had_focus {
                        self.on_focus_change();
                    }
                }
                ItemActivation::First if !self.can_select_many => self.try_select_first(),
                ItemActivation::First if had_focus => self.on_focus_change(),
                ItemActivation::First => {}
            }
        }
        if self.checkboxes_shown != self.can_select_many {
            self.checkboxes_shown = self.can_select_many;
            if self.can_select_many {
                self.focused = None;
                self.on_focus_change();
            } else {
                self.try_select_first();
            }
        }
        if self.active_items_updated {
            self.active_items_updated = false;
            self.confirming_active = true;
            let generation = self.generation;
            self.focused = self.active_items.iter().find(|(item_generation, row)| *item_generation == generation && !self.rows[*row].hidden).map(|(_, row)| *row);
            self.on_focus_change();
            self.confirming_active = false;
        }
        if self.selected_items_updated {
            self.selected_items_updated = false;
            self.confirming_selection = true;
            let rows = self.current_rows(&self.selected_items);
            if self.can_select_many {
                let mut changed = false;
                for (index, row) in self.rows.iter_mut().enumerate() {
                    let checked = rows.contains(&index);
                    changed |= row.checked != checked;
                    row.checked = checked;
                }
                if changed {
                    self.on_checked_change();
                }
            } else {
                self.list_selection = rows;
                self.on_selection_change();
            }
            self.confirming_selection = false;
        }
        if self.hide_input && self.can_select_many {
            self.focus(Focus::First, 0);
        }
    }

    fn try_select_first(&mut self) {
        if !self.can_select_many {
            self.focus(Focus::First, 0);
        }
    }

    /// `setElements`: new rows for the items, each with the buttonless separator before it.
    fn set_elements(&mut self) {
        self.generation += 1;
        self.focused = None;
        self.list_selection.clear();
        let mut rows = Vec::new();
        let mut section = 0;
        let mut section_label = None;
        for entry in &self.items {
            match entry {
                Entry::Separator(label) => {
                    section += 1;
                    section_label = label.clone().filter(|label| !label.is_empty());
                }
                Entry::Item(item) => rows.push(Row {
                    item: item.clone(),
                    section,
                    section_label: section_label.clone(),
                    separator: None,
                    hidden: false,
                    label_highlights: item.label_highlights.clone(),
                    description_highlights: item.description_highlights.clone(),
                    detail_highlights: vec![],
                    checked: false,
                }),
            }
        }
        self.rows = rows;
        self.order = (0..self.rows.len()).collect();
        self.place_separators(false);
    }

    /// Puts each section's separator above its first visible row; a label-sorted list shows none.
    fn place_separators(&mut self, sorted: bool) {
        let mut previous_section = None;
        for at in 0..self.order.len() {
            let row = &mut self.rows[self.order[at]];
            row.separator = (!sorted && row.section > 0 && previous_section != Some(row.section)).then(|| row.section_label.clone().unwrap_or_default());
            previous_section = Some(row.section);
        }
    }

    /// `filter(query)`: hides what does not match, highlights what does, sorts by label when asked.
    fn filter(&mut self, query: &str) -> bool {
        if !(self.sort_by_label || self.match_on_label || self.match_on_description || self.match_on_detail) {
            return false;
        }
        let query_with_whitespace = query;
        let query = query.trim();
        if query.is_empty() || !(self.match_on_label || self.match_on_description || self.match_on_detail) {
            for row in &mut self.rows {
                row.label_highlights.clear();
                row.description_highlights.clear();
                row.detail_highlights.clear();
                row.hidden = false;
            }
        } else {
            for row in &mut self.rows {
                let label = filters::parse_label_with_icons(&row.item.label);
                let label_highlights = match (self.match_on_label, self.match_on_label_contiguous) {
                    (false, _) => None,
                    (true, false) => filters::matches_fuzzy_icon_aware(query, &label),
                    (true, true) => filters::matches_contiguous_icon_aware(query_with_whitespace, &label),
                };
                let fuzzy = |enabled: bool, text: &Option<String>| {
                    enabled.then(|| filters::matches_fuzzy_icon_aware(query, &filters::parse_label_with_icons(text.as_deref().unwrap_or_default()))).flatten()
                };
                let description_highlights = fuzzy(self.match_on_description, &row.item.description);
                let detail_highlights = fuzzy(self.match_on_detail, &row.item.detail);
                let matched = label_highlights.is_some() || description_highlights.is_some() || detail_highlights.is_some();
                row.label_highlights = label_highlights.unwrap_or_default();
                row.description_highlights = description_highlights.unwrap_or_default();
                row.detail_highlights = detail_highlights.unwrap_or_default();
                row.hidden = !matched && !row.item.always_show;
            }
        }
        self.order = (0..self.rows.len()).filter(|row| !self.rows[*row].hidden).collect();
        let sorted = self.sort_by_label && !query.is_empty();
        if sorted {
            let look_for = query.to_lowercase();
            let rows = &self.rows;
            self.order.sort_by(|a, b| compare_entries(&rows[*a], &rows[*b], &look_for));
        }
        self.place_separators(sorted);
        if self.focused.is_some_and(|row| self.rows[row].hidden) {
            self.focused = None;
            self.on_focus_change();
        }
        true
    }

    /// `QuickPickFocus.NextSeparator` / `PreviousSeparator`: the first item of the next or current section.
    fn separator_target(&self, next: bool, position: Option<usize>) -> Option<usize> {
        let mut sections = Vec::new();
        let mut current = None;
        for (at, row) in self.order.iter().enumerate() {
            if at == 0 || self.rows[*row].section != self.rows[self.order[at - 1]].section {
                sections.push(at);
            }
            if Some(at) == position {
                current = Some(sections.len() - 1);
            }
        }
        if sections.is_empty() {
            return None;
        }
        let current = current.map_or(-1, |section| section as isize);
        let at_section_start = position.is_some() && current >= 0 && sections[current as usize] == position.unwrap_or_default();
        let delta = if next { 1 } else if at_section_start { -1 } else { 0 };
        let count = sections.len() as isize;
        let section = sections[((current + delta + count) % count) as usize];
        if next && Some(section) == position {
            return self.order.last().copied();
        }
        Some(self.order[section])
    }

    /// The handler of the list's focus changes, installed by `show()`.
    fn on_focus_change(&mut self) {
        if self.active_items_updated {
            return;
        }
        let focused: Vec<ItemId> = self.focused.map(|row| (self.generation, row)).into_iter().collect();
        if !self.confirming_active && focused == self.active_items {
            return;
        }
        self.active_items = focused;
        self.events.push(Event::ChangeActive(self.handles(&self.active_items)));
    }

    /// The handler of the list's selection changes: a single-select pick accepts what was selected.
    fn on_selection_change(&mut self) {
        let selected: Vec<ItemId> = self.list_selection.iter().map(|row| (self.generation, *row)).collect();
        if self.can_select_many && self.list_selection.iter().all(|row| self.rows[*row].item.pickable) {
            self.list_selection.clear();
            return;
        }
        if !self.confirming_selection && selected == self.selected_items {
            return;
        }
        self.selected_items = selected;
        self.events.push(Event::ChangeSelection(self.handles(&self.selected_items)));
        if !self.selected_items.is_empty() {
            self.events.push(Event::Accept(false));
        }
    }

    fn on_checked_change(&mut self) {
        if !self.can_select_many || !self.visible {
            return;
        }
        let checked: Vec<ItemId> = (0..self.rows.len()).filter(|row| self.rows[*row].checked).map(|row| (self.generation, row)).collect();
        if !self.confirming_selection && checked == self.selected_items {
            return;
        }
        self.selected_items = checked;
        self.events.push(Event::ChangeSelection(self.handles(&self.selected_items)));
    }

    fn ids_of_handles(&self, handles: &[i64]) -> Vec<ItemId> {
        // Items the list does not hold yet get the ids the next `setElements` gives them.
        let generation = if self.items_updated { self.generation + 1 } else { self.generation };
        let item_handles: Vec<i64> = self.items.iter().filter_map(|entry| if let Entry::Item(item) = entry { Some(item.handle) } else { None }).collect();
        handles.iter().filter_map(|handle| item_handles.iter().position(|item| item == handle).map(|row| (generation, row))).collect()
    }

    fn current_rows(&self, ids: &[ItemId]) -> Vec<usize> {
        ids.iter().filter(|(generation, _)| *generation == self.generation).map(|(_, row)| *row).collect()
    }

    fn handles(&self, ids: &[ItemId]) -> Vec<i64> {
        ids.iter().filter_map(|(generation, row)| (*generation == self.generation).then(|| self.rows.get(*row).map(|row| row.item.handle)).flatten()).collect()
    }
}

/// `compareEntries`: label matches first, then `compareAnything` on the label without icons.
fn compare_entries(a: &Row, b: &Row, look_for: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a.label_highlights.is_empty(), b.label_highlights.is_empty()) {
        (false, true) => Ordering::Less,
        (true, false) => Ordering::Greater,
        (true, true) => Ordering::Equal,
        (false, false) => {
            let sort_label = |row: &Row| filters::parse_label_with_icons(&row.item.label).text.trim().to_owned();
            filters::compare_anything(&sort_label(a), &sort_label(b), look_for)
        }
    }
}

/// A `TransferQuickPickItemOrSeparator` as the main thread expands it (`expandItemProps`): an item with a
/// `resourceUri` and no label shows the file name, with the folder relative to the workspace as description.
pub fn entry_from_transfer(value: &Value, workspace: Option<&Workspace>) -> Entry {
    if value["type"] == "separator" {
        return Entry::Separator(value["label"].as_str().map(str::to_owned));
    }
    let text = |key: &str| value[key].as_str().map(str::to_owned);
    let mut item = Item {
        handle: value["handle"].as_i64().unwrap_or_default(),
        label: text("label").unwrap_or_default(),
        description: text("description"),
        detail: text("detail"),
        always_show: value["alwaysShow"].as_bool().unwrap_or(false),
        pickable: value["pickable"].as_bool() != Some(false),
        picked: value["picked"].as_bool().unwrap_or(false),
        icon: value["iconPathDto"]["id"].as_str().map(str::to_owned),
        icon_path: icon_file(&value["iconPathDto"]),
        buttons: value["buttons"].as_array().into_iter().flatten().map(button_from_transfer).collect(),
        ..Item::default()
    };
    if let Some(path) = value["resourceUri"]["path"].as_str().filter(|_| value["resourceUri"]["scheme"] == "file") {
        let path = Path::new(path);
        if item.label.is_empty() {
            item.label = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            if item.description.is_none() {
                item.description = Some(path_label(path.parent().unwrap_or(path), workspace));
            }
        } else if item.description.is_none() {
            item.description = Some(path_label(path, workspace));
        }
    }
    Entry::Item(item)
}

/// A `TransferQuickInputButton` with its icon expanded (`expandIconPath`).
pub fn button_from_transfer(value: &Value) -> Button {
    Button {
        handle: value["handle"].as_i64().unwrap_or_default(),
        codicon: value["iconPathDto"]["id"].as_str().map(str::to_owned),
        icon_path: icon_file(&value["iconPathDto"]),
        tooltip: value["tooltip"].as_str().map(str::to_owned),
        toggle: value["toggle"]["checked"].as_bool(),
        location: value["location"].as_u64(),
        always_visible: false,
    }
}

/// The image files of an icon, for light and for dark themes.
#[derive(Clone, PartialEq, Debug)]
pub struct IconPath {
    pub light: std::path::PathBuf,
    pub dark: std::path::PathBuf,
}

/// The image of an `IconPath` DTO: one file URI, or a `{ light, dark }` pair.
fn icon_file(icon: &Value) -> Option<IconPath> {
    let path = |uri: &Value| (uri["scheme"] == "file").then(|| uri["path"].as_str().map(std::path::PathBuf::from)).flatten();
    match path(icon) {
        Some(file) => Some(IconPath { light: file.clone(), dark: file }),
        None => Some(IconPath { light: path(&icon["light"])?, dark: path(&icon["dark"])? }),
    }
}

/// `labelService.getUriLabel(uri, { relative: true })` for a file: relative to its workspace folder, after the
/// folder's name when there are several, else the absolute path with the home folder as `~`.
pub fn path_label(path: &Path, workspace: Option<&Workspace>) -> String {
    if let Some(workspace) = workspace
        && let Some(folder) = workspace.folder_of(path)
    {
        let relative = path.iter().skip(folder.path.iter().count()).collect::<PathBuf>().to_string_lossy().into_owned();
        return match (workspace.folders.len() > 1, relative.is_empty()) {
            (false, _) => relative,
            (true, true) => folder.name.clone(),
            (true, false) => format!("{} \u{2022} {relative}", folder.name),
        };
    }
    if let Some(home) = std::env::var_os("HOME") {
        if let Ok(relative) = path.strip_prefix(&home) {
            return format!("~/{}", relative.to_string_lossy());
        }
    }
    path.to_string_lossy().into_owned()
}

/// `labelService.getUriLabel(uri, { relative, noPrefix: true, separator })` for a file: like `path_label` but
/// without the folder name and the `~`, each `/` written as `separator`.
pub fn path_label_without_prefix(path: &Path, workspace: Option<&Workspace>, relative: bool, separator: &str) -> String {
    let label = match workspace.filter(|_| relative).and_then(|workspace| workspace.folder_of(path)) {
        Some(folder) => path.iter().skip(folder.path.iter().count()).collect::<PathBuf>().to_string_lossy().into_owned(),
        None => path.to_string_lossy().into_owned(),
    };
    label.replace('/', separator)
}

/// Where a dragged quick input lands (`QuickInputDragAndDropController._layout`): `top` and `left` of the widget
/// kept inside the window and snapped within 20px to `top_snap`, the 25% line and the horizontal center.
/// Returns the view state: top as a fraction of the window height, the widget's center as a fraction of its width.
pub fn drag_layout(mut top: f32, mut left: f32, window: (f32, f32), widget: (f32, f32), title_bar_height: f32, top_snap: f32) -> (f32, f32) {
    const SNAP: f32 = 20.;
    let (window_width, window_height) = window;
    let (widget_width, widget_height) = widget;
    let center_y = (window_height * 0.25).round();
    let center_x = (window_width / 2.).round() - (widget_width / 2.).round();
    top = top.min(window_height - widget_height).max(0.);
    // Custom window controls on macOS: keep clear of the traffic lights while over the title bar.
    if top < title_bar_height {
        left = left.max(80.);
    }
    if (top - top_snap).abs() < SNAP {
        top = top_snap;
    }
    if (top - center_y).abs() < SNAP {
        top = center_y;
    }
    left = left.min(window_width - widget_width).max(0.);
    if (left - center_x).abs() < SNAP {
        left = center_x;
    }
    (top / window_height, (left + widget_width / 2.) / window_width)
}

#[cfg(test)]
mod tests;
