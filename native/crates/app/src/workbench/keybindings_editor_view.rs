//! The Keyboard Shortcuts editor (contrib/preferences/browser/keybindingsEditor.ts with keybindingWidgets.ts) as
//! the modal editor part shows it (browser/parts/editor/modalEditorPart.ts), styled as media/keybindingsEditor.css,
//! media/keybindings.css and media/modalEditorPart.css draw it.

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::ops::Range;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, BoxShadow, ClipboardItem, Context, Div, FocusHandle, FontWeight, HighlightStyle, ListAlignment, ListState, MouseButton, MouseDownEvent, PromptLevel, Rgba, Stateful,
    MouseMoveEvent, MouseUpEvent, StyledText, Subscription, Task, Window, canvas, div, list, point, prelude::*, px, rgb, rgba,
};
use serde_json::{Value, json};

use super::Workbench;
use super::input_box_view::{InputId, InputLook, InputView};
use crate::commands::Outcome;
use crate::input_box::InputBox;
use crate::keybinding_editing::{self, Change};
use crate::keybindings::{self, Chord, Resolution};
use crate::keybindings_editor::{ChordMatch, Entry, Model, Source};
use crate::main_thread_quick_open::Owner;
use crate::menus::MenuAction;
use crate::quick_input::{Entry as PickEntry, Event, Item, Kind, QuickInput, Severity};
use crate::split_view::{SashState, SplitView, View};
use crate::theme::ThemeKind;

pub const OPEN_COMMAND: &str = "workbench.action.openGlobalKeybindings";
const CLOSE_MODAL_COMMAND: &str = "workbench.action.closeModalEditor";
/// `MODAL_MAX_DEFAULT_WIDTH`, `MODAL_MAX_DEFAULT_HEIGHT` and `MODAL_MAXIMIZED_PADDING` of modalEditorPart.ts.
const MODAL_MAX_WIDTH: f32 = 1400.;
const MODAL_MAX_HEIGHT: f32 = 900.;
const MODAL_MAXIMIZED_PADDING: f32 = 16.;
/// The title bar part's height, `mainContainerOffset.top`.
const TITLE_BAR_HEIGHT: f32 = 35.;
const MODAL_HEADER_HEIGHT: f32 = 32.;
const TABLE_HEADER_HEIGHT: f32 = 30.;
const ACTIONS_COLUMN_WIDTH: f32 = 40.;
/// The column weights of the table after the 40px actions column: Command, Keybinding, When, Source.
const COLUMN_WEIGHTS: [f32; 4] = [0.3, 0.2, 0.35, 0.15];
/// `ColumnHeader.minimumSize` (tableWidget.ts) for a column without `minimumWidth`.
const MIN_COLUMN_WIDTH: f32 = 120.;
const COLUMN_LABELS: [&str; 4] = ["Command", "Keybinding", "When", "Source"];
/// `--vscode-sash-size`.
pub(super) const SASH_SIZE: f32 = 4.;
/// `workbench.sash.hoverDelay` default.
/// The `border-color 0.2s ease-out` transition of the column borders on table hover.
const TABLE_BORDER_FADE: Duration = Duration::from_millis(200);
/// `DefineKeybindingWidget.WIDTH` and `HEIGHT`.
const DEFINE_WIDTH: f32 = 400.;
const DEFINE_HEIGHT: f32 = 110.;
const SEARCH_PLACEHOLDER: &str = "Type to search in keybindings";
const RECORDING_PLACEHOLDER: &str = "Recording Keys. Press Escape to exit";
const DIRTY_FILE_ERROR: &str = "Unable to write because the keybindings configuration file has unsaved changes. Please save it first and then try again.";

/// `defaultModalEditorAllowableCommands`: the `workbench.*` commands a key may run while focus is in the modal.
const MODAL_ALLOWED_COMMANDS: &[&str] = &[
    "workbench.action.quit",
    "workbench.action.reloadWindow",
    "workbench.action.toggleFullScreen",
    "workbench.action.gotoSymbol",
    "workbench.action.gotoLine",
    "workbench.action.zoomIn",
    "workbench.action.zoomOut",
    "workbench.action.zoomReset",
    "workbench.action.files.save",
    "workbench.action.files.saveAll",
    "workbench.action.files.revert",
    "workbench.action.closeActiveEditor",
    "workbench.action.closeAllEditors",
    "workbench.action.closeEditorsInGroup",
    "workbench.action.closeUnmodifiedEditors",
    "workbench.action.openSettings",
    "workbench.action.openSettings2",
    "workbench.action.openSettingsJson",
    "workbench.action.openGlobalSettings",
    "workbench.action.openApplicationSettingsJson",
    "workbench.action.openRawDefaultSettings",
    "workbench.action.openWorkspaceSettings",
    "workbench.action.openWorkspaceSettingsFile",
    "workbench.action.openFolderSettings",
    "workbench.action.openFolderSettingsFile",
    "workbench.action.openRemoteSettings",
    "workbench.action.openRemoteSettingsFile",
    "workbench.action.openAccessibilitySettings",
    "workbench.action.configureLanguageBasedSettings",
    "workbench.action.openGlobalKeybindings",
    "workbench.action.openDefaultKeybindingsFile",
    "workbench.action.openGlobalKeybindingsFile",
    "workbench.action.openKeyboardLayoutPicker",
    CLOSE_MODAL_COMMAND,
    "workbench.action.moveModalEditorToMain",
    "workbench.action.moveModalEditorToWindow",
    "workbench.action.toggleModalEditorMaximized",
    "workbench.action.navigateModalEditorPrevious",
    "workbench.action.navigateModalEditorNext",
    "workbench.action.toggleModalEditorSidebar",
];

/// The editor a modal editor part shows: its title bar icon and title, and its "Open ... (JSON)" action.
pub(super) struct ModalHeader {
    pub icon: &'static str,
    pub title: &'static str,
    pub json_title: &'static str,
    pub json_command: &'static str,
    /// A text editor's file icon, name and description, drawn in place of `icon` and `title`.
    pub resource: Option<AnyElement>,
}

pub struct KeybindingsEditor {
    pub open: bool,
    /// The modal editor part's maximized state, which the Settings editor in it shares.
    pub(super) maximized: bool,
    model: Model,
    /// The `Keybindings::generation` the rows were built from.
    model_generation: u64,
    entries: Vec<Entry>,
    /// The table's focused and selected row.
    selected: Option<usize>,
    /// `list.toggleSelection` took the focused row out of the selection, until the focus moves.
    unselected: bool,
    hovered: Option<usize>,
    sort_by_precedence: bool,
    /// Record Keys is checked: keys pressed in the search are typed as their user settings labels.
    recording: bool,
    recorded: Vec<Chord>,
    define: Option<Define>,
    pub when: Option<WhenEdit>,
    /// The command whose row is revealed and selected once the model has it bound (`unAssignedKeybindingItemToRevealAndFocus`).
    reveal_command: Option<String>,
    pub search: InputBox,
    pub search_view: InputView,
    pub search_focus: FocusHandle,
    pub when_view: InputView,
    pub when_focus: FocusHandle,
    pub(super) table_focus: FocusHandle,
    define_focus: FocusHandle,
    modal_focus: FocusHandle,
    pub(super) list: ListState,
    /// The table's column headers, resized by their sashes and `list.resizeColumn`.
    columns: SplitView,
    /// The sash under the pointer, shown as hovered once `workbench.sash.hoverDelay` passed, and its timer.
    sash_hovered: Option<usize>,
    sash_hover: Option<Task<()>>,
    /// Whether the pointer is over the table, and since when, for the column borders' fade.
    table_hovered: (bool, Instant),
    listed: usize,
    revealed: Option<usize>,
    /// `delayedFiltering`: the search filters 300ms after the last change.
    filtering: Option<Task<()>>,
    previous_focus: Option<FocusHandle>,
    define_blur: Option<Subscription>,
    when_blur: Option<Subscription>,
}

/// The shown `DefineKeybindingWidget`: the row it defines a keybinding for, whether it adds one, and the chords
/// pressed so far. No row is the `DefineKeybindingOverlayWidget` over the user keybindings.json.
pub(super) struct Define {
    row: Option<(Entry, bool)>,
    chords: Vec<Chord>,
}

/// The `WhenInputWidget` of a row's When column.
pub struct WhenEdit {
    entry: Entry,
    pub input: InputBox,
}

impl KeybindingsEditor {
    pub fn new(cx: &mut gpui::App) -> Self {
        KeybindingsEditor {
            open: false,
            maximized: false,
            model: Model::default(),
            model_generation: 0,
            entries: Vec::new(),
            selected: None,
            unselected: false,
            hovered: None,
            sort_by_precedence: false,
            recording: false,
            recorded: Vec::new(),
            define: None,
            when: None,
            reveal_command: None,
            search: InputBox::default(),
            search_view: InputView::default(),
            search_focus: cx.focus_handle(),
            when_view: InputView::default(),
            when_focus: cx.focus_handle(),
            table_focus: cx.focus_handle(),
            define_focus: cx.focus_handle(),
            modal_focus: cx.focus_handle(),
            list: ListState::new(0, ListAlignment::Top, px(200.)),
            columns: table_columns(),
            sash_hovered: None,
            sash_hover: None,
            table_hovered: (false, Instant::now() - TABLE_BORDER_FADE),
            listed: 0,
            revealed: None,
            filtering: None,
            previous_focus: None,
            define_blur: None,
            when_blur: None,
        }
    }

