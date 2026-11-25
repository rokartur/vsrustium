//! The list model of platform/actionWidget/browser/actionList.ts (`ActionListWidget` and `ActionList`): rows, keyboard
//! focus, the text filter, collapsible sections and the height and width the widget lays out with. The view is
//! `workbench/action_widget.rs`.

use std::collections::HashSet;

#[cfg(test)]
mod tests;

/// `ActionListItemKind`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ItemKind {
    Action,
    Header,
    Separator,
}

/// The `IActionListItem` fields the list logic reads, with the `id` and `checked` that upstream reads from the item
/// itself; the row's icon, keybinding and tooltip come with the view.
#[derive(Clone, Debug)]
pub struct Item<T> {
    pub item: Option<T>,
    pub kind: ItemKind,
    pub id: Option<String>,
    pub checked: bool,
    pub disabled: bool,
    pub focusable_when_disabled: bool,
    pub label: Option<String>,
    pub description: Option<String>,
    pub detail: Option<String>,
    pub filter_text: Option<String>,
    pub section: Option<String>,
    pub is_section_toggle: bool,
    pub show_always: bool,
}

impl<T> Item<T> {
    pub fn new(kind: ItemKind) -> Self {
        Item {
            item: None,
            kind,
            id: None,
            checked: false,
            disabled: false,
            focusable_when_disabled: false,
            label: None,
            description: None,
            detail: None,
            filter_text: None,
            section: None,
            is_section_toggle: false,
            show_always: false,
        }
    }

    pub fn action(item: T, label: impl Into<String>) -> Self {
        Item { item: Some(item), label: Some(label.into()), ..Item::new(ItemKind::Action) }
    }

    pub fn separator(label: Option<&str>) -> Self {
        Item { label: label.map(str::to_owned), ..Item::new(ItemKind::Separator) }
    }

    /// `focusCondition`.
    pub fn focusable(&self) -> bool {
        self.kind == ItemKind::Action && (!self.disabled || self.focusable_when_disabled)
    }

    fn matches(&self, filter_lower: &str) -> bool {
        let contains = |text: &Option<String>| text.as_deref().unwrap_or("").to_lowercase().contains(filter_lower);
        contains(&self.label) || contains(&self.description) || contains(&self.filter_text)
    }
}

/// The `IActionListOptions` the port's callers use.
#[derive(Clone, Default)]
pub struct Options {
    pub show_filter: bool,
    pub focus_filter_on_open: bool,
    pub collapsed_by_default: HashSet<String>,
    pub min_width: Option<f32>,
    pub max_width: Option<f32>,
    pub max_visible_items: Option<usize>,
    pub initial_focus_item_id: Option<String>,
}

pub const ACTION_LINE_HEIGHT: f32 = 24.;
pub const HEADER_LINE_HEIGHT: f32 = 24.;
pub const SEPARATOR_LINE_HEIGHT: f32 = 8.;
const DETAIL_ITEM_HEIGHT: f32 = 48.;
/// The filter row of `.action-list-filter`.
pub const FILTER_HEIGHT: f32 = 36.;

pub struct ActionList<T> {
    pub all: Vec<Item<T>>,
    /// Indexes into `all` of the rows shown, in order.
    pub visible: Vec<usize>,
    /// The focused row, an index into `visible`.
    pub focused: Option<usize>,
    pub options: Options,
    pub filter: String,
    /// The filter input has the keyboard focus rather than the list.
    pub filter_focused: bool,
    collapsed: HashSet<String>,
    initial_focus_item_id: Option<String>,
}

/// What `accept_selected` asks the owner to do.
#[derive(Debug, PartialEq)]
pub enum Accepted<T> {
    Select { item: T, preview: bool },
    Nothing,
}

impl<T: Clone> ActionList<T> {
    pub fn new(items: Vec<Item<T>>, options: Options) -> Self {
        let mut list = ActionList {
            all: items,
            visible: Vec::new(),
            focused: None,
            collapsed: options.collapsed_by_default.clone(),
            initial_focus_item_id: options.initial_focus_item_id.clone(),
            options,
            filter: String::new(),
            filter_focused: false,
        };
        list.apply_filter(None);
        list
    }

    pub fn visible_item(&self, row: usize) -> &Item<T> {
        &self.all[self.visible[row]]
    }

    pub fn focused_item(&self) -> Option<&Item<T>> {
        self.focused.map(|row| self.visible_item(row))
    }

    /// `focus`: the filter when it takes focus on open, else the list, with the checked or first row focused.
    pub fn focus(&mut self) {
        self.filter_focused = self.options.show_filter && self.options.focus_filter_on_open;
        self.focus_checked_or_first();
    }

