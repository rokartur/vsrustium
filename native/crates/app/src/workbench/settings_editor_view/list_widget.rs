//! settingsWidgets.ts `ListSettingWidget`, `ExcludeSettingWidget`, `IncludeSettingWidget`, `ObjectSettingDropdownWidget`
//! and `ObjectSettingCheckboxWidget`, styled as media/settingsWidgets.css, over the rows of `settings_editor::lists`.

use gpui::{AnyElement, ClickEvent, Context, FocusHandle, KeyDownEvent, Keystroke, MouseButton, MouseDownEvent, Rgba, Window, anchored, deferred, div, point, prelude::*, px, rgba};
use serde_json::Value;

use super::transparent;
use super::super::Workbench;
use super::super::input_box_view::{InputId, InputLook, InputView};
use crate::input_box::InputBox;
use crate::settings_editor::{Row, ValueType};
use crate::settings_editor::lists::{self, EditField, EntryType, ListEdit, ListItem};
use crate::settings_editor::validation::{js_string_to_number, validate};

const ROW_HEIGHT: f32 = 24.;

/// The list widget with the selected or edited row; one widget at a time, as the focus allows upstream.
pub struct ListEditor {
    setting: Option<usize>,
    selected: Option<usize>,
    hovered: Option<(usize, usize)>,
    /// The row in edit mode; `items.len()` is the new row the Add button opens.
    editing: Option<usize>,
    /// The edit row's key, typed into `key` when it is text.
    key_field: EditField,
    /// The include/exclude `when` sibling or the object entry's value, typed into `value` when it is text.
    value_field: Option<EditField>,
    pub key: InputBox,
    pub key_view: InputView,
    pub key_focus: FocusHandle,
    pub value: InputBox,
    pub value_view: InputView,
    pub value_focus: FocusHandle,
    /// The edit row's open select: the key's (true) or the value's, and the highlighted option.
    dropdown: Option<(bool, usize)>,
    pub focus: FocusHandle,
}

impl ListEditor {
    pub fn new(cx: &mut gpui::App) -> Self {
        ListEditor {
            setting: None,
            selected: None,
            hovered: None,
            editing: None,
            key_field: EditField::text(String::new()),
            value_field: None,
            key: InputBox::default(),
            key_view: InputView::default(),
            key_focus: cx.focus_handle(),
            value: InputBox::default(),
            value_view: InputView::default(),
            value_focus: cx.focus_handle(),
            dropdown: None,
            focus: cx.focus_handle(),
        }
    }

    pub fn is_focused(&self, window: &Window) -> bool {
        [&self.focus, &self.key_focus, &self.value_focus].into_iter().any(|focus| focus.is_focused(window))
    }