    fn focused_entry(&self) -> Option<&Entry> {
        self.entries.get(self.selected?)
    }
}

/// `printKeybinding` of `KeybindingsSearchWidget`: a third chord starts over. GPUI reports no key down for a lone
/// modifier, so the incomplete chord upstream replaces never shows.
pub fn record_chord(chords: &mut Vec<Chord>, chord: Chord) {
    if chords.len() == 2 {
        chords.clear();
    }
    chords.push(chord);
}

/// `Delegate.getHeight`: rows whose command id or extension id matched show it on a second line.
pub fn row_height(entry: &Entry) -> f32 {
    if entry.matches.extension_id.is_some() || entry.matches.command_id.is_some() { 40. } else { 24. }
}

/// `JSON.stringify(userFriendlyKeybinding, null, '  ')` of `copyKeybinding`.
pub fn copied_entry(entry: &Entry) -> String {
    let row = &entry.row;
    let mut object = json!({ "key": keybindings::user_settings_label(&row.keybinding), "command": row.command });
    if !row.when.is_empty() {
        object["when"] = json!(row.when);
    }
    serde_json::to_string_pretty(&object).expect("a JSON object serializes")
}

/// `onContextMenu`: the row's actions, enabled as its keybinding allows.
pub fn context_menu_actions(entry: &Entry) -> Vec<MenuAction> {
    let row = &entry.row;
    let bound = !row.keybinding.is_empty();
    let action = |id: &str, title: &str, enabled: bool| MenuAction::Command { id: id.into(), title: title.into(), enabled, checked: false, alt: None };
    let mut actions = vec![
        action("keybindings.editor.copyKeybindingEntry", "Copy", true),
        action("keybindings.editor.copyCommandKeybindingEntry", "Copy Command ID", true),
        action("keybindings.editor.copyCommandTitle", "Copy Command Title", !row.command_label.is_empty()),
        MenuAction::Separator,
    ];
    if bound {
        actions.push(action("keybindings.editor.defineKeybinding", "Change Keybinding...", true));
    }
    actions.push(action("keybindings.editor.addKeybinding", "Add Keybinding...", true));
    actions.extend([
        MenuAction::Separator,
        action("keybindings.editor.removeKeybinding", "Remove Keybinding", bound),
        action("keybindings.editor.resetKeybinding", "Reset Keybinding", !row.is_default),
        MenuAction::Separator,
        action("keybindings.editor.defineWhenExpression", "Change When Expression", bound),
        MenuAction::Separator,
        action("keybindings.editor.showConflicts", "Show Same Keybindings", bound),
    ]);
    actions
}