    /// `_focusCheckedOrFirst`: the configured initial row once, else the focused row if focusable, the checked row,
    /// or the first focusable row.
    fn focus_checked_or_first(&mut self) {
        if let Some(id) = self.initial_focus_item_id.take()
            && let Some(row) = self.rows().find(|&row| self.visible_item(row).focusable() && self.visible_item(row).id.as_deref() == Some(&id))
        {
            self.focused = Some(row);
            return;
        }
        if self.focused_item().is_some_and(Item::focusable) {
            return;
        }
        if let Some(row) = self.rows().find(|&row| self.visible_item(row).focusable() && self.visible_item(row).checked) {
            self.focused = Some(row);
            return;
        }
        self.focused = self.find_next(0, false);
    }

    fn rows(&self) -> std::ops::Range<usize> {
        0..self.visible.len()
    }

    /// listWidget.ts `findNextIndex` over focusable rows.
    fn find_next(&self, mut index: usize, wrap: bool) -> Option<usize> {
        let length = self.visible.len();
        for _ in 0..length {
            if index >= length && !wrap {
                return None;
            }
            index %= length;
            if self.visible_item(index).focusable() {
                return Some(index);
            }
            index += 1;
        }
        None
    }

    /// listWidget.ts `findPreviousIndex` over focusable rows.
    fn find_previous(&self, mut index: isize, wrap: bool) -> Option<usize> {
        let length = self.visible.len() as isize;
        for _ in 0..length {
            if index < 0 && !wrap {
                return None;
            }
            index = index.rem_euclid(length);
            if self.visible_item(index as usize).focusable() {
                return Some(index as usize);
            }
            index -= 1;
        }
        None
    }

    /// `focusNext`: from the filter the first row (or the one after the highlighted row); wrapping past the last row
    /// moves to the filter when there is one.
    pub fn focus_next(&mut self) {
        if self.filter_focused {
            self.filter_focused = false;
            let next = match self.focused {
                Some(row) => self.find_next(row + 1, false),
                None => self.find_next(0, false),
            };
            if next.is_some() {
                self.focused = next;
            }
            return;
        }
        let previous = self.focused;
        let Some(next) = self.find_next(previous.map_or(0, |row| row + 1), true) else { return };
        if self.options.show_filter && previous.is_some_and(|previous| next < previous) {
            self.focused = None;
            self.filter_focused = true;
            return;
        }
        self.focused = Some(next);
    }

    /// `focusPrevious`, the mirror of `focus_next`.
    pub fn focus_previous(&mut self) {
        if self.filter_focused {
            self.filter_focused = false;
            match self.focused {
                // Already at the first row: back to the filter.
                Some(current) => match self.find_previous(current as isize - 1, false) {
                    Some(previous) => self.focused = Some(previous),
                    None => self.filter_focused = true,
                },
                None => self.focused = self.find_previous(self.visible.len() as isize - 1, false),
            }
            return;
        }
        let current = self.focused;
        let Some(previous) = self.find_previous(current.map_or(0, |row| row as isize - 1), true) else { return };
        if self.options.show_filter && current.is_some_and(|current| previous > current) {
            self.focused = None;
            self.filter_focused = true;
            return;
        }
        self.focused = Some(previous);
    }

    /// `acceptSelected` and `onListSelection`: a section toggle row toggles its section, a disabled row or a header
    /// does nothing, an action row is selected.
    pub fn accept_selected(&mut self, preview: bool, supports_preview: bool) -> Accepted<T> {
        let Some(row) = self.focused else { return Accepted::Nothing };
        self.accept_row(row, preview, supports_preview)
    }

    /// A click on a row runs the same selection as Enter.
    pub fn accept_row(&mut self, row: usize, preview: bool, supports_preview: bool) -> Accepted<T> {
        let element = self.visible_item(row).clone();
        if element.disabled || element.kind != ItemKind::Action {
            return Accepted::Nothing;
        }
        if element.is_section_toggle
            && let Some(section) = element.section
        {
            self.toggle_section(&section);
            return Accepted::Nothing;
        }
        match element.item {
            Some(item) => Accepted::Select { item, preview: preview && supports_preview },
            None => Accepted::Nothing,
        }
    }

    fn toggle_section(&mut self, section: &str) {
        if !self.collapsed.remove(section) {
            self.collapsed.insert(section.to_owned());
            if let Some(focused) = self.focused_item()
                && focused.section.as_deref() == Some(section)
                && !focused.is_section_toggle
            {
                self.focused = self.rows().find(|&row| {
                    let item = self.visible_item(row);
                    item.section.as_deref() == Some(section) && item.is_section_toggle
                });
            }
        }
        self.apply_filter(None);
    }