    /// `cancelEdit` and the selection drop, as when the row is rendered for another element.
    pub fn reset(&mut self) {
        self.setting = None;
        self.selected = None;
        self.editing = None;
        self.dropdown = None;
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Widget {
    List,
    Exclude,
    Include,
    Object,
}

struct Strings {
    delete: &'static str,
    edit: &'static str,
    add: &'static str,
    placeholder: &'static str,
    sibling_placeholder: &'static str,
}

/// `getLocalizedStrings` of each widget.
fn strings(widget: Widget) -> Strings {
    match widget {
        Widget::List => Strings { delete: "Remove Item", edit: "Edit Item", add: "Add Item", placeholder: "Item...", sibling_placeholder: "Sibling..." },
        Widget::Exclude => Strings {
            delete: "Remove Exclude Item",
            edit: "Edit Exclude Item",
            add: "Add Pattern",
            placeholder: "Exclude Pattern...",
            sibling_placeholder: "When Pattern Is Present...",
        },
        Widget::Include => Strings {
            delete: "Remove Include Item",
            edit: "Edit Include Item",
            add: "Add Pattern",
            placeholder: "Include Pattern...",
            sibling_placeholder: "When Pattern Is Present...",
        },
        Widget::Object => Strings { delete: "Remove Item", edit: "Edit Item", add: "Add Item", placeholder: "Key", sibling_placeholder: "Value" },
    }
}

fn widget_of(kind: ValueType) -> Widget {
    match kind {
        ValueType::Exclude => Widget::Exclude,
        ValueType::Include => Widget::Include,
        ValueType::Object => Widget::Object,
        _ => Widget::List,
    }
}

/// `rowElement` hover title: `addTooltipsToRow` of each widget.
fn row_title(widget: Widget, item: &ListItem) -> String {
    let sibling = item.value.as_ref().map(lists::display);
    match (widget, sibling) {
        (Widget::List, None) => format!("List item `{}`", item.key),
        (Widget::List, Some(sibling)) => format!("List item `{}` with sibling `${sibling}`", item.key),
        (Widget::Exclude, None) => format!("Exclude files matching `{}`", item.key),
        (Widget::Exclude, Some(sibling)) => format!("Exclude files matching `{}`, only when a file matching `{sibling}` is present", item.key),
        (Widget::Include, None) => format!("Include files matching `{}`", item.key),
        (Widget::Include, Some(sibling)) => format!("Include files matching `{}`, only when a file matching `{sibling}` is present", item.key),
        (Widget::Object, value) => item.description.clone().unwrap_or_else(|| format!("The property `{}` is set to `{}`.", item.key, value.unwrap_or_default())),
    }
}

impl Workbench {
    fn list_items(&self, index: usize, widget: Widget) -> Vec<ListItem> {
        let setting = &self.settings_editor.model.settings[index];
        let inspection = &self.settings_editor.inspections[index];
        match widget {
            Widget::List => lists::array_items(inspection),
            Widget::Exclude | Widget::Include => lists::include_exclude_items(inspection),
            Widget::Object => lists::object_items(setting, inspection),
        }
    }

    /// The array, include/exclude and object widgets: the rows, the edit row, then the Add button.
    pub(super) fn list_widget(&mut self, index: usize, kind: ValueType, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if kind == ValueType::BooleanObject {
            return self.bool_object_widget(index, cx);
        }
        let widget = widget_of(kind);
        let items = self.list_items(index, widget);
        let setting = &self.settings_editor.model.settings[index];
        let show_add = match widget {
            Widget::List => lists::array_shows_add_button(setting, items.len()),
            Widget::Object => lists::object_shows_add_button(setting, &items),
            Widget::Exclude | Widget::Include => true,
        };
        let state = &self.settings_editor.list;
        let ours = state.setting == Some(index);
        let editing = state.editing.filter(|_| ours);
        let list_focused = ours && state.focus.is_focused(window);
        // renderArrayValidations: array items validate as their display strings, objects as the value; include/exclude never do.
        let validated = match widget {
            Widget::List => Some(Value::Array(items.iter().map(|item| Value::String(item.key.clone())).collect())),
            Widget::Object => Some(self.settings_editor.inspections[index].value.clone()),
            Widget::Exclude | Widget::Include => None,
        };
        let error = validated.map(|value| validate(&setting.schema, &value)).filter(|error| !error.is_empty());
        let message = error.map(|error| self.validation_message(error).mt(px(16.)));
        let mut rows: Vec<AnyElement> = Vec::new();
        if widget == Widget::Object {
            rows.push(self.object_header());
        }
        for (row, item) in items.iter().enumerate() {
            rows.push(match editing == Some(row) {
                true => self.list_edit_row(index, widget, window, cx),
                false => self.list_data_row(index, widget, row, item, list_focused, cx),
            });
        }
        let adding = editing == Some(items.len());
        if adding {
            rows.push(self.list_edit_row(index, widget, window, cx));
        }
        let add_button = (show_add && !adding).then(|| {
            text_button(self, strings(widget).add, true)
                .id(("setting-list-add", index))
                .self_start()
                .mt(px(4.))
                .mr(px(4.))
                .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| workbench.start_list_edit(index, None, window, cx)))
        });
        div()
            .id(("setting-list", index))
            .w_full()
            .flex()
            .flex_col()
            .children(message)
            .child(
                div()
                    .id(("setting-list-rows", index))
                    .track_focus(&self.settings_editor.list.focus)
                    .mb(px(1.))
                    .p(px(1.))
                    .flex()
                    .flex_col()
                    .on_key_down(cx.listener(move |workbench, event: &KeyDownEvent, window, cx| workbench.list_rows_key(index, event, window, cx)))
                    .children(rows),
            )
            .children(add_button)
            .into_any_element()
    }