impl Workbench {
    pub(super) fn register_keybindings_editor_commands(&mut self) {
        let commands = &mut self.commands;
        commands.register(OPEN_COMMAND, |workbench, args, window, cx| {
            let query = match args.first() {
                Some(Value::String(query)) => Some(query.clone()),
                _ => None,
            };
            workbench.open_keybindings_editor(query, window, cx);
            Outcome::done()
        });
        commands.register(CLOSE_MODAL_COMMAND, |workbench, _, window, cx| {
            workbench.close_modal_editor(window, cx);
            Outcome::done()
        });
        commands.register("workbench.action.toggleModalEditorMaximized", |workbench, _, _, cx| {
            let open = workbench.keybindings_editor.open || workbench.settings_editor.open || workbench.modal_group_open();
            let editor = &mut workbench.keybindings_editor;
            editor.maximized = open && !editor.maximized;
            cx.notify();
            Outcome::done()
        });
        commands.register("keybindings.editor.searchKeybindings", |workbench, _, window, cx| {
            workbench.focus_keybindings_search(window, cx);
            Outcome::done()
        });
        commands.register("keybindings.editor.clearSearchResults", |workbench, _, window, cx| {
            workbench.set_keybindings_search(String::new(), window, cx);
            Outcome::done()
        });
        commands.register("keybindings.editor.recordSearchKeys", |workbench, _, window, cx| {
            workbench.set_recording_keys(true, window, cx);
            Outcome::done()
        });
        commands.register("keybindings.editor.toggleSortByPrecedence", |workbench, _, window, cx| {
            workbench.keybindings_editor.sort_by_precedence = !workbench.keybindings_editor.sort_by_precedence;
            workbench.render_keybindings_entries(false, false, window, cx);
            Outcome::done()
        });
        // ListResizeColumnAction: the last focused list is the keybindings table, the only table so far.
        commands.register("list.resizeColumn", |workbench, _, window, cx| {
            if workbench.keybindings_editor.table_focus.is_focused(window) {
                workbench.show_resize_column_pick(window, cx);
            }
            Outcome::done()
        });
        commands.register("keybindings.editor.focusKeybindings", |workbench, _, window, cx| {
            let editor = &mut workbench.keybindings_editor;
            if !editor.entries.is_empty() {
                editor.selected = Some(editor.selected.unwrap_or(0));
            }
            window.focus(&editor.table_focus.clone(), cx);
            Outcome::done()
        });
        commands.register("keybindings.editor.defineKeybinding", |workbench, _, window, cx| workbench.with_focused_entry(window, cx, |workbench, entry, window, cx| workbench.define_keybinding(entry, false, window, cx)));
        commands.register("keybindings.editor.addKeybinding", |workbench, _, window, cx| workbench.with_focused_entry(window, cx, |workbench, entry, window, cx| workbench.define_keybinding(entry, true, window, cx)));
        commands.register("keybindings.editor.defineWhenExpression", |workbench, _, window, cx| workbench.with_focused_entry(window, cx, Workbench::define_when_expression));
        commands.register("keybindings.editor.acceptWhenExpression", |workbench, _, window, cx| {
            workbench.accept_when_expression(window, cx);
            Outcome::done()
        });
        commands.register("keybindings.editor.rejectWhenExpression", |workbench, _, window, cx| {
            workbench.hide_when_input(window, cx);
            Outcome::done()
        });
        commands.register("keybindings.editor.removeKeybinding", |workbench, _, window, cx| {
            workbench.with_focused_entry(window, cx, |workbench, entry, window, cx| {
                if !entry.row.keybinding.is_empty() {
                    workbench.edit_keybindings_file(&entry, Change::Remove, window, cx);
                }
            })
        });
        commands.register("keybindings.editor.resetKeybinding", |workbench, _, window, cx| {
            workbench.with_focused_entry(window, cx, |workbench, entry, window, cx| {
                if entry.row.keybinding.is_empty() {
                    workbench.keybindings_editor.reveal_command = Some(entry.row.command.clone());
                }
                workbench.edit_keybindings_file(&entry, Change::Reset, window, cx);
            })
        });
        commands.register("keybindings.editor.copyKeybindingEntry", |workbench, _, window, cx| workbench.with_focused_entry(window, cx, |_, entry, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copied_entry(&entry)))));
        commands.register("keybindings.editor.copyCommandKeybindingEntry", |workbench, _, window, cx| workbench.with_focused_entry(window, cx, |_, entry, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(entry.row.command))));
        commands.register("keybindings.editor.copyCommandTitle", |workbench, _, window, cx| workbench.with_focused_entry(window, cx, |_, entry, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(entry.row.command_label))));
        commands.register("keybindings.editor.showConflicts", |workbench, _, window, cx| {
            workbench.with_focused_entry(window, cx, |workbench, entry, window, cx| {
                let value = format!("\"{}\"", keybindings::aria_label(&entry.row.keybinding));
                if value != workbench.keybindings_editor.search.value {
                    workbench.set_keybindings_search(value, window, cx);
                }
            })
        });
    }

    /// Runs `action` on the focused row of a shown editor; upstream's commands run on `activeKeybindingEntry`.
    fn with_focused_entry(&mut self, window: &mut Window, cx: &mut Context<Self>, action: impl FnOnce(&mut Self, Entry, &mut Window, &mut Context<Self>)) -> Outcome {
        if let Some(entry) = self.keybindings_editor.focused_entry().cloned().filter(|_| self.keybindings_editor.open) {
            action(self, entry, window, cx);
        }
        Outcome::done()
    }

    /// `TableColumnResizeQuickPick.show`: the columns with a label, by index.
    fn show_resize_column_pick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some("Select the column to resize, type to filter.".into());
        input.set_items(COLUMN_LABELS.iter().enumerate().map(|(index, label)| PickEntry::Item(Item { handle: index as i64 + 1, label: label.to_string(), pickable: true, ..Item::default() })).collect());
        let id = self.quick_inputs.add(input, Owner::ResizeColumn(None));
        self.show_quick_input(id, window, cx);
        self.flush_quick_input_events(window, cx);
    }

    pub(crate) fn resize_column_event(&mut self, id: i64, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        let Some(&Owner::ResizeColumn(column)) = self.quick_inputs.owner(id) else { return };
        match (event, column) {
            (Event::Accept(_), None) => {
                let Some(handle) = self.quick_inputs.get(id).and_then(|input| input.active_handles().first().copied()) else { return };
                self.hide_quick_input_by_id(id, window, cx);
                self.show_resize_column_input(handle as usize, window, cx);
            }
            (Event::ChangeValue(value), Some(_)) => {
                // quickInputService.input validates 100ms after typing stops.
                cx.spawn_in(window, async move |workbench, cx| {
                    cx.background_executor().timer(Duration::from_millis(100)).await;
                    let _ = workbench.update(cx, |workbench, cx| {
                        let Some(input) = workbench.quick_inputs.get_mut(id).filter(|input| input.input_box.value == value) else { return };
                        input.validation_message = column_resize_validation(&value);
                        input.severity = if input.validation_message.is_some() { Severity::Error } else { Severity::Ignore };
                        cx.notify();
                    });
                })
                .detach();
            }
            (Event::Accept(_), Some(column)) => {
                let Some(input) = self.quick_inputs.get_mut(id) else { return };
                let value = input.input_box.value.clone();
                if let Some(message) = column_resize_validation(&value) {
                    (input.validation_message, input.severity) = (Some(message), Severity::Error);
                    return cx.notify();
                }
                self.hide_quick_input_by_id(id, window, cx);
                // `if (!percentageValue)`: an empty value or 0 leaves the column.
                if let Some(percentage) = parse_int(&value).filter(|&percentage| percentage != 0) {
                    let columns = &mut self.keybindings_editor.columns;
                    columns.resize_view(column, (percentage as f32 / 100. * columns.size()).round());
                }
            }
            (Event::Hide, _) => self.quick_inputs.remove(id),
            _ => {}
        }
        cx.notify();
    }

    fn show_resize_column_input(&mut self, column: usize, window: &mut Window, cx: &mut Context<Self>) {
        let mut input = QuickInput::new(0, Kind::InputBox);
        input.placeholder = Some("i.e. 20, 60, 100...".into());
        input.prompt = Some(format!("Please enter a width in percentage for the '{}' column.", COLUMN_LABELS[column - 1]));
        let id = self.quick_inputs.add(input, Owner::ResizeColumn(Some(column)));
        self.show_quick_input(id, window, cx);
        self.flush_quick_input_events(window, cx);
    }

    /// `openGlobalKeybindingSettings(false, { query })`: shows the editor, focusing its search, and searches `query`.
    pub(super) fn open_keybindings_editor(&mut self, query: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.close_settings_editor(window, cx);
        self.close_modal_group(window, cx);
        let editor = &mut self.keybindings_editor;
        if !editor.open {
            editor.previous_focus = window.focused(cx);
            editor.open = true;
            editor.maximized = false;
            editor.sort_by_precedence = false;
            editor.recording = false;
            editor.recorded.clear();
            editor.selected = None;
            editor.columns = table_columns();
            editor.search = InputBox::default();
            editor.search_view.reset();
            self.refresh_keybindings_model();
            self.render_keybindings_entries(true, false, window, cx);
            self.focus_keybindings_search(window, cx);
        }
        let Some(query) = query else { return cx.notify() };
        // `search`: the value filters after the delay, with the first row selected and the table focused.
        self.focus_keybindings_search(window, cx);
        self.set_keybindings_search(query, window, cx);
        self.select_keybinding_entry(0, true, window, cx);
    }

    pub(super) fn close_keybindings_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let editor = &mut self.keybindings_editor;
        if !editor.open {
            return;
        }
        editor.open = false;
        if editor.define.as_ref().is_some_and(|define| define.row.is_some()) {
            editor.define = None;
        }
        editor.when = None;
        editor.filtering = None;
        editor.define_blur = None;
        editor.when_blur = None;
        editor.search_view.reset();
        editor.when_view.reset();
        if let Some(focus) = editor.previous_focus.take() {
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    fn refresh_keybindings_model(&mut self) {
        let registered: Vec<&str> = self.commands.ids().collect();
        let keybindings = &self.keybindings;
        self.keybindings_editor.model = Model::new(keybindings.resolver.keybindings(), &keybindings.defaults, &self.quick_access.menu, &registered);
        self.keybindings_editor.model_generation = keybindings.generation;
    }

    /// `onDidUpdateKeybindings`: the shown editor rebuilds its model, keeping the focused row.
    fn keybindings_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.keybindings_editor.open || self.keybindings_editor.model_generation == self.keybindings.generation {
            return;
        }
        self.refresh_keybindings_model();
        let keep_focus = !self.keybindings_editor.table_focus.is_focused(window);
        self.render_keybindings_entries(false, keep_focus, window, cx);
    }

    /// `renderKeybindingsEntries`: fetches the rows of the search and selects a row as upstream does.
    fn render_keybindings_entries(&mut self, reset: bool, preserve_focus: bool, window: &mut Window, cx: &mut Context<Self>) {
        let editor = &mut self.keybindings_editor;
        editor.entries = editor.model.fetch(&editor.search.value, editor.sort_by_precedence);
        editor.list.reset(editor.entries.len());
        editor.listed = editor.entries.len();
        editor.revealed = None;
        editor.hovered = None;
        let current = editor.selected.take();
        cx.notify();
        if reset {
            return;
        }
        if let Some(command) = editor.reveal_command.take() {
            if let Some(index) = editor.entries.iter().position(|entry| entry.row.command == command) {
                self.select_keybinding_entry(index, true, window, cx);
            }
            return;
        }
        match current {
            Some(index) if index < self.keybindings_editor.entries.len() => self.select_keybinding_entry(index, !preserve_focus, window, cx),
            _ if !preserve_focus => self.focus_keybindings_search(window, cx),
            _ => {}
        }
    }

    /// `selectEntry`: selects the row, focusing the table when `focus`.
    fn select_keybinding_entry(&mut self, index: usize, focus: bool, window: &mut Window, cx: &mut Context<Self>) {
        let editor = &mut self.keybindings_editor;
        if index >= editor.entries.len() {
            return;
        }
        editor.selected = Some(index);
        editor.unselected = false;
        if focus {
            window.focus(&editor.table_focus.clone(), cx);
        }
        cx.notify();
    }

    fn focus_keybindings_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focus = self.keybindings_editor.search_focus.clone();
        window.focus(&focus, cx);
        self.blink_caret(InputId::KeybindingsSearch, window, cx);
    }

    /// `searchWidget.setValue`: the value changes and filters after the delay.
    fn set_keybindings_search(&mut self, value: String, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_input_box(InputId::KeybindingsSearch, window, cx, |input| input.set_value(value));
    }

    /// Typing in the search: `onDidChange` filters 300ms later.
    pub(super) fn edit_keybindings_search(&mut self, edit: impl FnOnce(&mut InputBox), window: &mut Window, cx: &mut Context<Self>) {
        let editor = &mut self.keybindings_editor;
        let before = editor.search.value.clone();
        edit(&mut editor.search);
        cx.notify();
        if editor.search.value == before {
            return;
        }
        editor.filtering = Some(cx.spawn_in(window, async move |workbench, cx| {
            cx.background_executor().timer(Duration::from_millis(300)).await;
            let _ = workbench.update_in(cx, |workbench, window, cx| {
                let reset = workbench.keybindings_editor.search_focus.is_focused(window);
                workbench.render_keybindings_entries(reset, false, window, cx);
            });
        }));
    }

    pub(super) fn edit_when_input(&mut self, edit: impl FnOnce(&mut InputBox), cx: &mut Context<Self>) {
        if let Some(when) = self.keybindings_editor.when.as_mut() {
            edit(&mut when.input);
        }
        cx.notify();
    }

    /// The Record Keys toggle: the placeholder changes and the search takes the focus.
    fn set_recording_keys(&mut self, recording: bool, window: &mut Window, cx: &mut Context<Self>) {
        let editor = &mut self.keybindings_editor;
        editor.recording = recording;
        editor.recorded.clear();
        self.focus_keybindings_search(window, cx);
        cx.notify();
    }

    /// The key handlers of the recording inputs, after the modal's capture listener: a key that runs
    /// `closeModalEditor` closes the modal, others are recorded. Returns whether the key was used.
    pub(super) fn keybindings_editor_record_key(&mut self, chord: Chord, resolution: &Resolution, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let editor = &mut self.keybindings_editor;
        let closes = matches!(resolution, Resolution::KbFound { command, .. } if command == CLOSE_MODAL_COMMAND);
        let key = chord.key_aria_label();
        let plain = !chord.ctrl && !chord.shift && !chord.alt && !chord.meta;
        if editor.define.is_some() && editor.define_focus.is_focused(window) {
            if closes {
                return false;
            }
            match key.as_str() {
                "Enter" if plain => self.accept_define(window, cx),
                "Escape" if plain => self.clear_or_hide_define(window, cx),
                _ => self.record_define_chord(chord, cx),
            }
            return true;
        }
        if !editor.open {
            return false;
        }
        if editor.recording && editor.search_focus.is_focused(window) && !closes {
            if key == "Escape" && plain {
                // `onEscape` unchecks Record Keys.
                self.set_recording_keys(false, window, cx);
                return true;
            }
            record_chord(&mut editor.recorded, chord);
            let value = format!("\"{}\"", editor.recorded.iter().map(Chord::user_settings_label).collect::<Vec<_>>().join(" "));
            self.set_keybindings_search(value, window, cx);
            return true;
        }
        false
    }

    /// The modal's key handler: a key bound to a `workbench.*` command the modal does not allow does nothing while
    /// focus is in the modal, unless `workbench.editor.useModal` is `all`.
    pub(super) fn modal_blocks_command(&self, resolution: &Resolution, window: &Window) -> bool {
        let Resolution::KbFound { command, .. } = resolution else { return false };
        (self.keybindings_editor_focused(window) || self.settings_editor_focused(window) || self.modal_group_focused(window)) && command.starts_with("workbench.") && !MODAL_ALLOWED_COMMANDS.contains(&command.as_str()) && self.setting("workbench.editor.useModal") != json!("all")
    }

    fn keybindings_editor_focused(&self, window: &Window) -> bool {
        let editor = &self.keybindings_editor;
        editor.open && [&editor.modal_focus, &editor.search_focus, &editor.table_focus, &editor.define_focus, &editor.when_focus].into_iter().any(|focus| focus.is_focused(window))
    }

    /// The context keys of the modal part (`editorPartModal`) and the editor (`inKeybindings` and its widgets').
    pub(super) fn keybindings_editor_context(&self, window: &Window, context: &mut HashMap<String, Value>) {
        if !self.keybindings_editor_focused(window) {
            return;
        }
        let editor = &self.keybindings_editor;
        let search_focused = editor.search_focus.is_focused(window);
        let table_focused = editor.table_focus.is_focused(window);
        let when_focused = editor.when_focus.is_focused(window);
        context.insert("editorPartModal".into(), json!(true));
        context.insert("editorPartModalVisible".into(), json!(true));
        context.insert("editorPartModalMaximized".into(), json!(editor.maximized));
        context.insert("activeEditor".into(), json!("workbench.editor.keybindings"));
        context.insert("inKeybindings".into(), json!(true));
        context.insert("inKeybindingsSearch".into(), json!(search_focused));
        context.insert("keybindingsSearchHasValue".into(), json!(!editor.search.value.is_empty()));
        context.insert("keybindingFocus".into(), json!(table_focused && editor.selected.is_some()));
        context.insert("whenFocus".into(), json!(when_focused));
        context.insert("listFocus".into(), json!(table_focused));
        if search_focused || when_focused {
            context.insert("inputFocus".into(), json!(true));
        }
    }

    /// `defineKeybinding`: the define widget shows over the table and records the keys.
    fn define_keybinding(&mut self, entry: Entry, add: bool, window: &mut Window, cx: &mut Context<Self>) {
        let index = self.keybindings_editor.entries.iter().position(|shown| shown.row.id() == entry.row.id());
        if let Some(index) = index {
            self.select_keybinding_entry(index, true, window, cx);
        }
        self.show_define_widget(Some((entry, add)), window, cx);
    }

    /// `DefineKeybindingWidget.define`: the widget takes the focus and records until Enter, Escape or a blur.
    pub(super) fn show_define_widget(&mut self, row: Option<(Entry, bool)>, window: &mut Window, cx: &mut Context<Self>) {
        let editor = &mut self.keybindings_editor;
        editor.define = Some(Define { row, chords: Vec::new() });
        let focus = editor.define_focus.clone();
        window.focus(&focus, cx);
        // `onBlur` cancels.
        editor.define_blur = Some(cx.on_blur(&focus, window, |workbench, window, cx| workbench.hide_define(None, window, cx)));
        cx.notify();
    }

    fn record_define_chord(&mut self, chord: Chord, cx: &mut Context<Self>) {
        let Some(define) = self.keybindings_editor.define.as_mut() else { return };
        record_chord(&mut define.chords, chord);
        cx.notify();
    }

    fn accept_define(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chords = self.keybindings_editor.define.as_ref().map(|define| define.chords.clone()).unwrap_or_default();
        let key = (!chords.is_empty()).then(|| keybindings::user_settings_label(&chords));
        self.hide_define(key, window, cx);
    }

    /// `clearOrHide`: Escape clears the recorded keys, or hides the widget without any.
    fn clear_or_hide_define(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(define) = self.keybindings_editor.define.as_mut() else { return };
        if define.chords.is_empty() {
            return self.hide_define(None, window, cx);
        }
        define.chords.clear();
        cx.notify();
    }

    /// The widget hides: `key` is the keybinding to write, `None` cancels; the row is selected again.
    fn hide_define(&mut self, key: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let editor = &mut self.keybindings_editor;
        editor.define_blur = None;
        let Some(define) = editor.define.take() else { return };
        cx.notify();
        let Some((entry, add)) = define.row else { return self.define_overlay_accepted(key, window, cx) };
        if let Some(key) = key {
            self.update_keybinding(&entry, &key, add, window, cx);
        }
        if let Some(index) = self.keybindings_editor.entries.iter().position(|shown| shown.row.id() == entry.row.id()) {
            self.select_keybinding_entry(index, true, window, cx);
        }
    }

    /// `updateKeybinding`: writes only a changed key; a keybinding given to an unbound command is revealed.
    fn update_keybinding(&mut self, entry: &Entry, key: &str, add: bool, window: &mut Window, cx: &mut Context<Self>) {
        let row = &entry.row;
        if keybindings::user_settings_label(&row.keybinding) == key {
            return;
        }
        let when = (!row.when.is_empty()).then_some(row.when.as_str());
        if row.keybinding.is_empty() {
            self.keybindings_editor.reveal_command = Some(row.command.clone());
        }
        let change = if add { Change::Add { key, when } } else { Change::Edit { key, when } };
        self.edit_keybindings_file(entry, change, window, cx);
    }

    /// `defineWhenExpression`: a bound row's When column turns into an input holding its `when`.
    fn define_when_expression(&mut self, entry: Entry, window: &mut Window, cx: &mut Context<Self>) {
        if entry.row.keybinding.is_empty() {
            return;
        }
        if let Some(index) = self.keybindings_editor.entries.iter().position(|shown| shown.row.id() == entry.row.id()) {
            self.select_keybinding_entry(index, true, window, cx);
        }
        let editor = &mut self.keybindings_editor;
        let mut input = InputBox::new(entry.row.when.clone());
        input.select_all();
        editor.when = Some(WhenEdit { entry, input });
        editor.when_view.reset();
        let focus = editor.when_focus.clone();
        window.focus(&focus, cx);
        editor.when_blur = Some(cx.on_blur(&focus, window, |workbench, window, cx| workbench.hide_when_input(window, cx)));
        self.blink_caret(InputId::KeybindingsWhen, window, cx);
        cx.notify();
    }

    /// `onDidAccept`: a changed `when` is written with the row's keybinding.
    fn accept_when_expression(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let editor = &mut self.keybindings_editor;
        editor.when_blur = None;
        let Some(when) = editor.when.take() else { return };
        let row = &when.entry.row;
        let value = when.input.value.trim();
        if value != row.when {
            let key = keybindings::user_settings_label(&row.keybinding);
            let new_when = (!value.is_empty()).then_some(value);
            self.edit_keybindings_file(&when.entry, Change::Edit { key: &key, when: new_when }, window, cx);
        }
        self.focus_entry_of(&when.entry, window, cx);
    }

    /// `onDidReject`: the input goes, the row keeps the focus.
    fn hide_when_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let editor = &mut self.keybindings_editor;
        editor.when_blur = None;
        let Some(when) = editor.when.take() else { return };
        editor.when_view.reset();
        cx.notify();
        if editor.when_focus.is_focused(window) {
            self.focus_entry_of(&when.entry, window, cx);
        }
    }

    fn focus_entry_of(&mut self, entry: &Entry, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.keybindings_editor.entries.iter().position(|shown| shown.row.id() == entry.row.id()) {
            self.select_keybinding_entry(index, true, window, cx);
        }
    }

    /// `KeybindingsEditingService`: writes the change to keybindings.json, refusing while an open editor of it has
    /// unsaved changes, and reloads the keybindings. Errors show as `onKeybindingEditingError` does.
    fn edit_keybindings_file(&mut self, entry: &Entry, change: Change, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(error) = self.write_keybindings_file(entry, change, cx) {
            self.keybindings_editor.reveal_command = None;
            if !self.skip_dialogs {
                drop(window.prompt(PromptLevel::Critical, &error, None, &["OK"], cx));
            }
            return;
        }
        self.keybindings_changed(window, cx);
    }

    fn write_keybindings_file(&mut self, entry: &Entry, change: Change, cx: &mut Context<Self>) -> Result<(), String> {
        let Some(file) = self.keybindings.user_file.clone() else { return Err("no keybindings.json".into()) };
        let open_editor = self.editors.find(&file);
        if open_editor.is_some_and(|index| self.editors.editors[index].is_dirty()) {
            return Err(DIRTY_FILE_ERROR.into());
        }
        let content = std::fs::read_to_string(&file).ok();
        let options = config::service::formatting_options(content.as_deref().unwrap_or_default(), &self.settings.lock().unwrap().configuration, &file);
        let edited = keybinding_editing::apply(content.as_deref(), &entry.row, change, &options)?;
        let written = config::service::write_user_data_file(&file, edited.as_bytes());
        written.map_err(|error| format!("Error '{error}' while editing the keybinding. Please open 'keybindings.json' file and check for errors."))?;
        // The open editor's text model is the file's: it shows the edit.
        if let Some(index) = open_editor {
            self.revert_editor(index, cx);
        }
        self.set_user_keybindings(keybindings::user_keybindings(&file));
        Ok(())
    }

    fn show_keybinding_context_menu(&mut self, index: usize, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.select_keybinding_entry(index, true, window, cx);
        let actions = context_menu_actions(&self.keybindings_editor.entries[index]);
        let context = self.key_context(window);
        let anchor = gpui::Bounds { origin: event.position, size: gpui::Size::default() };
        self.show_context_menu_actions(actions, &context, anchor, Vec::new(), window, cx);
    }

    /// The list commands on the table, named by their default key.
    /// listCommands.ts on the keybindings table: the key each list command stands for, when the table has focus.
    /// The table is a list, not a tree: the tree commands do nothing on it, the `focusAny` moves are the focus moves.
    pub(super) fn keybindings_table_list_command(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.keybindings_editor.table_focus.is_focused(window) {
            return false;
        }
        let keys = [
            ("list.select", "enter"),
            ("list.focusDown", "down"),
            ("list.focusAnyDown", "down"),
            ("list.focusUp", "up"),
            ("list.focusAnyUp", "up"),
            ("list.focusPageDown", "pagedown"),
            ("list.focusPageUp", "pageup"),
            ("list.focusFirst", "home"),
            ("list.focusAnyFirst", "home"),
            ("list.focusLast", "end"),
            ("list.focusAnyLast", "end"),
        ];
        if let Some((_, key)) = keys.into_iter().find(|(list_id, _)| *list_id == id) {
            self.keybindings_table_key(key, window, cx);
        } else if id == "list.toggleSelection" {
            let editor = &mut self.keybindings_editor;
            editor.unselected = editor.selected.is_some() && !editor.unselected;
            cx.notify();
        } else if id == "list.selectAndPreserveFocus" {
            self.keybindings_editor.unselected = false;
            cx.notify();
        }
        true
    }

    /// keybindingsEditor.ts `keyboardNavigationLabelProvider`: each row's command label, else its command id; and the
    /// focused row.
    pub(super) fn keybindings_table_navigation_labels(&self) -> (Vec<String>, Option<usize>) {
        let editor = &self.keybindings_editor;
        let labels = editor.entries.iter().map(|entry| if entry.row.command_label.is_empty() { entry.row.command.clone() } else { entry.row.command_label.clone() }).collect();
        (labels, editor.selected)
    }

    pub(super) fn focus_keybinding_entry(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.select_keybinding_entry(index, true, window, cx);
    }

    fn keybindings_table_key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let editor = &self.keybindings_editor;
        let count = editor.entries.len();
        if count == 0 {
            return;
        }
        let current = editor.selected;
        // `list.select` opens the focused row, and the table's `onDidOpen` defines its keybinding.
        if key == "enter" {
            if let Some(entry) = current.map(|index| editor.entries[index].clone()) {
                self.define_keybinding(entry, false, window, cx);
            }
            return;
        }
        let page = (f32::from(editor.list.viewport_bounds().size.height) / 24.).floor().max(1.) as usize;
        let index = match key {
            "down" => current.map_or(0, |index| (index + 1).min(count - 1)),
            "up" => current.map_or(0, |index| index.saturating_sub(1)),
            "pagedown" => current.map_or(0, |index| (index + page).min(count - 1)),
            "pageup" => current.map_or(0, |index| index.saturating_sub(page)),
            "home" => 0,
            "end" => count - 1,
            _ => return,
        };
        self.select_keybinding_entry(index, true, window, cx);
    }

    /// The modal editor part: the dimmed window, the dialog with its header, and the Keyboard Shortcuts editor.
    pub(super) fn keybindings_editor_widget(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.keybindings_editor.open {
            return None;
        }
        let header = ModalHeader { icon: "record-keys", title: "Keyboard Shortcuts", json_title: "Open Keyboard Shortcuts (JSON)", json_command: "workbench.action.openGlobalKeybindingsFile", resource: None };
        let focus = self.keybindings_editor.modal_focus.clone();
        Some(self.modal_editor_part(header, &focus, window, cx, |workbench, width, height, window, cx| workbench.keybindings_editor_pane(width, height, window, cx).into_any_element()))
    }

    /// The modal editor part around `editor`, which gets the size inside the dialog's border and header.
    pub(super) fn modal_editor_part(
        &mut self,
        header: ModalHeader,
        focus: &FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
        editor: impl FnOnce(&mut Self, f32, f32, &mut Window, &mut Context<Self>) -> AnyElement,
    ) -> AnyElement {
        let viewport = window.viewport_size();
        let (window_width, window_height) = (f32::from(viewport.width), f32::from(viewport.height));
        let available_height = (window_height - TITLE_BAR_HEIGHT).max(0.);
        let (width, height) = match self.keybindings_editor.maximized {
            true => ((window_width - MODAL_MAXIMIZED_PADDING).max(0.), (available_height - TITLE_BAR_HEIGHT.max(MODAL_MAXIMIZED_PADDING)).max(0.)),
            false => ((window_width * 0.8).min(MODAL_MAX_WIDTH), (available_height * 0.8).min(MODAL_MAX_HEIGHT)),
        };
        let height = height.min(available_height);
        let left = ((window_width - width) / 2.).round();
        let top = ((window_height - height) / 2.).max(TITLE_BAR_HEIGHT).round();
        let editor_height = height - 2. - MODAL_HEADER_HEIGHT - 1.;
        let editor = editor(self, width - 2., editor_height, window, cx);
        let modal = div()
            .id("modal-editor")
            .track_focus(focus)
            .absolute()
            .left(px(left))
            .top(px(top))
            .w(px(width))
            .h(px(height))
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(px(8.))
            .border_1()
            .border_color(self.theme.color("editorWidget.border").or(self.theme.color("contrastBorder")).unwrap_or(transparent()))
            .bg(self.color("editor.background"))
            // --vscode-shadow-xl blurs 20px; GPUI's blur_radius is the sigma, half the CSS blur.
            .shadow(vec![BoxShadow { color: rgba(0x00000026).into(), offset: point(px(0.), px(0.)), blur_radius: px(10.), spread_radius: px(0.), inset: false }])
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(self.modal_header(header, cx))
            .child(editor);
        let block = div()
            .id("modal-editor-block")
            .absolute()
            .inset_0()
            .bg(rgba(0x00000080))
            .occlude()
            // A press outside the dialog closes the modal.
            .on_mouse_down(MouseButton::Left, cx.listener(|workbench, _, window, cx| workbench.close_modal_editor(window, cx)))
            .child(modal);
        block.into_any_element()
    }

    /// `workbench.action.closeModalEditor`: the modal part closes with the editor it shows.
    pub(super) fn close_modal_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_keybindings_editor(window, cx);
        self.close_settings_editor(window, cx);
        self.close_profiles_editor(cx);
        self.close_workspace_trust_editor(cx);
        self.close_modal_group(window, cx);
    }

    /// `.modal-editor-header`: the editor's icon and title, and the `ModalEditorTitle` actions.
    fn modal_header(&self, header: ModalHeader, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let foreground = self.foreground_or("titleBar.activeForeground");
        let action = |name: &'static str, title: &'static str, command: &'static str| {
            self.icon(name, 22., 16., foreground)
                .id(command)
                .rounded(px(5.))
                .cursor_pointer()
                .hover(|item| item.bg(self.color("toolbar.hoverBackground")))
                .child(self.action_hover(title, command, super::hover::HoverOptions::mouse()))
                .on_click(cx.listener(move |workbench, _, window, cx| {
                    let outcome = workbench.execute_command(command, Vec::new(), window, cx);
                    super::keyboard::report_failure(outcome, cx);
                }))
        };
        let maximize_title = if self.keybindings_editor.maximized { "Restore Modal Editor" } else { "Maximize Modal Editor" };
        div()
            .id("modal-editor-header")
            // modalEditorPart.css: 32px plus the 1px bottom border (content-box).
            .h(px(MODAL_HEADER_HEIGHT + 1.))
            .flex_none()
            .flex()
            .items_center()
            .pl(px(10.))
            .pr(px(8.))
            .bg(self.color("titleBar.activeBackground"))
            .text_color(foreground)
            .border_b_1()
            .border_color(self.theme.color("titleBar.border").unwrap_or(transparent()))
            .on_click(cx.listener(|workbench, event: &gpui::ClickEvent, _, cx| {
                // A double click on the header toggles maximized.
                if event.click_count() == 2 {
                    workbench.keybindings_editor.maximized = !workbench.keybindings_editor.maximized;
                    cx.notify();
                }
            }))
            .map(|row| match header.resource {
                Some(resource) => row.child(div().flex_1().min_w_0().flex().items_center().text_size(px(12.)).child(resource)),
                None => row.child(self.icon(header.icon, 16., 16., foreground).mr(px(6.))).child(div().flex_1().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).truncate().child(header.title)),
            })
            // `ModalEditorEditorTitle`, then a separator and `ModalEditorTitle`, 4px apart.
            // The Profiles editor has no JSON action and no separator.
            .when(!header.json_command.is_empty(), |row| {
                row.child(action("go-to-file", header.json_title, header.json_command)).child(div().mx(px(4.)).w(px(1.)).h(px(16.)).bg(foreground).opacity(0.3))
            })
            // shortcut: Open Modal Editor in Main Window is drawn disabled: the port's editor area only hosts text editors; upgrade with the editor pane registry.
            .child(self.icon("open-in-product", 22., 16., foreground).opacity(0.4))
            .child(action(if self.keybindings_editor.maximized { "screen-normal" } else { "screen-full" }, maximize_title, "workbench.action.toggleModalEditorMaximized").ml(px(4.)))
            .child(action("close", "Close Modal Editor", CLOSE_MODAL_COMMAND).ml(px(4.)))
    }

    /// `.keybindings-editor`: the header with the search and the table, the define widget over them.
    fn keybindings_editor_pane(&mut self, width: f32, height: f32, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let table_width = width - 27.;
        self.keybindings_changed(window, cx);
        let define = self.define_widget_shown(false).then(|| self.define_keybinding_widget(width, height, window, cx));
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .pt(px(11.))
            .pl(px(27.))
            .text_size(px(13.))
            .text_color(self.color("foreground"))
            .child(div().pr(px(10.)).pb(px(11.)).child(self.keybindings_search(window, cx)))
            .child(self.keybindings_table(table_width, window, cx))
            .children(define)
    }

    /// The `KeybindingsSearchWidget` with its Record Keys and Sort by Precedence toggles and the Clear action.
    fn keybindings_search(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let editor = &self.keybindings_editor;
        let focused = editor.search_focus.is_focused(window);
        let border = if focused { self.color("focusBorder") } else { self.theme.color("settings.textInputBorder").or(self.theme.color("input.border")).unwrap_or(transparent()) };
        let placeholder = if editor.recording { RECORDING_PLACEHOLDER } else { SEARCH_PLACEHOLDER };
        let has_value = !editor.search.value.is_empty();
        let foreground = self.foreground_or("icon.foreground");
        let toggle = |icon: &'static str, title: &'static str, command: &'static str, checked: bool| {
            // 22x22 in the reference, where toggle.css alone gives 20 wide.
            self.icon(icon, 22., 16., if checked { self.foreground_or("inputOption.activeForeground") } else { foreground })
                .id(command)
                .mr(px(4.))
                .rounded(px(3.))
                .border_1()
                .border_color(if checked { self.color("inputOption.activeBorder") } else { transparent() })
                .cursor_pointer()
                .when(checked, |toggle| toggle.bg(self.color("inputOption.activeBackground")))
                .when(!checked, |toggle| toggle.hover(|toggle| toggle.bg(self.color("inputOption.hoverBackground"))))
                .child(self.action_hover(title, command, super::hover::HoverOptions::mouse()))
                .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
        };
        let recording = editor.recording;
        let actions = div()
            .absolute()
            // margin-top 4px from the input's outer border; this box's children start inside its 1px border.
            .top(px(3.))
            .right(px(10.))
            .flex()
            .items_center()
            .when(recording, |actions| {
                // `.recording-badge.monaco-count-badge.long` under modern UI: `fontSize.label3` 10px, `cornerRadius.small`,
                // `line-height: normal` (22px tall in the reference); the border only with `contrastBorder` (countBadge.ts).
                let border = self.theme.color("contrastBorder");
                actions.child(
                    div()
                        .mr(px(8.))
                        .p(px(4.))
                        .rounded(px(4.))
                        .text_size(px(10.))
                        .line_height(px(14.))
                        .bg(self.color("badge.background"))
                        .text_color(self.color("badge.foreground"))
                        .when_some(border, |badge, border| badge.border_1().border_color(border))
                        .child("Recording Keys"),
                )
            })
            .child(toggle("record-keys", "Record Keys", "keybindings.editor.recordSearchKeys", recording).on_click(cx.listener(|workbench, _, window, cx| {
                let recording = !workbench.keybindings_editor.recording;
                workbench.set_recording_keys(recording, window, cx);
            })))
            .child(toggle("sort-precedence", "Sort by Precedence (Highest first)", "keybindings.editor.toggleSortByPrecedence", editor.sort_by_precedence).on_click(cx.listener(|workbench, _, window, cx| {
                workbench.keybindings_editor.sort_by_precedence = !workbench.keybindings_editor.sort_by_precedence;
                workbench.render_keybindings_entries(false, false, window, cx);
                workbench.focus_keybindings_search(window, cx);
            })))
            .child(
                self.icon("clear-all", 20., 16., foreground)
                    .id("keybindings.editor.clearSearchResults")
                    .h(px(22.))
                    .mr(px(4.))
                    .rounded(px(5.))
                    .when(!has_value, |clear| clear.opacity(0.4))
                    .when(has_value, |clear| {
                        clear.cursor_pointer().hover(|clear| clear.bg(self.color("toolbar.hoverBackground"))).on_click(cx.listener(|workbench, _, window, cx| {
                            workbench.set_keybindings_search(String::new(), window, cx);
                            workbench.focus_keybindings_search(window, cx);
                        }))
                    })
                    .child(self.action_hover("Clear Keybindings Search Input", "keybindings.editor.clearSearchResults", super::hover::HoverOptions::mouse())),
            );
        div()
            .relative()
            .h(px(30.))
            .flex()
            .rounded(px(4.))
            .bg(self.color("input.background"))
            .border_1()
            .border_color(border)
            // The ibwrapper has no height, so the input is 4px padding, a 17px line and 4px tall at the box's top.
            .child(div().flex_1().h(px(25.)).pl(px(10.)).mr(px(if recording { 178. } else { 88. })).flex().text_size(px(14.)).child(self.input_text(
                InputId::KeybindingsSearch,
                InputLook { password: false, placeholder: Some(placeholder.into()), focused },
                cx,
            )))
            .child(actions)
    }

    /// The `WorkbenchTable` of the rows: a 30px header and rows 24px tall, or 40px with a matched id on a second line.
    fn keybindings_table(&mut self, width: f32, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let editor = &mut self.keybindings_editor;
        if editor.listed != editor.entries.len() {
            editor.list.reset(editor.entries.len());
            editor.listed = editor.entries.len();
        }
        if editor.revealed != editor.selected {
            if let Some(index) = editor.selected {
                editor.list.scroll_to_reveal_item(index);
            }
            editor.revealed = editor.selected;
        }
        if editor.columns.size() != width {
            editor.columns.layout(width);
        }
        let columns: [f32; 5] = editor.columns.sizes().try_into().expect("the table has five columns");
        let sash_states = editor.columns.sash_states();
        let border = self.table_columns_border(window);
        let header = div()
            .h(px(TABLE_HEADER_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .font_weight(FontWeight::BOLD)
            .bg(self.color("keybindingTable.headerBackground"))
            .child(div().w(px(columns[0])).pl(px(20.)))
            .children(COLUMN_LABELS.into_iter().zip(&columns[1..]).map(|(title, width)| div().w(px(*width)).pl(px(10.)).truncate().child(title)));
        let rows = list(self.keybindings_editor.list.clone(), cx.processor(move |workbench: &mut Workbench, index: usize, window, cx| workbench.keybinding_row(index, columns, window, cx))).size_full();
        let mut sashes = Vec::new();
        let mut x = 0.;
        for (index, state) in sash_states.into_iter().enumerate() {
            x += columns[index];
            sashes.push(self.column_sash(index, x, state, border, cx));
        }
        let focus = self.keybindings_editor.table_focus.clone();
        div()
            .id("keybindings-table")
            .relative()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .track_focus(&focus)
            .on_hover(cx.listener(|workbench, hovered: &bool, _, cx| {
                workbench.keybindings_editor.table_hovered = (*hovered, Instant::now());
                cx.notify();
            }))
            .child(header)
            .child(div().relative().flex_1().min_h_0().border_t_1().border_color(border).child(rows).child(self.list_scroll_layer(super::list_scroll::ListId::KeybindingsTable, cx)))
            .children(sashes)
            .when(self.keybindings_editor.columns.dragged_sash().is_some(), |table| table.child(self.column_sash_drag(cx)))
    }

    /// `tree.tableColumnsBorder` while the pointer is over the table, fading in and out over 200ms; always with
    /// reduced motion (listWidget.ts `.monaco-enable-motion`).
    fn table_columns_border(&self, window: &mut Window) -> Rgba {
        let border = self.theme.color("tree.tableColumnsBorder").unwrap_or(transparent());
        if self.is_motion_reduced() {
            return border;
        }
        let (hovered, since) = self.keybindings_editor.table_hovered;
        let progress = (since.elapsed().as_secs_f32() / TABLE_BORDER_FADE.as_secs_f32()).min(1.);
        if progress < 1. {
            window.request_animation_frame();
        }
        let shown = ease_out(progress);
        Rgba { a: border.a * if hovered { shown } else { 1. - shown }, ..border }
    }

    /// The sash after column `index`: 4px wide over the boundary at `x`, its 1px border below the header in
    /// `sash.hoverBorder` under the column border once hovered or dragged. A double click resets the column to its
    /// weight (`onDidSashReset`).
    fn column_sash(&self, index: usize, x: f32, state: SashState, border: Rgba, cx: &mut Context<Self>) -> AnyElement {
        let editor = &self.keybindings_editor;
        let active = editor.sash_hovered == Some(index) || editor.columns.dragged_sash() == Some(index);
        let hover_border = self.theme.color("sash.hoverBorder").unwrap_or(self.color("focusBorder"));
        let line = |color: Rgba| div().absolute().left(px(SASH_SIZE / 2.)).top(px(TABLE_HEADER_HEIGHT + 1.)).bottom_0().w(px(1.)).bg(color);
        let sash = div()
            .id(("column-sash", index))
            .absolute()
            .top_0()
            .bottom_0()
            .left(px(x - SASH_SIZE / 2.))
            .w(px(SASH_SIZE))
            .when(active, |sash| sash.child(line(hover_border)))
            .child(line(border));
        if state == SashState::Disabled {
            return sash.into_any_element();
        }
        sash.cursor(match state {
            SashState::AtMinimum => gpui::CursorStyle::ResizeRight,
            SashState::AtMaximum => gpui::CursorStyle::ResizeLeft,
            _ => gpui::CursorStyle::ResizeColumn,
        })
        .on_hover(cx.listener(move |workbench, hovered: &bool, window, cx| {
            let delay = workbench.sash_hover_delay();
            let editor = &mut workbench.keybindings_editor;
            editor.sash_hovered = None;
            editor.sash_hover = hovered.then(|| {
                cx.spawn_in(window, async move |workbench, cx| {
                    cx.background_executor().timer(delay).await;
                    let _ = workbench.update(cx, |workbench, cx| {
                        workbench.keybindings_editor.sash_hovered = Some(index);
                        cx.notify();
                    });
                })
            });
            cx.notify();
        }))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |workbench, event: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                let editor = &mut workbench.keybindings_editor;
                if event.click_count == 2 {
                    let width = editor.columns.size();
                    editor.columns.resize_view(index, column_weight(index) / COLUMN_WEIGHTS.iter().sum::<f32>() * width);
                } else {
                    editor.columns.start_sash_drag(index, f32::from(event.position.x), event.modifiers.alt);
                }
                cx.notify();
            }),
        )
        .into_any_element()
    }

    /// Follows the pointer anywhere in the window while a sash drags.
    fn column_sash_drag(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let workbench = cx.entity();
        canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let moved = workbench.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, _, _, cx| {
                    moved.update(cx, |workbench, cx| {
                        workbench.keybindings_editor.columns.drag_sash(f32::from(event.position.x), event.modifiers.alt);
                        cx.notify();
                    });
                });
                let released = workbench.clone();
                window.on_mouse_event(move |_: &MouseUpEvent, _, _, cx| {
                    released.update(cx, |workbench, cx| {
                        workbench.keybindings_editor.columns.end_sash_drag();
                        cx.notify();
                    });
                });
            },
        )
        .size_0()
    }

    /// A `.monaco-list-row` of the table.
    fn keybinding_row(&mut self, index: usize, columns: [f32; 5], window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let editor = &self.keybindings_editor;
        let Some(entry) = editor.entries.get(index) else { return div().into_any_element() };
        let table_focused = editor.table_focus.is_focused(window) || editor.when_focus.is_focused(window);
        let focused = editor.selected == Some(index);
        let selected = focused && !editor.unselected;
        let hovered = editor.hovered == Some(index);
        let (background, foreground) = match (selected, table_focused) {
            (true, true) => (Some(self.color("list.activeSelectionBackground")), self.foreground_or("list.activeSelectionForeground")),
            (true, false) => (Some(self.color("list.inactiveSelectionBackground")), self.foreground_or("list.inactiveSelectionForeground")),
            (false, _) if hovered => (Some(self.color("list.hoverBackground")), self.foreground_or("list.hoverForeground")),
            // `data-parity=odd`: every second row takes keybindingTable.rowsBackground.
            (false, _) if index % 2 == 1 => (Some(self.color("keybindingTable.rowsBackground")), self.color("foreground")),
            _ => (None, self.color("foreground")),
        };
        let outline = self.theme.color("list.focusOutline").filter(|_| focused && table_focused);
        let row_active = selected && table_focused;
        let highlight = if row_active { foreground } else { self.color("list.highlightForeground") };
        let height = row_height(entry);
        let when_edit = editor.when.as_ref().filter(|when| when.entry.row.id() == entry.row.id()).is_some();
        let show_action = selected || hovered;
        let row = &entry.row;
        let action = match row.keybinding.is_empty() {
            false => ("edit", "Change Keybinding", false),
            true => ("add", "Add Keybinding", true),
        };
        let action_icon = self.icon(action.0, 16., 16., foreground).id(("keybinding-action", index)).cursor_pointer().on_click(cx.listener(move |workbench, _, window, cx| {
            if let Some(entry) = workbench.keybindings_editor.entries.get(index).cloned() {
                workbench.define_keybinding(entry, action.2, window, cx);
            }
        }));
        let code_keys = match (selected, table_focused) {
            (true, true) => Some(("list.activeSelectionForeground", "list.activeSelectionBackground")),
            (true, false) => Some(("list.inactiveSelectionForeground", "list.inactiveSelectionBackground")),
            (false, true) if hovered => Some(("list.hoverForeground", "list.hoverBackground")),
            _ => None,
        };
        let code_foreground = code_keys
            .and_then(|(foreground, background)| Some(code_color(self.theme.color(foreground)?, self.theme.color(background)?)))
            .unwrap_or_else(|| code_color(self.color("foreground"), workbench_background(self.theme.kind)));
        let command = self.command_cell(entry, highlight, code_foreground);
        let keybinding = self.keybinding_cell(entry, row_active, highlight);
        let when = match when_edit {
            true => self.when_input(window, cx).into_any_element(),
            false => self.when_cell(entry, highlight, code_foreground).into_any_element(),
        };
        let source = self.source_cell(entry, row_active, highlight);
        div()
            .id(("keybinding-row", index))
            .h(px(height))
            .flex()
            .items_center()
            .text_color(foreground)
            .when_some(background, |row, background| row.bg(background))
            .when_some(outline, |row, outline| row.border_1().border_color(outline))
            .on_hover(cx.listener(move |workbench, hovered: &bool, _, cx| {
                let editor = &mut workbench.keybindings_editor;
                if *hovered {
                    editor.hovered = Some(index);
                } else if editor.hovered == Some(index) {
                    editor.hovered = None;
                }
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| {
                workbench.select_keybinding_entry(index, true, window, cx);
                // `onDidOpen` of a double click defines the keybinding.
                if event.click_count == 2
                    && let Some(entry) = workbench.keybindings_editor.entries.get(index).cloned()
                {
                    workbench.define_keybinding(entry, false, window, cx);
                }
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(move |workbench, event, window, cx| workbench.show_keybinding_context_menu(index, event, window, cx)))
            .child(div().w(px(columns[0])).flex_none().pl(px(20.)).when(show_action, |cell| cell.child(action_icon.child(self.action_hover(action.1, "keybindings.editor.defineKeybinding", super::hover::HoverOptions::mouse())))))
            .child(div().w(px(columns[1])).flex_none().h_full().pl(px(10.)).flex().items_center().overflow_hidden().child(command))
            .child(div().w(px(columns[2])).flex_none().h_full().pl(px(10.)).flex().items_center().overflow_hidden().child(keybinding))
            .child(div().w(px(columns[3])).flex_none().h_full().pl(px(10.)).flex().items_center().overflow_hidden().child(when))
            .child(div().w(px(columns[4])).flex_none().h_full().pl(px(10.)).flex().items_center().overflow_hidden().child(source))
            .into_any_element()
    }

    /// `CommandColumnRenderer`: the title, with the command id on a second line when the search matched it or the
    /// command has no title.
    fn command_cell(&self, entry: &Entry, highlight: Rgba, foreground: Rgba) -> Div {
        let row = &entry.row;
        let id_matched = entry.matches.command_id.is_some();
        let mut cell = div().w_full().flex().overflow_hidden();
        cell = if id_matched { cell.flex_col().justify_center() } else { cell.items_center() };
        if !row.command_label.is_empty() {
            cell = cell.child(highlighted(&row.command_label, entry.matches.command_label.as_deref().unwrap_or_default(), highlight));
        }
        if id_matched || row.command_label.is_empty() {
            let id = highlighted(&row.command, entry.matches.command_id.as_deref().unwrap_or_default(), highlight);
            cell = cell.child(id.font_family("Menlo").text_size(px(13. * 0.9)).text_color(foreground));
        }
        cell
    }

    /// `KeybindingColumnRenderer`: the `KeybindingLabel` with the matched keys highlighted.
    fn keybinding_cell(&self, entry: &Entry, row_active: bool, highlight: Rgba) -> Div {
        let matches = entry.matches.keybinding.unwrap_or_default();
        div().flex().items_center().children(entry.row.keybinding.iter().enumerate().map(|(index, chord)| {
            let matched = matches_of(chord, matches[index.min(1)]);
            div().flex().when(index > 0, |chord| chord.ml(px(5.))).children(chord.key_caps().into_iter().zip(matched).map(|(label, matched)| {
                self.key_cap(&label, row_active).when(matched, |key| key.font_weight(FontWeight::BOLD).text_color(highlight))
            }))
        }))
    }

    /// `WhenColumnRenderer`: `-` without a `when`, else the expression in the monospace font.
    fn when_cell(&self, entry: &Entry, highlight: Rgba, code_foreground: Rgba) -> Div {
        let when = &entry.row.when;
        if when.is_empty() {
            return div().pl(px(4.)).child("-");
        }
        let when = highlighted(when, entry.matches.when.as_deref().unwrap_or_default(), highlight);
        div().w_full().font_family("Menlo").text_size(px(13. * 0.9)).text_color(code_foreground).child(when)
    }

    /// The `WhenInputWidget` in the When column.
    fn when_input(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let focused = self.keybindings_editor.when_focus.is_focused(window);
        div()
            .ml(px(0.))
            .mr(px(4.))
            .w_full()
            .h(px(22.))
            .flex()
            .px(px(4.))
            .bg(self.color("input.background"))
            .border_1()
            .border_color(if focused { self.color("focusBorder") } else { self.theme.color("input.border").unwrap_or(transparent()) })
            .font_family("Menlo")
            .text_size(px(13. * 0.9))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(self.input_text(InputId::KeybindingsWhen, InputLook { password: false, placeholder: None, focused }, cx))
    }

    /// `SourceColumnRenderer`: User or System, or the contributing extension's name as a link.
    fn source_cell(&self, entry: &Entry, row_active: bool, highlight: Rgba) -> Div {
        match &entry.row.source {
            Source::Contributed(extension) => {
                let name = extension.display_name.clone().unwrap_or_else(|| extension.id.clone());
                let link = if row_active { None } else { self.theme.color("textLink.foreground") };
                // shortcut: the name does not open the extension: the port has no Extensions view; upgrade with milestone 13.
                let mut cell = div().w_full().flex().overflow_hidden();
                cell = if entry.matches.extension_id.is_some() { cell.flex_col().justify_center() } else { cell.items_center() };
                cell.child(highlighted(&name, entry.matches.extension_label.as_deref().unwrap_or_default(), highlight).when_some(link, |name, link| name.text_color(link)))
                    .when(entry.matches.extension_id.is_some(), |cell| cell.child(highlighted(&extension.id, entry.matches.extension_id.as_deref().unwrap_or_default(), highlight)))
            }
            source => highlighted(source.label().unwrap_or_default(), entry.matches.source.as_deref().unwrap_or_default(), highlight),
        }
    }

    /// Whether the define widget shows over the user keybindings.json (`over_file`) or over the keybindings editor.
    pub(super) fn define_widget_shown(&self, over_file: bool) -> bool {
        self.keybindings_editor.define.as_ref().is_some_and(|define| define.row.is_none() == over_file)
    }

    /// `DefineKeybindingWidget`, centered over the editor; only the keybindings editor calls `printExisting`.
    pub(super) fn define_keybinding_widget(&self, width: f32, height: f32, window: &Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let define = self.keybindings_editor.define.as_ref().expect("checked by define_widget_shown");
        let focused = self.keybindings_editor.define_focus.is_focused(window);
        let value = define.chords.iter().map(Chord::user_settings_label).collect::<Vec<_>>().join(" ");
        let existing = match define.chords.is_empty() || define.row.is_none() {
            true => 0,
            false => self.keybindings_editor.model.fetch(&format!("\"{value}\""), false).len(),
        };
        let existing_text = match existing {
            0 => None,
            1 => Some("1 existing command has this keybinding".to_string()),
            count => Some(format!("{count} existing commands have this keybinding")),
        };
        let mut output = div().mt(px(10.)).w(px(DEFINE_WIDTH)).h(px(18.)).flex().justify_center().items_center();
        for (index, chord) in define.chords.iter().enumerate() {
            if index > 0 {
                output = output.child("chord to");
            }
            output = output.child(div().mx(px(4.)).flex().children(chord.key_caps().into_iter().map(|label| self.key_cap(&label, false))));
        }
        let input_border = if focused { self.color("focusBorder") } else { self.theme.color("input.border").unwrap_or(transparent()) };
        let input = div()
            .mt(px(10.))
            .w(px(DEFINE_WIDTH))
            .h(px(30.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.))
            .bg(self.color("input.background"))
            .border_1()
            .border_color(input_border)
            .text_color(self.foreground_or("input.foreground"))
            .child(value.clone());
        let label = value.clone();
        div()
            .id("define-keybinding-widget")
            .track_focus(&self.keybindings_editor.define_focus)
            .absolute()
            .left(px(((width - DEFINE_WIDTH) / 2.).round()))
            .top(px(((height - DEFINE_HEIGHT) / 2.).round()))
            // setHeight(110) on a content-box with 10px padding and a 1px border.
            .h(px(DEFINE_HEIGHT + 22.))
            .p(px(10.))
            .rounded(px(8.))
            .border_1()
            .border_color(self.theme.color("editorWidget.border").unwrap_or(transparent()))
            .bg(self.color("editorWidget.background"))
            .text_color(self.foreground_or("editorWidget.foreground"))
            .shadow(vec![BoxShadow { color: self.color("widget.shadow").into(), offset: point(px(0.), px(0.)), blur_radius: px(6.), spread_radius: px(0.), inset: false }])
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
            .child(div().w(px(DEFINE_WIDTH)).line_height(px(18.)).flex().justify_center().child("Press desired key combination and then press ENTER."))
            .child(input)
            .child(output)
            .children(existing_text.map(|text| {
                // `onShowExistingKeybidings`: the search shows the commands bound to the keys.
                let show = cx.listener(move |workbench, _: &gpui::ClickEvent, window, cx| workbench.set_keybindings_search(format!("\"{label}\""), window, cx));
                div().mt(px(10.)).w(px(DEFINE_WIDTH)).flex().justify_center().child(div().id("existing-keybindings").underline().cursor_pointer().child(text).on_click(show))
            }))
    }
}

/// The table's `SplitView` of column headers as its descriptor gives them: the 40px actions column, then the
/// columns by weight, each 120px at least.
fn table_columns() -> SplitView {
    let views = (0..5).map(|index| match index {
        0 => View::new(0., ACTIONS_COLUMN_WIDTH, ACTIONS_COLUMN_WIDTH),
        _ => View::new(column_weight(index), MIN_COLUMN_WIDTH, f32::INFINITY),
    });
    SplitView::new(COLUMN_WEIGHTS.iter().sum(), views.collect())
}

/// `_validateColumnResizeValue`: `parseInt` must give an integer from 0 to 100.
fn column_resize_validation(value: &str) -> Option<String> {
    match parse_int(value) {
        None if !value.is_empty() => Some("Please enter an integer.".into()),
        Some(percentage) if !(0..=100).contains(&percentage) => Some("Please enter a number greater than 0 and less than or equal to 100.".into()),
        _ => None,
    }
}

/// `Number.parseInt(value)`: leading whitespace, an optional sign, then hex digits after `0x` or decimal digits.
fn parse_int(value: &str) -> Option<i64> {
    let value = value.trim_start();
    let (sign, rest) = match value.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, value.strip_prefix('+').unwrap_or(value)),
    };
    let (radix, digits) = match rest.get(..2) {
        Some("0x" | "0X") => (16, &rest[2..]),
        _ => (10, rest),
    };
    let end = digits.find(|c: char| !c.is_digit(radix)).unwrap_or(digits.len());
    match i64::from_str_radix(&digits[..end], radix) {
        Ok(number) => Some(sign * number),
        // JavaScript numbers do not overflow; any too large value is out of the 0 to 100 range.
        Err(error) if *error.kind() == std::num::IntErrorKind::PosOverflow => Some(sign * i64::MAX),
        Err(_) => None,
    }
}