    /// The focused row's section, when that section has a toggle that can expand it again.
    fn focused_section(&self) -> Option<String> {
        let section = self.focused_item()?.section.clone()?;
        self.all.iter().any(|item| item.section.as_ref() == Some(&section) && item.is_section_toggle).then_some(section)
    }

    pub fn collapse_focused_section(&mut self) -> bool {
        match self.focused_section() {
            Some(section) if !self.collapsed.contains(&section) => {
                self.toggle_section(&section);
                true
            }
            _ => false,
        }
    }

    pub fn expand_focused_section(&mut self) -> bool {
        match self.focused_section() {
            Some(section) if self.collapsed.contains(&section) => {
                self.toggle_section(&section);
                true
            }
            _ => false,
        }
    }

    pub fn toggle_focused_section(&mut self) -> bool {
        let Some(item) = self.focused_item() else { return false };
        match (item.is_section_toggle, item.section.clone()) {
            (true, Some(section)) => {
                self.toggle_section(&section);
                true
            }
            _ => false,
        }
    }

    /// The filter input's text changed.
    pub fn set_filter(&mut self, value: String) {
        self.filter = value;
        self.apply_filter(None);
    }

    /// `clearFilter`: whether there was a filter to clear.
    pub fn clear_filter(&mut self) -> bool {
        if !self.options.show_filter || self.filter.is_empty() {
            return false;
        }
        self.set_filter(String::new());
        true
    }

    /// `updateItems`: new rows in place, keeping the filter and the focused row by id.
    pub fn update_items(&mut self, items: Vec<Item<T>>, focus_item_id: Option<&str>) {
        let focused_id = focus_item_id.map(str::to_owned).or_else(|| self.focused_item().and_then(|item| item.id.clone()));
        self.all = items;
        self.focused = None;
        self.apply_filter(focused_id.as_deref());
        if self.focused.is_none() {
            self.focus_checked_or_first();
        }
    }

    /// `_applyFilter`: the rows matching the filter (headers dropped, a titled separator kept above its matches),
    /// or without a filter every row outside the collapsed sections; then separators with no action after them
    /// in their section, and unlabeled separators before the first action, go.
    fn apply_filter(&mut self, focus_item_id: Option<&str>) {
        let focused_id = focus_item_id.map(str::to_owned).or_else(|| self.focused_item().and_then(|item| item.id.clone()));
        let filter = self.filter.to_lowercase();
        let mut visible = Vec::new();
        if !filter.is_empty() {
            let mut pending_separator = None;
            let mut section_rows = Vec::new();
            let mut section_matched = false;
            let flush = |visible: &mut Vec<usize>, pending: &mut Option<usize>, rows: &mut Vec<usize>, matched: &mut bool| {
                if let Some(separator) = pending.take().filter(|_| *matched) {
                    visible.push(separator);
                }
                visible.append(rows);
                *matched = false;
            };
            for (index, item) in self.all.iter().enumerate() {
                match item.kind {
                    ItemKind::Header => continue,
                    ItemKind::Separator => {
                        flush(&mut visible, &mut pending_separator, &mut section_rows, &mut section_matched);
                        pending_separator = item.label.is_some().then_some(index);
                        continue;
                    }
                    ItemKind::Action => {}
                }
                if item.show_always {
                    section_rows.push(index);
                } else if item.is_section_toggle {
                    continue;
                } else if item.matches(&filter) {
                    section_matched = true;
                    section_rows.push(index);
                }
            }
            flush(&mut visible, &mut pending_separator, &mut section_rows, &mut section_matched);
        } else {
            for (index, item) in self.all.iter().enumerate() {
                let collapsed = item.section.as_ref().is_some_and(|section| self.collapsed.contains(section));
                if item.kind == ItemKind::Header || item.is_section_toggle || !collapsed {
                    visible.push(index);
                }
            }
        }
        self.visible = self.drop_orphaned_separators(visible);
        self.focused = focused_id.and_then(|id| self.rows().find(|&row| self.visible_item(row).id.as_deref() == Some(&id)));
        if self.filter_focused && self.focused.is_none() {
            self.focus_checked_or_first();
        }
    }

    fn drop_orphaned_separators(&self, mut visible: Vec<usize>) -> Vec<usize> {
        let kind = |visible: &[usize], i: usize| self.all[visible[i]].kind;
        let mut action_before = vec![false; visible.len()];
        let mut seen = false;
        for (i, before) in action_before.iter_mut().enumerate() {
            *before = seen;
            seen |= kind(&visible, i) == ItemKind::Action;
        }
        let mut action_after = vec![false; visible.len()];
        let mut seen_in_section = false;
        for i in (0..visible.len()).rev() {
            match kind(&visible, i) {
                ItemKind::Action => seen_in_section = true,
                ItemKind::Separator => {
                    action_after[i] = seen_in_section;
                    seen_in_section = false;
                }
                ItemKind::Header => {}
            }
        }
        for i in (0..visible.len()).rev() {
            if kind(&visible, i) != ItemKind::Separator {
                continue;
            }
            let leading_unlabeled = self.all[visible[i]].label.is_none() && !action_before[i];
            if !action_after[i] || leading_unlabeled {
                visible.remove(i);
            }
        }
        visible
    }