    fn object_header(&self) -> AnyElement {
        div()
            .flex()
            .h(px(ROW_HEIGHT))
            .pr(px(4.))
            .font_weight(gpui::FontWeight::BOLD)
            .bg(odd_row_background())
            .child(div().w(gpui::relative(0.4)).ml(px(4.)).mr(px(3.)).line_height(px(ROW_HEIGHT)).child("Item"))
            .child(div().w(gpui::relative(0.6)).line_height(px(ROW_HEIGHT)).child("Value"))
            .into_any_element()
    }

    fn list_data_row(&mut self, index: usize, widget: Widget, row: usize, item: &ListItem, list_focused: bool, cx: &mut Context<Self>) -> AnyElement {
        let state = &self.settings_editor.list;
        let selected = state.setting == Some(index) && state.selected == Some(row);
        let hovered = state.hovered == Some((index, row));
        let (background, foreground) = match (selected, list_focused, hovered) {
            (true, true, _) => (Some(self.color("list.activeSelectionBackground")), self.theme.color("list.activeSelectionForeground")),
            (true, false, _) => (Some(self.color("list.inactiveSelectionBackground")), self.theme.color("list.inactiveSelectionForeground")),
            (false, _, true) => (self.theme.color("list.hoverBackground"), self.theme.color("list.hoverForeground")),
            // `.setting-list-object-row:nth-child(odd)`: the header is the first child.
            _ => ((widget == Widget::Object && row % 2 == 1).then(odd_row_background), None),
        };
        let monospace = matches!(widget, Widget::Exclude | Widget::Include);
        let text = |content: String| {
            let cell = div().line_height(px(ROW_HEIGHT)).min_h(px(ROW_HEIGHT)).flex_none().truncate().child(content);
            if monospace { cell.font_family("Menlo") } else { cell }
        };
        let cells: Vec<AnyElement> = match widget {
            Widget::Object => vec![
                text(item.key.clone()).w(gpui::relative(0.4)).ml(px(4.)).mr(px(3.)).into_any_element(),
                text(item.value.as_ref().map(lists::display).unwrap_or_default()).w(gpui::relative(0.6)).pr(px(40.)).into_any_element(),
            ],
            _ => {
                let sibling = item.value.as_ref().map(lists::display);
                let value = text(item.key.clone()).ml(px(2.)).mr(px(3.)).pr(px(40.)).w(gpui::relative(if sibling.is_some() { 0.6 } else { 1. }));
                let sibling = sibling.map(|sibling| text(format!("when: {sibling}")).w(gpui::relative(0.4)).ml(px(6.)).opacity(0.7).text_size(px(11.7)).pr(px(50.)).into_any_element());
                std::iter::once(value.into_any_element()).chain(sibling).collect()
            }
        };
        let names = strings(widget);
        let mut actions: Vec<AnyElement> = vec![self.list_action(index, row, "edit", names.edit, 4., cx)];
        if widget == Widget::Object && item.resetable {
            actions.push(self.list_action(index, row, "discard", "Reset Item", 2., cx));
        }
        if widget != Widget::Object || item.removable {
            actions.push(self.list_action(index, row, "close", names.delete, 2., cx));
        }
        let title = row_title(widget, item);
        div()
            .id(("setting-list-row", row))
            .relative()
            .flex()
            .max_h(px(ROW_HEIGHT))
            .when(widget == Widget::Object, |row| row.pr(px(4.)))
            .when_some(background, |row, background| row.bg(background))
            .when_some(foreground, |row, foreground| row.text_color(foreground))
            .on_hover(cx.listener(move |workbench, hovered: &bool, _, cx| {
                let state = &mut workbench.settings_editor.list;
                if *hovered {
                    state.hovered = Some((index, row));
                } else if state.hovered == Some((index, row)) {
                    state.hovered = None;
                }
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                if event.click_count == 2 {
                    workbench.start_list_edit(index, Some(row), window, cx);
                    return;
                }
                workbench.select_list_row(index, row, window, cx);
            }))
            .children(cells)
            .when(selected || hovered, |row| row.child(div().absolute().right_0().top_0().flex().children(actions)))
            .child(self.hover_target(("setting-list-row-hover", row), super::super::hover::HoverContent::Text(title), super::super::hover::HoverOptions::mouse()))
            .into_any_element()
    }