/// The weight of column `index`; the actions column has none.
fn column_weight(index: usize) -> f32 {
    if index == 0 { 0. } else { COLUMN_WEIGHTS[index - 1] }
}

/// CSS `ease-out`, cubic-bezier(0, 0, 0.58, 1): the curve's x solved by bisection, then its y.
fn ease_out(x: f32) -> f32 {
    let bezier = |p1: f32, p2: f32, t: f32| 3. * p1 * t * (1. - t).powi(2) + 3. * p2 * t * t * (1. - t) + t.powi(3);
    let (mut low, mut high) = (0., 1.);
    for _ in 0..24 {
        let mid = (low + high) / 2.;
        if bezier(0., 0.58, mid) < x { low = mid } else { high = mid }
    }
    bezier(0., 1., (low + high) / 2.)
}

/// Which of the chord's key caps (`Chord::key_caps` order) a search matched.
fn matches_of(chord: &Chord, matched: ChordMatch) -> Vec<bool> {
    let mut caps = Vec::new();
    for (pressed, matched) in [(chord.ctrl, matched.ctrl), (chord.shift, matched.shift), (chord.alt, matched.alt), (chord.meta, matched.meta)] {
        if pressed {
            caps.push(matched);
        }
    }
    caps.push(matched.key);
    caps
}