    /// `_itemHeightWith`.
    pub fn item_height(item: &Item<T>) -> f32 {
        match item.kind {
            ItemKind::Header => HEADER_LINE_HEIGHT,
            ItemKind::Separator if item.detail.is_some() => DETAIL_ITEM_HEIGHT,
            ItemKind::Separator if item.label.is_some() => ACTION_LINE_HEIGHT,
            ItemKind::Separator => SEPARATOR_LINE_HEIGHT,
            ItemKind::Action if item.detail.is_some() => DETAIL_ITEM_HEIGHT,
            ItemKind::Action => ACTION_LINE_HEIGHT,
        }
    }

    /// `computeListHeight`: the shown rows.
    pub fn list_height(&self) -> f32 {
        self.visible.iter().map(|&index| Self::item_height(&self.all[index])).sum()
    }

    /// `computeHeightForItems` of every row outside a collapsed section.
    fn full_height(&self) -> f32 {
        self.all.iter().filter(|item| item.is_section_toggle || !item.section.as_ref().is_some_and(|section| self.collapsed.contains(section))).map(Self::item_height).sum()
    }

    /// `computeMaxVisibleItemsHeightFor`: the rows through the `maxVisibleItems`th action.
    fn max_visible_items_height<'a>(&self, items: impl Iterator<Item = &'a Item<T>>) -> f32
    where
        T: 'a,
    {
        let Some(max) = self.options.max_visible_items else { return f32::INFINITY };
        let mut height = 0.;
        let mut actions = 0;
        for item in items {
            if actions >= max {
                break;
            }
            height += Self::item_height(item);
            if item.kind == ItemKind::Action {
                actions += 1;
            }
        }
        height
    }

    /// `hasDynamicHeight`: a filter or collapsible sections change the rows while the widget shows.
    pub fn has_dynamic_height(&self) -> bool {
        self.options.show_filter || self.all.iter().any(|item| item.is_section_toggle)
    }

    /// `ActionList.computeHeight`: the list's height under the anchor (or above it, when the rows fit better
    /// there), capped at 60% of the window. `chrome` is the widget's padding and border; `show_above` is decided
    /// once and kept while filtering. Static lists fill the space below the widget's top.
    pub fn compute_height(&self, anchor_top: f32, anchor_height: f32, viewport_height: f32, chrome: f32, show_above: &mut Option<bool>) -> f32 {
        let visible_items = self.visible.iter().map(|&index| &self.all[index]);
        let list_height = self.list_height().min(self.max_visible_items_height(visible_items));
        let filter_height = if self.options.show_filter { FILTER_HEIGHT } else { 0. };
        let available = if self.has_dynamic_height() {
            let space_below = viewport_height - anchor_top - anchor_height - 30.;
            let space_above = anchor_top;
            let above = *show_above.get_or_insert_with(|| {
                let full = self.full_height().min(self.max_visible_items_height(self.all.iter()));
                filter_height + full > space_below && space_above > space_below
            });
            (if above { space_above } else { space_below } - chrome).max(0.)
        } else {
            let widget_top = anchor_top + anchor_height;
            if widget_top > 0. { viewport_height - widget_top - 10. } else { viewport_height * 0.7 }
        };
        let viewport_max = (viewport_height * 0.6).floor();
        let max_height = (available.max(ACTION_LINE_HEIGHT * 3. + filter_height)).min(viewport_max);
        (list_height + filter_height).min(max_height) - filter_height
    }

    /// `computeMaxWidth`: the widest row, at least `min_width` (the action bar's width) and the option's minimum,
    /// at most the option's maximum. Lists of 50 rows or more take 380px without measuring.
    pub fn compute_width(&self, min_width: f32, row_width: impl Fn(&Item<T>) -> f32) -> f32 {
        let min = min_width.max(self.options.min_width.unwrap_or(0.));
        let max = self.options.max_width.unwrap_or(f32::INFINITY).max(min);
        let clamp = |width: f32| width.max(min).min(max);
        if self.all.len() >= 50 {
            return clamp(380.);
        }
        clamp(self.all.iter().map(row_width).fold(f32::NEG_INFINITY, f32::max))
    }
}