    /// `.monaco-action-bar .action-label` of a row: a 16x20 codicon button.
    fn list_action(&self, index: usize, row: usize, icon: &'static str, tooltip: &'static str, margin_right: f32, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id(icon)
            .w(px(20.))
            .h(px(24.))
            .mr(px(margin_right))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .child(self.icon(icon, 16., 16., self.foreground_or("icon.foreground")))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| match icon {
                "edit" => workbench.start_list_edit(index, Some(row), window, cx),
                "discard" => workbench.apply_list_edit(index, ListEdit::Reset { index: row }, window, cx),
                _ => workbench.apply_list_edit(index, ListEdit::Remove { index: row }, window, cx),
            }))
            .child(self.hover_target(("setting-list-action", row), super::super::hover::HoverContent::Text(tooltip.into()), super::super::hover::HoverOptions::mouse()))
            .into_any_element()
    }

    /// `renderEdit`: the key input or select, the sibling or value, OK and Cancel.
    fn list_edit_row(&mut self, index: usize, widget: Widget, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let names = strings(widget);
        let state = &self.settings_editor.list;
        let key_field = state.key_field.clone();
        let value_field = state.value_field.clone();
        let ok_enabled = widget != Widget::Object || key_field.kind != EntryType::String || !state.key.value.is_empty();
        let mut row = div().flex().items_center().min_h(px(ROW_HEIGHT)).when(widget == Widget::Object, |row| row.pr(px(4.)));
        let key_width = match (widget, &value_field) {
            (Widget::Object, _) => gpui::relative(0.4),
            (_, Some(_)) => gpui::relative(0.6),
            (_, None) => gpui::relative(1.),
        };
        row = row.child(div().flex_none().w(key_width).mr(px(4.)).child(self.list_field(true, &key_field, names.placeholder, window, cx)));
        if let Some(value_field) = &value_field {
            let placeholder = names.sibling_placeholder;
            row = row.child(div().flex_1().min_w_0().when(widget != Widget::Object, |cell| cell.max_w(px(320.)).mr(px(4.))).child(self.list_field(false, value_field, placeholder, window, cx)));
        }
        let ok = text_button(self, "OK", true)
            .id("setting-list-ok")
            .mx(px(4.))
            .when(!ok_enabled, |button| button.opacity(0.4))
            .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| workbench.commit_list_edit(index, window, cx)));
        let cancel = text_button(self, "Cancel", false).id("setting-list-cancel").on_click(cx.listener(|workbench, _: &ClickEvent, window, cx| workbench.cancel_list_edit(window, cx)));
        row.child(ok).child(cancel).into_any_element()
    }

    /// An `InputBox` for text, a `SelectBox` for an enum or boolean.
    fn list_field(&self, key: bool, field: &EditField, placeholder: &str, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let state = &self.settings_editor.list;
        if field.kind == EntryType::String {
            let (id, focus) = match key {
                true => (InputId::SettingsListKey, &state.key_focus),
                false => (InputId::SettingsListValue, &state.value_focus),
            };
            let focused = focus.is_focused(window);
            let border = if focused { self.color("focusBorder") } else { self.theme.color("settings.textInputBorder").unwrap_or(transparent()) };
            return div()
                .id(if key { "setting-list-key-input" } else { "setting-list-value-input" })
                .h(px(ROW_HEIGHT))
                .px(px(4.))
                .flex()
                .items_center()
                .rounded(px(2.))
                .border_1()
                .border_color(border)
                .bg(self.color("settings.textInputBackground"))
                .text_color(self.color("settings.textInputForeground"))
                .cursor_text()
                .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, _, window, cx| {
                    cx.stop_propagation();
                    workbench.focus_list_input(key, window, cx);
                }))
                .child(self.input_text(id, InputLook { password: false, placeholder: Some(placeholder.into()), focused }, cx))
                .into_any_element();
        }
        let open = state.dropdown.filter(|(dropdown_key, _)| *dropdown_key == key).map(|(_, option)| option);
        let select = div()
            .id(if key { "setting-list-key-select" } else { "setting-list-value-select" })
            .w_full()
            .h(px(ROW_HEIGHT))
            .px(px(6.))
            .flex()
            .items_center()
            .justify_between()
            .rounded(px(4.))
            .border_1()
            .border_color(self.theme.color("settings.dropdownBorder").unwrap_or(transparent()))
            .bg(self.color("settings.dropdownBackground"))
            .text_color(self.color("settings.dropdownForeground"))
            .cursor_pointer()
            .child(div().truncate().child(field.text.clone()))
            .child(self.icon("chevron-down", 16., 16., self.color("settings.dropdownForeground")))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                let state = &mut workbench.settings_editor.list;
                let field = if key { &state.key_field } else { state.value_field.as_ref().expect("a value select has a value field") };
                let selected = field.options.iter().position(|option| *option == field.text).unwrap_or(0);
                state.dropdown = Some((key, selected));
                let focus = state.focus.clone();
                window.focus(&focus, cx);
                cx.notify();
            }));
        let Some(highlighted) = open else { return select.into_any_element() };
        let options = field.options.iter().enumerate().map(|(option, label)| {
            div()
                .id(("setting-list-option", option))
                .h(px(22.))
                .pl(px(6.))
                .flex()
                .items_center()
                .cursor_pointer()
                .when(option == highlighted, |item| item.bg(self.color("list.activeSelectionBackground")).text_color(self.foreground_or("list.activeSelectionForeground")))
                .child(label.clone())
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |workbench, _: &ClickEvent, _, cx| workbench.pick_list_option(key, option, cx)))
        });
        let menu = div()
            .min_w(px(160.))
            .flex()
            .flex_col()
            .py(px(2.))
            .rounded(px(8.))
            .border_1()
            .border_color(self.theme.color("settings.dropdownListBorder").or(self.theme.color("settings.dropdownBorder")).unwrap_or(transparent()))
            .bg(self.color("settings.dropdownBackground"))
            .text_color(self.color("settings.dropdownForeground"))
            .shadow_lg()
            .children(options);
        div()
            .relative()
            .child(select)
            .child(deferred(anchored().position_mode(gpui::AnchoredPositionMode::Local).offset(point(px(0.), px(ROW_HEIGHT))).child(menu)).with_priority(2))
            .into_any_element()
    }

    /// `SettingBoolObjectRenderer`: a checkbox and the key of each declared property.
    fn bool_object_widget(&mut self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let setting = &self.settings_editor.model.settings[index];
        let items = lists::bool_object_items(setting, &self.settings_editor.inspections[index]);
        let border = self.theme.color("settings.checkboxBorder").unwrap_or(transparent());
        let rows = items.iter().enumerate().map(|(row, item)| {
            let checked = item.value == Some(Value::Bool(true));
            let title = item.description.clone().unwrap_or_else(|| item.key.clone());
            div()
                .id(("setting-bool-object-row", row))
                .flex()
                .items_center()
                .h(px(ROW_HEIGHT))
                .cursor_pointer()
                .child(
                    div()
                        .ml(px(4.))
                        .mr(px(9.))
                        .size(px(18.))
                        .flex_none()
                        .rounded(px(3.))
                        .border_1()
                        .border_color(border)
                        .bg(self.color("settings.checkboxBackground"))
                        .when(checked, |checkbox| checkbox.child(self.icon("check", 16., 16., self.color("settings.checkboxForeground")))),
                )
                .child(div().flex_1().truncate().child(item.key.clone()))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |workbench, _: &ClickEvent, _, cx| {
                    let editor = &workbench.settings_editor;
                    let items = lists::bool_object_items(&editor.model.settings[index], &editor.inspections[index]);
                    let value = lists::new_bool_object_value(&editor.inspections[index], &items, row, !checked);
                    workbench.write_setting(index, value, false, cx);
                }))
                .child(self.hover_target(("setting-bool-object-hover", row), super::super::hover::HoverContent::Text(title), super::super::hover::HoverOptions::mouse()))
        });
        div().id(("setting-bool-object", index)).w_full().flex().flex_col().children(rows).into_any_element()
    }

    /// A press in the widget moves the tree's focus to its setting, as DOM focus does upstream.
    fn focus_tree_row_of(&mut self, index: usize) {
        let editor = &mut self.settings_editor;
        let row = editor.rows.iter().position(|row| matches!(row, Row::Setting(setting, _) if *setting == index));
        editor.focused = row.or(editor.focused);
    }

    fn select_list_row(&mut self, index: usize, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_tree_row_of(index);
        let state = &mut self.settings_editor.list;
        if state.setting != Some(index) {
            state.reset();
            state.setting = Some(index);
        }
        state.selected = Some(row);
        state.dropdown = None;
        let focus = state.focus.clone();
        window.focus(&focus, cx);
        cx.notify();
    }

    /// `editSetting` and the Add button: the row turns into inputs holding its key and sibling or value.
    fn start_list_edit(&mut self, index: usize, row: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_tree_row_of(index);
        let kind = crate::settings_editor::value_type(&self.settings_editor.model.settings[index]);
        let widget = widget_of(kind);
        let items = self.list_items(index, widget);
        let setting = &self.settings_editor.model.settings[index];
        let keys: Vec<String> = items.iter().map(|item| item.key.clone()).collect();
        let item = row.and_then(|row| items.get(row));
        let key_text = item.map(|item| item.key.clone()).unwrap_or_default();
        let (key_field, value_field) = match widget {
            Widget::List => {
                let options = lists::array_key_options(setting, &keys, row);
                let key = match options.is_empty() {
                    true => EditField::text(key_text),
                    false => {
                        let text = if item.is_some() { key_text } else { options[0].clone() };
                        EditField { kind: EntryType::Enum, text, options }
                    }
                };
                (key, item.and_then(|item| item.value.as_ref()).map(|sibling| EditField::text(lists::display(sibling))))
            }
            Widget::Exclude | Widget::Include => (EditField::text(key_text), item.and_then(|item| item.value.as_ref()).map(|sibling| EditField::text(lists::display(sibling)))),
            Widget::Object => match item {
                Some(item) => (EditField::text(key_text), Some(lists::object_value_field(setting, item))),
                None => {
                    let options = lists::object_key_options(setting, &keys);
                    let key = match options.first() {
                        Some(first) => EditField { kind: EntryType::Enum, text: first.clone(), options: options.clone() },
                        None => EditField::text(String::new()),
                    };
                    let value = lists::object_value_suggestion(setting, &key.text).unwrap_or_else(|| EditField::text(String::new()));
                    (key, Some(value))
                }
            },
        };
        let state = &mut self.settings_editor.list;
        state.reset();
        state.setting = Some(index);
        state.selected = row;
        state.editing = Some(row.unwrap_or(items.len()));
        state.key = InputBox::new(key_field.text.clone());
        state.key.select_all();
        state.key_view.reset();
        state.value = InputBox::new(value_field.as_ref().map(|field| field.text.clone()).unwrap_or_default());
        state.value.select_all();
        state.value_view.reset();
        let key_is_text = key_field.kind == EntryType::String;
        let value_is_text = value_field.as_ref().is_some_and(|field| field.kind == EntryType::String);
        state.key_field = key_field;
        state.value_field = value_field;
        match (key_is_text, value_is_text) {
            (true, _) => self.focus_list_input(true, window, cx),
            (false, true) => self.focus_list_input(false, window, cx),
            (false, false) => {
                let focus = self.settings_editor.list.focus.clone();
                window.focus(&focus, cx);
            }
        }
        cx.notify();
    }

    fn focus_list_input(&mut self, key: bool, window: &mut Window, cx: &mut Context<Self>) {
        let state = &self.settings_editor.list;
        let (id, focus) = if key { (InputId::SettingsListKey, state.key_focus.clone()) } else { (InputId::SettingsListValue, state.value_focus.clone()) };
        window.focus(&focus, cx);
        self.blink_caret(id, window, cx);
        cx.notify();
    }

    pub(in crate::workbench) fn edit_settings_list_input(&mut self, key: bool, edit: impl FnOnce(&mut InputBox), cx: &mut Context<Self>) {
        let state = &mut self.settings_editor.list;
        edit(if key { &mut state.key } else { &mut state.value });
        if key && state.editing.is_some() {
            let typed = state.key.value.clone();
            self.object_key_changed(typed);
        }
        cx.notify();
    }

    /// `onKeyChange`: a new key of an object brings its suggested value when `shouldUseSuggestion` says so.
    fn object_key_changed(&mut self, key: String) {
        let editor = &mut self.settings_editor;
        let Some(index) = editor.list.setting else { return };
        let setting = &editor.model.settings[index];
        if widget_of(crate::settings_editor::value_type(setting)) != Widget::Object {
            return;
        }
        let state = &mut editor.list;
        state.key_field.text = key.clone();
        let Some(previous) = state.value_field.clone() else { return };
        let original_empty = state.editing.is_some_and(|row| state.selected != Some(row));
        let Some(suggested) = lists::object_value_suggestion(setting, &key) else { return };
        if !lists::should_use_suggestion(original_empty, &previous, &suggested) {
            return;
        }
        state.value = InputBox::new(suggested.text.clone());
        state.value_view.reset();
        state.value_field = Some(suggested);
    }

    fn pick_list_option(&mut self, key: bool, option: usize, cx: &mut Context<Self>) {
        let state = &mut self.settings_editor.list;
        state.dropdown = None;
        let field = if key { &mut state.key_field } else { state.value_field.as_mut().expect("a value select has a value field") };
        field.text = field.options[option].clone();
        if key {
            let typed = field.text.clone();
            self.object_key_changed(typed);
        }
        cx.notify();
    }

    fn cancel_list_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = &mut self.settings_editor.list;
        state.editing = None;
        state.dropdown = None;
        let focus = state.focus.clone();
        window.focus(&focus, cx);
        cx.notify();
    }

    /// `handleItemChange`: OK or Enter fires `add` for the new row, `change` for an existing one.
    fn commit_list_edit(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let state = &self.settings_editor.list;
        let Some(row) = state.editing else { return };
        let key = match state.key_field.kind {
            EntryType::String => state.key.value.clone(),
            _ => state.key_field.text.clone(),
        };
        let value = state.value_field.as_ref().map(|field| match field.kind {
            EntryType::String => Value::String(state.value.value.clone()),
            _ => field.value(),
        });
        let widget = widget_of(crate::settings_editor::value_type(&self.settings_editor.model.settings[index]));
        if widget == Widget::Object && key.is_empty() {
            return;
        }
        let new_row = row >= self.list_items(index, widget).len();
        let edit = match new_row {
            true => ListEdit::Add { key, value },
            false => ListEdit::Change { index: row, key, value },
        };
        self.apply_list_edit(index, edit, window, cx);
    }

    /// The setting's `onDidChangeList`: the new value is written, the edit row closes.
    fn apply_list_edit(&mut self, index: usize, edit: ListEdit, window: &mut Window, cx: &mut Context<Self>) {
        let editor = &self.settings_editor;
        let setting = &editor.model.settings[index];
        let inspection = &editor.inspections[index];
        let kind = crate::settings_editor::value_type(setting);
        let widget = widget_of(kind);
        let items = self.list_items(index, widget);
        // The empty item of the list widgets (`isItemNew`) is never written.
        if let ListEdit::Add { key, .. } | ListEdit::Change { key, .. } = &edit
            && key.is_empty()
            && widget != Widget::Object
        {
            self.cancel_list_edit(window, cx);
            return;
        }
        let workspace_target = matches!(editor.target, config::service::Target::Workspace | config::service::Target::WorkspaceFolder);
        let value = match widget {
            Widget::List => lists::new_array_value(inspection, workspace_target, &edit).map(|value| numeric_array(setting, value)),
            Widget::Exclude | Widget::Include => lists::new_include_exclude_value(inspection, &items, &edit),
            Widget::Object => lists::new_object_value(setting, inspection, &items, &edit),
        };
        let state = &mut self.settings_editor.list;
        state.editing = None;
        state.dropdown = None;
        if matches!(edit, ListEdit::Remove { .. }) {
            state.selected = None;
        }
        let focus = state.focus.clone();
        window.focus(&focus, cx);
        self.write_setting(index, value, matches!(edit, ListEdit::Reset { .. }), cx);
    }

    /// Up and Down on the rows (`selectPreviousRow`, `selectNextRow`); the open select's keys.
    fn list_rows_key(&mut self, index: usize, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let state = &mut self.settings_editor.list;
        if !state.focus.is_focused(window) || state.setting != Some(index) {
            return;
        }
        let key = event.keystroke.key.as_str();
        if let Some((dropdown_key, option)) = state.dropdown {
            let count = if dropdown_key { state.key_field.options.len() } else { state.value_field.as_ref().map_or(0, |field| field.options.len()) };
            cx.stop_propagation();
            match key {
                "down" => state.dropdown = Some((dropdown_key, (option + 1).min(count.saturating_sub(1)))),
                "up" => state.dropdown = Some((dropdown_key, option.saturating_sub(1))),
                "enter" | "space" => self.pick_list_option(dropdown_key, option, cx),
                "escape" => state.dropdown = None,
                _ => {}
            }
            cx.notify();
            return;
        }
        if state.editing.is_some() {
            if list_input_key(&event.keystroke) == Some(false) {
                cx.stop_propagation();
                self.cancel_list_edit(window, cx);
            } else if list_input_key(&event.keystroke) == Some(true) {
                cx.stop_propagation();
                self.commit_list_edit(index, window, cx);
            }
            return;
        }
        let kind = crate::settings_editor::value_type(&self.settings_editor.model.settings[index]);
        let count = self.list_items(index, widget_of(kind)).len();
        let state = &mut self.settings_editor.list;
        let next = match (key, state.selected) {
            ("down", Some(row)) => (row + 1) % count.max(1),
            ("down", None) => 0,
            ("up", Some(row)) => (row + count - 1) % count.max(1),
            ("up", None) => count.saturating_sub(1),
            _ => return,
        };
        cx.stop_propagation();
        if count > 0 {
            state.selected = Some(next);
        }
        cx.notify();
    }

    /// Enter and Escape in an edit row's input; true when the key was the widget's.
    pub(in crate::workbench) fn settings_list_input_key(&mut self, keystroke: &Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(commit) = list_input_key(keystroke) else { return false };
        let Some(index) = self.settings_editor.list.setting else { return false };
        match commit {
            true => self.commit_list_edit(index, window, cx),
            false => self.cancel_list_edit(window, cx),
        }
        true
    }

    /// `focusSettings` into a list setting: its rows, the first selected.
    pub(super) fn focus_list_widget(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let kind = crate::settings_editor::value_type(&self.settings_editor.model.settings[index]);
        let count = match kind {
            ValueType::BooleanObject => 0,
            kind => self.list_items(index, widget_of(kind)).len(),
        };
        let state = &mut self.settings_editor.list;
        if state.setting != Some(index) {
            state.reset();
            state.setting = Some(index);
        }
        if state.selected.is_none() && count > 0 {
            state.selected = Some(0);
        }
        let focus = state.focus.clone();
        window.focus(&focus, cx);
        cx.notify();
    }
}