/// A `HighlightedLabel` cut with an ellipsis: matches are bold in the highlight color.
pub(super) fn highlighted(text: &str, matches: &[Range<usize>], color: Rgba) -> Div {
    let byte_at = |chars: usize| text.char_indices().nth(chars).map_or(text.len(), |(byte, _)| byte);
    let highlights = matches.iter().map(|range| (byte_at(range.start)..byte_at(range.end), HighlightStyle { color: Some(color.into()), font_weight: Some(FontWeight::BOLD), ..Default::default() }));
    div().min_w_0().truncate().child(StyledText::new(text.to_string()).with_highlights(highlights.collect::<Vec<_>>()))
}

/// The `.code` color of the keybindings editor theming participant: `foreground.transparent(.8).makeOpaque(background)`.
fn code_color(foreground: Rgba, background: Rgba) -> Rgba {
    let alpha = foreground.a * 0.8;
    let mix = |front: f32, back: f32| front * alpha + back * (1. - alpha);
    Rgba { r: mix(foreground.r, background.r), g: mix(foreground.g, background.g), b: mix(foreground.b, background.b), a: 1. }
}

/// `WORKBENCH_BACKGROUND` (workbench/common/theme.ts).
fn workbench_background(kind: ThemeKind) -> Rgba {
    match kind {
        ThemeKind::Light => rgb(0xF3F3F3),
        ThemeKind::HighContrastLight => rgb(0xFFFFFF),
        ThemeKind::HighContrastDark => rgb(0x000000),
        ThemeKind::Dark => rgb(0x252526),
    }
}

fn transparent() -> Rgba {
    gpui::transparent_black().into()
}