/// Some(true) for Enter, Some(false) for Escape, without modifiers.
fn list_input_key(keystroke: &Keystroke) -> Option<bool> {
    if keystroke.modifiers.modified() {
        return None;
    }
    match keystroke.key.as_str() {
        "enter" => Some(true),
        "escape" => Some(false),
        _ => None,
    }
}

/// `arrToSave`: number and integer item types write numbers (`+a`) once the array validates;
/// an invalid array is written unparsed, with its errors shown.
fn numeric_array(setting: &crate::default_settings::Setting, value: Value) -> Value {
    let item_type = &setting.schema["items"]["type"];
    if item_type != "number" && item_type != "integer" {
        return value;
    }
    let Value::Array(items) = value else { return value };
    let strings: Vec<Value> = items.iter().map(|item| Value::String(lists::display(item))).collect();
    if !validate(&setting.schema, &Value::Array(strings.clone())).is_empty() {
        return Value::Array(strings);
    }
    let numbers = strings.iter().map(|text| {
        let number = js_string_to_number(text.as_str().unwrap_or_default()).unwrap_or_default();
        match number.fract() == 0.0 && number.abs() < i64::MAX as f64 {
            true => Value::from(number as i64),
            false => serde_json::Number::from_f64(number).map_or(Value::Null, Value::Number),
        }
    });
    Value::Array(numbers.collect())
}

/// `.setting-list-object-row:nth-child(odd)` and the header: rgba(130, 130, 130, 0.04).
fn odd_row_background() -> Rgba {
    rgba(0x8282820a)
}

/// A `monaco-text-button` with the default button styles, primary or secondary.
fn text_button(workbench: &Workbench, label: &'static str, primary: bool) -> gpui::Div {
    let (background, foreground, hover) = match primary {
        true => ("button.background", "button.foreground", "button.hoverBackground"),
        false => ("button.secondaryBackground", "button.secondaryForeground", "button.secondaryHoverBackground"),
    };
    let hover_color = workbench.color(hover);
    div()
        .h(px(26.))
        .px(px(8.))
        .flex()
        .flex_none()
        .items_center()
        .rounded(px(4.))
        .cursor_pointer()
        .bg(workbench.color(background))
        .text_color(workbench.color(foreground))
        .when_some(workbench.theme.color(if primary { "button.border" } else { "button.secondaryBorder" }), |button, border| button.border_1().border_color(border))
        .hover(move |button| button.bg(hover_color))
        .whitespace_nowrap()
        .child(label)
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
}
