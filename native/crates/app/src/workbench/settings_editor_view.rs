//! The Settings editor (contrib/preferences/browser/settingsEditor2.ts with settingsTree.ts, tocTree.ts,
//! preferencesWidgets.ts and settingsSearchMenu.ts) in the modal editor part, styled as media/settingsEditor2.css,
//! media/settingsWidgets.css and the modern UI overrides draw it.

mod list_widget;
mod search_suggest;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use config::service::Target;
use gpui::{
    AnyElement, ClipboardItem, Context, FocusHandle, FontWeight, InteractiveText, KeyDownEvent, ListAlignment, ListState, MouseButton, MouseDownEvent, Rgba, StyledText, Task,
    MouseMoveEvent, MouseUpEvent, TextRun, Window, anchored, canvas, deferred, div, font, list, point, prelude::*, px,
};
use serde_json::{Value, json};

use super::Workbench;
use super::find_widget::HISTORY_LIMIT;
use super::hover::{HoverContent, HoverOptions};
use super::input_box_view::{InputId, InputLook, InputView};
use super::keybindings_editor_view::{ModalHeader, SASH_HOVER_DELAY, SASH_SIZE};
use crate::commands::Outcome;
use crate::default_settings::{SettingsGroup, registered_groups};
use crate::history::HistoryNavigator;
use crate::input_box::InputBox;
use crate::menus::MenuAction;
use crate::split_view::{SplitView, View};
use crate::settings_editor::{
    self as model, Filters, Inspection, MarkdownRun, Model, Row, TocRow, ValueType, inspect_setting, markdown_runs, parse_query, setting_key_to_display_format, tree_rows, value_type,
};
use crate::settings_editor::validation::{invalid_type_error, js_string_to_number, validate};

const SEARCH_DELAY: Duration = Duration::from_millis(200);
/// `SETTING_UPDATE_SLOW_DEBOUNCE`: typed text and numbers write 1s after the last key.
const SLOW_UPDATE_DELAY: Duration = Duration::from_millis(1000);
const TOC_MIN_WIDTH: f32 = 100.;
const TOC_RESET_WIDTH: f32 = 200.;
const EDITOR_MIN_WIDTH: f32 = 500.;
const NARROW_TOTAL_WIDTH: f32 = TOC_RESET_WIDTH + EDITOR_MIN_WIDTH;
const SPLIT_VIEW_WIDTH_KEY: &str = "settingsEditor2.splitViewWidth";
const TOC_ROW_HEIGHT: f32 = 22.;
const SEARCH_BOX_LABEL: &str = "Search settings";
const SEARCH_HISTORY_KEY: &str = "settingsEditor2.searchHistory";

/// settingsSearchMenu.ts `getActions`: id, label, the query word, and how it applies (`Append` triggers suggest upstream).
enum FilterKind {
    Toggle,
    Append,
    Exclusive(&'static [&'static str]),
}
const FILTER_ACTIONS: &[(&str, &str, &str, FilterKind)] = &[
    ("modifiedSettingsSearch", "Modified", "@modified", FilterKind::Toggle),
    ("", "", "", FilterKind::Append),
    ("extSettingsSearch", "Extension ID...", "@ext:", FilterKind::Append),
    ("featuresSettingsSearch", "Feature...", "@feature:", FilterKind::Append),
    ("tagSettingsSearch", "Tag...", "@tag:", FilterKind::Append),
    ("langSettingsSearch", "Language...", "@lang:", FilterKind::Append),
    ("idSettingsSearch", "Setting ID...", "@id:", FilterKind::Append),
    ("", "", "", FilterKind::Append),
    ("onlineSettingsSearch", "Online services", "@tag:usesOnlineServices", FilterKind::Toggle),
    ("policySettingsSearch", "Organization policies", "@hasPolicy", FilterKind::Toggle),
    ("", "", "", FilterKind::Append),
    ("stableSettingsSearch", "Stable", "@stable", FilterKind::Exclusive(&["@tag:preview", "@tag:experimental"])),
    ("previewSettingsSearch", "Preview", "@tag:preview", FilterKind::Exclusive(&["@stable", "@tag:experimental"])),
    ("experimentalSettingsSearch", "Experimental", "@tag:experimental", FilterKind::Exclusive(&["@stable", "@tag:preview"])),
    ("", "", "", FilterKind::Append),
    ("advancedSettingsSearch", "Advanced", "@tag:advanced", FilterKind::Toggle),
    ("expAssignmentSettingsSearch", "ExP Assigned", "@tag:expassigned", FilterKind::Toggle),
];

pub struct SettingsEditor {
    pub open: bool,
    /// The settings target tab: User, Workspace, or Folder with its folder.
    target: Target,
    folder: Option<PathBuf>,
    groups: Vec<SettingsGroup>,
    model: Model,
    inspections: Vec<Inspection>,
    /// The search results in result order, `None` while browsing.
    results: Option<Vec<usize>>,
    rows: Vec<Row>,
    toc: Vec<TocRow>,
    expanded: HashSet<String>,
    /// `viewState.categoryFilter`: the TOC entry whose settings the tree shows.
    category: Option<String>,
    toc_focused: Option<usize>,
    focused: Option<usize>,
    hovered: Option<usize>,
    /// The focused row's open enum dropdown and its highlighted option.
    dropdown: Option<usize>,
    pub search: InputBox,
    pub search_view: InputView,
    pub search_focus: FocusHandle,
    history: HistoryNavigator,
    pub list: list_widget::ListEditor,
    suggest: Option<search_suggest::Suggest>,
    /// The text or number control being edited: the setting and its value.
    pub value: Option<(usize, InputBox)>,
    pub value_view: InputView,
    pub value_focus: FocusHandle,
    control_focus: FocusHandle,
    tree_focus: FocusHandle,
    toc_focus: FocusHandle,
    modal_focus: FocusHandle,
    tree: ListState,
    toc_list: ListState,
    /// The TOC and the tree side by side; the TOC width persists under `settingsEditor2.splitViewWidth`.
    split: Option<SplitView>,
    sash_hovered: bool,
    sash_hover: Option<Task<()>>,
    searching: Option<Task<()>>,
    writing: Option<Task<()>>,
    previous_focus: Option<FocusHandle>,
}

impl SettingsEditor {
    pub fn new(cx: &mut gpui::App) -> Self {
        SettingsEditor {
            open: false,
            target: Target::UserLocal,
            folder: None,
            groups: Vec::new(),
            model: Model::default(),
            inspections: Vec::new(),
            results: None,
            rows: Vec::new(),
            toc: Vec::new(),
            expanded: HashSet::new(),
            category: None,
            toc_focused: None,
            focused: None,
            hovered: None,
            dropdown: None,
            search: InputBox::default(),
            search_view: InputView::default(),
            search_focus: cx.focus_handle(),
            history: HistoryNavigator::new(Vec::new(), HISTORY_LIMIT),
            list: list_widget::ListEditor::new(cx),
            suggest: None,
            value: None,
            value_view: InputView::default(),
            value_focus: cx.focus_handle(),
            control_focus: cx.focus_handle(),
            tree_focus: cx.focus_handle(),
            toc_focus: cx.focus_handle(),
            modal_focus: cx.focus_handle(),
            tree: ListState::new(0, ListAlignment::Top, px(400.)),
            toc_list: ListState::new(0, ListAlignment::Top, px(200.)),
            split: None,
            sash_hovered: false,
            sash_hover: None,
            searching: None,
            writing: None,
            previous_focus: None,
        }
    }

    fn focused_setting(&self) -> Option<usize> {
        match self.rows.get(self.focused?)? {
            Row::Setting(index, _) => Some(*index),
            Row::Group { .. } => None,
        }
    }
}

/// `filterAction` menu items: the toggles are checked when the query has their word.
pub fn filter_menu_actions(query: &str) -> Vec<MenuAction> {
    let words: Vec<&str> = query.split(' ').collect();
    let mut actions = Vec::new();
    for (id, title, word, kind) in FILTER_ACTIONS {
        if id.is_empty() {
            actions.push(MenuAction::Separator);
            continue;
        }
        let checked = !matches!(kind, FilterKind::Append) && words.contains(word);
        actions.push(MenuAction::Command { id: id.to_string(), title: title.to_string(), enabled: true, checked, alt: None });
    }
    actions
}

/// The query after running the filter menu item `id` on `query`.
pub fn apply_filter_action(query: &str, id: &str) -> String {
    let Some((_, _, word, kind)) = FILTER_ACTIONS.iter().find(|(action, ..)| *action == id) else { return query.to_string() };
    let words: Vec<&str> = query.split(' ').collect();
    let append = |words: Vec<&str>| {
        let rest = words.join(" ").trim_end().to_string();
        if rest.is_empty() { word.to_string() } else { format!("{rest} {word}") }
    };
    match kind {
        FilterKind::Append => format!("{} {word}", query.trim_end()),
        _ if words.contains(word) => words.into_iter().filter(|part| part != word).collect::<Vec<_>>().join(" "),
        FilterKind::Toggle => append(words),
        FilterKind::Exclusive(excluded) => append(words.into_iter().filter(|part| !excluded.contains(part)).collect()),
    }
}

/// The value a typed text control writes once the setting's validator (`renderValidations`) accepts it:
/// a number type parses as `parseInt` or `parseFloat`, an empty nullable number is null.
pub fn parse_typed_value(schema: &Value, kind: ValueType, text: &str) -> Result<Value, String> {
    let error = validate(schema, &Value::String(text.to_string()));
    if !error.is_empty() {
        return Err(error);
    }
    let nullable = matches!(kind, ValueType::NullableInteger | ValueType::NullableNumber);
    if nullable && text.is_empty() {
        return Ok(Value::Null);
    }
    let number = || js_string_to_number(text).ok_or_else(|| "Value must be a number.".to_string());
    match kind {
        ValueType::Integer | ValueType::NullableInteger => Ok(Value::from(number()?.trunc() as i64)),
        ValueType::Number | ValueType::NullableNumber => serde_json::Number::from_f64(number()?).map(Value::Number).ok_or_else(|| "Value must be a number.".to_string()),
        _ => Ok(Value::String(text.to_string())),
    }
}

/// `CopySettingAsJSONAction`: `"key": value` with two-space indents.
pub fn setting_as_json(key: &str, value: &Value) -> String {
    let mut buffer = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut buffer, serde_json::ser::PrettyFormatter::with_indent(b"  "));
    serde::Serialize::serialize(value, &mut serializer).expect("a JSON value serializes");
    format!("\"{key}\": {}", String::from_utf8(buffer).expect("JSON is UTF-8"))
}

impl Workbench {
    pub(super) fn register_settings_editor_commands(&mut self) {
        self.register_settings_suggest_commands();
        let commands = &mut self.commands;
        // preferences.contribution.ts: the UI editor unless `workbench.settings.editor` is `json`.
        commands.register("workbench.action.openSettings", |workbench, args, window, cx| {
            let query = open_settings_query(&args);
            if workbench.setting("workbench.settings.editor") == "json" {
                return workbench.execute_command("workbench.action.openSettingsJson", args, window, cx);
            }
            workbench.open_settings_editor(query, None, window, cx);
            Outcome::done()
        });
        commands.register("workbench.action.openSettings2", |workbench, args, window, cx| {
            workbench.open_settings_editor(open_settings_query(&args), None, window, cx);
            Outcome::done()
        });
        commands.register("workbench.action.openGlobalSettings", |workbench, args, window, cx| {
            workbench.open_settings_editor(open_settings_query(&args), Some((Target::UserLocal, None)), window, cx);
            Outcome::done()
        });
        commands.register("workbench.action.openWorkspaceSettings", |workbench, args, window, cx| {
            workbench.open_settings_editor(open_settings_query(&args), Some((Target::Workspace, None)), window, cx);
            Outcome::done()
        });
        commands.register("workbench.action.openAccessibilitySettings", |workbench, _, window, cx| {
            workbench.open_settings_editor(Some("@tag:accessibility".into()), None, window, cx);
            Outcome::done()
        });
        commands.register("settings.action.search", |workbench, _, window, cx| {
            workbench.focus_settings_search(true, window, cx);
            Outcome::done()
        });
        commands.register("settings.action.clearSearchResults", |workbench, _, window, cx| {
            workbench.set_settings_search(String::new(), window, cx);
            workbench.focus_settings_search(false, window, cx);
            Outcome::done()
        });
        // navigateSearchHistoryNextOrFocusSettings: Down walks the history forward while the input shows an entry of it.
        commands.register("settings.action.focusSettingsFromSearch", |workbench, _, window, cx| {
            let editor = &mut workbench.settings_editor;
            if !editor.history.is_nowhere() {
                let value = editor.history.show_next_value(&editor.search.value);
                workbench.set_settings_search(value, window, cx);
                return Outcome::done();
            }
            // focusFirstSettingFromSearch: nothing while the results lag behind the input.
            if editor.searching.is_none() {
                let row = editor.focused.unwrap_or(0);
                workbench.focus_settings_tree(Some(row), window, cx);
            }
            Outcome::done()
        });
        commands.register("settings.action.showPreviousSearch", |workbench, _, window, cx| {
            let editor = &mut workbench.settings_editor;
            if let Some(value) = editor.history.show_previous_value(&editor.search.value) {
                workbench.set_settings_search(value, window, cx);
                workbench.settings_editor.search.select(0, 0);
            }
            Outcome::done()
        });
        for id in ["settings.action.focusSettingsFromSearchOnEnter", "settings.action.focusSettingsList"] {
            commands.register(id, |workbench, _, window, cx| {
                workbench.flush_settings_search(cx);
                workbench.focus_settings_tree(Some(workbench.settings_editor.focused.unwrap_or(0)), window, cx);
                Outcome::done()
            });
        }
        commands.register("settings.action.focusSearchFromSettings", |workbench, _, window, cx| {
            workbench.focus_settings_search(false, window, cx);
            Outcome::done()
        });
        commands.register("settings.action.focusTOC", |workbench, _, window, cx| {
            let toc_focus = workbench.settings_editor.toc_focus.clone();
            window.focus(&toc_focus, cx);
            cx.notify();
            Outcome::done()
        });
        commands.register("settings.action.focusSettingControl", |workbench, _, window, cx| {
            workbench.focus_setting_control(window, cx);
            Outcome::done()
        });
        commands.register("settings.action.focusLevelUp", |workbench, _, window, cx| {
            let editor = &workbench.settings_editor;
            if editor.value_focus.is_focused(window) || editor.control_focus.is_focused(window) || editor.list.is_focused(window) {
                workbench.settings_write_now(cx);
                workbench.focus_settings_tree(editor_focused(workbench), window, cx);
            } else if workbench.settings_editor.tree_focus.is_focused(window) {
                let toc_focus = workbench.settings_editor.toc_focus.clone();
                window.focus(&toc_focus, cx);
            }
            cx.notify();
            Outcome::done()
        });
        commands.register("settings.action.showContextMenu", |workbench, _, window, cx| {
            let Some(index) = workbench.settings_editor.focused_setting() else { return Outcome::done() };
            let position = window.mouse_position();
            workbench.show_setting_context_menu(index, gpui::Bounds { origin: position, size: gpui::Size::default() }, window, cx);
            Outcome::done()
        });
        commands.register("settings.action.suggestFilters", |workbench, _, window, cx| {
            let anchor = gpui::Bounds { origin: window.mouse_position(), size: gpui::Size::default() };
            workbench.show_settings_filter_menu(anchor, window, cx);
            Outcome::done()
        });
        for (id, ..) in FILTER_ACTIONS.iter().filter(|(id, ..)| !id.is_empty()) {
            commands.register(*id, move |workbench, _, window, cx| {
                let query = apply_filter_action(&workbench.settings_editor.search.value, id);
                workbench.set_settings_search(query, window, cx);
                workbench.focus_settings_search(false, window, cx);
                Outcome::done()
            });
        }
        // settingsTree.ts `settingActions`, run on the setting the context menu was shown for.
        commands.register("settings.resetSetting", |workbench, args, _, cx| {
            let Some(index) = setting_arg(workbench, &args) else { return Outcome::done() };
            workbench.write_setting(index, None, true, cx);
            Outcome::done()
        });
        commands.register("settings.copySettingId", |workbench, args, _, cx| {
            let Some(index) = setting_arg(workbench, &args) else { return Outcome::done() };
            cx.write_to_clipboard(ClipboardItem::new_string(workbench.settings_editor.model.settings[index].key.clone()));
            Outcome::done()
        });
        commands.register("settings.copySettingAsJSON", |workbench, args, _, cx| {
            let Some(index) = setting_arg(workbench, &args) else { return Outcome::done() };
            let editor = &workbench.settings_editor;
            cx.write_to_clipboard(ClipboardItem::new_string(setting_as_json(&editor.model.settings[index].key, &editor.inspections[index].value)));
            Outcome::done()
        });
        commands.register("settings.copySettingAsURL", |workbench, args, _, cx| {
            let Some(index) = setting_arg(workbench, &args) else { return Outcome::done() };
            cx.write_to_clipboard(ClipboardItem::new_string(format!("vsrustium://settings/{}", workbench.settings_editor.model.settings[index].key)));
            Outcome::done()
        });
        // listCommands.ts on the settings tree and TOC; other lists keep the handler registered before.
        for (id, key) in [
            ("list.focusDown", "down"),
            ("list.focusUp", "up"),
            ("list.focusPageDown", "pagedown"),
            ("list.focusPageUp", "pageup"),
            ("list.focusFirst", "home"),
            ("list.focusLast", "end"),
            ("list.select", "enter"),
            ("list.toggleExpand", "space"),
            ("list.expand", "right"),
            ("list.collapse", "left"),
        ] {
            let previous = self.commands.get(id);
            self.commands.register(id, move |workbench, args, window, cx| {
                if workbench.settings_list_key(key, window, cx) {
                    return Outcome::done();
                }
                match &previous {
                    Some(previous) => previous(workbench, args, window, cx),
                    None => Outcome::error(format!("command '{id}' not found")),
                }
            });
        }
    }

    /// `preferencesService.openSettings` with `jsonEditor: false`: shows the editor in the modal part, then searches.
    pub(super) fn open_settings_editor(&mut self, query: Option<String>, target: Option<(Target, Option<PathBuf>)>, window: &mut Window, cx: &mut Context<Self>) {
        self.close_keybindings_editor(window, cx);
        let editor = &mut self.settings_editor;
        let opening = !editor.open;
        if opening {
            editor.previous_focus = window.focused(cx);
            editor.open = true;
            editor.target = Target::UserLocal;
            editor.folder = None;
            editor.search = InputBox::default();
            editor.search_view.reset();
            editor.category = None;
            editor.focused = None;
            editor.toc_focused = None;
            editor.expanded.clear();
            let stored = self.profile_storage.lock().unwrap().get(SEARCH_HISTORY_KEY).and_then(|raw| serde_json::from_str::<Vec<String>>(raw).ok());
            self.settings_editor.history = HistoryNavigator::new(stored.unwrap_or_default(), HISTORY_LIMIT);
            self.settings_editor.split = None;
        }
        if let Some((target, folder)) = target {
            let has_workspace = self.settings.lock().unwrap().workspace.is_some();
            if target == Target::UserLocal || has_workspace {
                self.settings_editor.target = target;
                self.settings_editor.folder = folder;
            }
        }
        self.keybindings_editor.maximized = false;
        self.refresh_settings_model();
        if let Some(query) = query {
            self.set_settings_search(query, window, cx);
            self.flush_settings_search(cx);
        } else if opening {
            // Initial category: the first one (Commonly Used), focused and selected in the TOC.
            let editor = &mut self.settings_editor;
            editor.category = editor.model.root.children.first().map(|group| group.id.clone());
            editor.toc_focused = editor.category.as_ref().map(|_| 0);
            self.render_settings(cx);
        }
        self.focus_settings_search(false, window, cx);
    }

    pub(super) fn close_settings_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.settings_editor.open {
            return;
        }
        self.settings_write_now(cx);
        let editor = &mut self.settings_editor;
        editor.open = false;
        editor.searching = None;
        editor.dropdown = None;
        editor.value = None;
        editor.list.reset();
        editor.suggest = None;
        editor.search_view.reset();
        editor.value_view.reset();
        if let Some(focus) = editor.previous_focus.take() {
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    pub(super) fn settings_editor_focused(&self, window: &Window) -> bool {
        let editor = &self.settings_editor;
        editor.open
            && ([&editor.modal_focus, &editor.search_focus, &editor.value_focus, &editor.control_focus, &editor.tree_focus, &editor.toc_focus].into_iter().any(|focus| focus.is_focused(window))
                || editor.list.is_focused(window))
    }

    /// The context keys of the modal part and of the Settings editor (`inSettingsEditor` and its widgets').
    pub(super) fn settings_editor_context(&self, window: &Window, context: &mut HashMap<String, Value>) {
        if !self.settings_editor_focused(window) {
            return;
        }
        let editor = &self.settings_editor;
        let search = editor.search_focus.is_focused(window);
        let tree = editor.tree_focus.is_focused(window);
        let toc = editor.toc_focus.is_focused(window);
        let toc_row = editor.toc_focused.and_then(|index| editor.toc.get(index));
        context.insert("editorPartModal".into(), json!(true));
        context.insert("editorPartModalVisible".into(), json!(true));
        context.insert("editorPartModalMaximized".into(), json!(self.keybindings_editor.maximized));
        context.insert("activeEditor".into(), json!("workbench.editor.settings2"));
        context.insert("inSettingsEditor".into(), json!(true));
        context.insert("inSettingsSearch".into(), json!(search));
        if search {
            self.settings_suggest_context(context);
        }
        context.insert("settingRowFocus".into(), json!(tree && editor.focused_setting().is_some()));
        context.insert("settingFirstRowFocus".into(), json!(tree && editor.focused == Some(0)));
        context.insert("settingsTocRowFocus".into(), json!(toc && toc_row.is_some()));
        context.insert("listFocus".into(), json!(tree || toc));
        if let Some(row) = toc_row.filter(|_| toc) {
            let expanded = editor.expanded.contains(&row.id);
            context.insert("treeElementCanCollapse".into(), json!(row.has_children && expanded));
            context.insert("treeElementCanExpand".into(), json!(row.has_children && !expanded));
            context.insert("treeElementHasParent".into(), json!(row.level > 1));
        }
        if search || editor.value_focus.is_focused(window) || editor.list.key_focus.is_focused(window) || editor.list.value_focus.is_focused(window) {
            context.insert("inputFocus".into(), json!(true));
        }
    }

    /// `onConfigUpdate`: the groups of the target, the tree, and each setting's state in the target.
    fn refresh_settings_model(&mut self) {
        let settings = self.settings.lock().unwrap();
        let editor = &mut self.settings_editor;
        editor.groups = registered_groups(&settings.registry, editor.target);
        let groups = &editor.groups;
        let show_advanced = settings.get_value(Some("workbench.settings.alwaysShowAdvancedSettings"), &Default::default()) == Some(json!(true));
        editor.model = Model::new(groups, show_advanced, |id| groups.iter().find(|group| group.extension_id.as_deref() == Some(id)).map_or_else(|| id.to_string(), |group| group.title.clone()));
        let folder = editor.folder.as_deref();
        editor.inspections = editor.model.settings.iter().map(|setting| inspect_setting(&settings.configuration, &setting.key, editor.target, folder)).collect();
    }

    /// The settings changed on disk or through a write: the shown editor reads them again.
    pub(super) fn settings_editor_configuration_changed(&mut self, cx: &mut Context<Self>) {
        if !self.settings_editor.open {
            return;
        }
        self.refresh_settings_model();
        self.render_settings(cx);
    }

    /// `triggerSearch` and `renderTree`: the results of the query, then the rows of the tree and the TOC.
    fn render_settings(&mut self, cx: &mut Context<Self>) {
        let languages = &self.languages;
        let editor = &mut self.settings_editor;
        let raw_query = editor.search.value.trim().replace('\u{203A}', " ");
        let parsed = parse_query(&raw_query);
        let always_show_advanced = self.settings.lock().unwrap().get_value(Some("workbench.settings.alwaysShowAdvancedSettings"), &Default::default()) == Some(json!(true));
        let filters = Filters { parsed: &parsed, raw_query: &raw_query, target: editor.target, inspections: &editor.inspections, always_show_advanced };
        let inspections = &editor.inspections;
        let value_of = |key: &str| editor.model.index_of(key).map(|index| inspections[index].value.clone());
        editor.results = model::search(&editor.model, &editor.groups, &filters, value_of, |language| languages.is_registered(language));
        editor.toc = model::toc_rows(&editor.model, editor.results.as_deref(), editor.target, &editor.expanded);
        editor.rows = tree_rows(&editor.model, editor.results.as_deref(), editor.category.as_deref(), editor.target);
        editor.focused = editor.focused.filter(|&index| index < editor.rows.len());
        editor.toc_focused = editor.toc_focused.filter(|&index| index < editor.toc.len());
        editor.tree.reset(editor.rows.len());
        editor.toc_list.reset(editor.toc.len());
        cx.notify();
    }

    fn focus_settings_search(&mut self, select_all: bool, window: &mut Window, cx: &mut Context<Self>) {
        let focus = self.settings_editor.search_focus.clone();
        window.focus(&focus, cx);
        if select_all {
            self.settings_editor.search.select_all();
        }
        self.blink_caret(InputId::SettingsSearch, window, cx);
        cx.notify();
    }

    fn set_settings_search(&mut self, value: String, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_input_box(InputId::SettingsSearch, window, cx, |input| input.set_value(value));
    }

    /// `onSearchInputChanged` after the 200ms `searchInputDelayer`: a new query clears the category filter.
    pub(super) fn edit_settings_search(&mut self, edit: impl FnOnce(&mut InputBox), window: &mut Window, cx: &mut Context<Self>) {
        let editor = &mut self.settings_editor;
        let before = editor.search.value.clone();
        edit(&mut editor.search);
        cx.notify();
        if editor.search.value == before {
            return;
        }
        let caret = editor.search.selection.1;
        let typed = (editor.search.len() == before.chars().count() + 1).then(|| editor.search.value.chars().nth(caret.wrapping_sub(1))).flatten();
        self.update_settings_search_suggest(typed);
        let editor = &mut self.settings_editor;
        editor.searching = Some(cx.spawn_in(window, async move |workbench, cx| {
            cx.background_executor().timer(SEARCH_DELAY).await;
            let _ = workbench.update(cx, |workbench, cx| workbench.search_settings_now(cx));
        }));
    }

    fn flush_settings_search(&mut self, cx: &mut Context<Self>) {
        if self.settings_editor.searching.take().is_some() {
            self.search_settings_now(cx);
        }
    }

    fn search_settings_now(&mut self, cx: &mut Context<Self>) {
        let editor = &mut self.settings_editor;
        editor.searching = None;
        editor.category = None;
        editor.focused = None;
        editor.toc_focused = None;
        // A filter model expands the whole TOC; leaving search mode collapses it.
        editor.expanded.clear();
        if !editor.search.value.trim().is_empty() {
            model::collect_group_ids(&editor.model.root, &mut editor.expanded);
            editor.history.add_to_history(&editor.search.value, false);
            self.save_settings_search_history();
        }
        self.render_settings(cx);
        self.settings_editor.tree.scroll_to(gpui::ListOffset::default());
    }

    fn save_settings_search_history(&self) {
        let history = self.settings_editor.history.elements();
        let mut storage = self.profile_storage.lock().unwrap();
        match history.is_empty() {
            true => storage.delete(SEARCH_HISTORY_KEY),
            false => storage.set(SEARCH_HISTORY_KEY, serde_json::to_string(history).expect("strings serialize")),
        }
    }

    fn focus_settings_tree(&mut self, row: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let editor = &mut self.settings_editor;
        editor.value = None;
        editor.dropdown = None;
        editor.list.reset();
        editor.suggest = None;
        editor.focused = row.filter(|&row| row < editor.rows.len());
        if let Some(row) = editor.focused {
            editor.tree.scroll_to_reveal_item(row);
        }
        let focus = editor.tree_focus.clone();
        window.focus(&focus, cx);
        cx.notify();
    }

    /// `focusSettings` into the focused row's control: the text input, or the checkbox, select or link.
    fn focus_setting_control(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.settings_editor.focused_setting() else { return };
        let editor = &mut self.settings_editor;
        let kind = value_type(&editor.model.settings[index]);
        if is_list_control(kind) {
            self.focus_list_widget(index, window, cx);
            return;
        }
        if is_text_control(kind) {
            let text = text_of(&editor.inspections[index].value);
            let mut input = InputBox::new(text);
            input.select_all();
            editor.value = Some((index, input));
            editor.value_view.reset();
            let focus = editor.value_focus.clone();
            window.focus(&focus, cx);
            self.blink_caret(InputId::SettingsValue, window, cx);
        } else {
            let focus = editor.control_focus.clone();
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    pub(super) fn edit_settings_value(&mut self, edit: impl FnOnce(&mut InputBox), window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, input)) = self.settings_editor.value.as_mut() else { return };
        let before = input.value.clone();
        edit(input);
        cx.notify();
        if input.value == before {
            return;
        }
        self.settings_editor.writing = Some(cx.spawn_in(window, async move |workbench, cx| {
            cx.background_executor().timer(SLOW_UPDATE_DELAY).await;
            let _ = workbench.update(cx, |workbench, cx| workbench.settings_write_now(cx));
        }));
    }

    /// The pending typed value writes now (`pendingSettingUpdate` when the focus moves on).
    fn settings_write_now(&mut self, cx: &mut Context<Self>) {
        if self.settings_editor.writing.take().is_none() {
            return;
        }
        let Some((index, input)) = &self.settings_editor.value else { return };
        let setting = &self.settings_editor.model.settings[*index];
        if let Ok(value) = parse_typed_value(&setting.schema, value_type(setting), &input.value) {
            let index = *index;
            self.write_setting_value(index, Some(value), false, cx);
        }
    }

    fn write_setting(&mut self, index: usize, value: Option<Value>, manual_reset: bool, cx: &mut Context<Self>) {
        self.write_setting_value(index, value, manual_reset, cx);
        self.settings_editor_configuration_changed(cx);
    }

    /// `updateChangedSetting`: the default value in the user tab resets the setting instead of writing it.
    fn write_setting_value(&mut self, index: usize, value: Option<Value>, manual_reset: bool, cx: &mut Context<Self>) {
        let editor = &self.settings_editor;
        let key = editor.model.settings[index].key.clone();
        let (target, folder) = (editor.target, editor.folder.clone());
        let language = parse_query(&editor.search.value).language_filter;
        let workspace_target = matches!(target, Target::Workspace | Target::WorkspaceFolder);
        let manual = workspace_target || language.is_some();
        let primitive = value.as_ref().is_some_and(|value| !value.is_object() && !value.is_array());
        let value = if !manual && !manual_reset && primitive && value.as_ref() == Some(&editor.inspections[index].default_value) { None } else { value };
        let before = self.model_creation_options();
        let overrides: Vec<String> = language.into_iter().collect();
        let result = self.settings.lock().unwrap().update_value(&key, value.as_ref(), target, folder.as_deref(), &overrides);
        match result {
            Ok(change) if !change.is_empty() => self.configuration_changed(&change, before, cx),
            Ok(_) => {}
            Err(error) => eprintln!("writing {key}: {error}"),
        }
    }

    fn set_settings_target(&mut self, target: Target, folder: Option<PathBuf>, cx: &mut Context<Self>) {
        self.settings_write_now(cx);
        let editor = &mut self.settings_editor;
        editor.target = target;
        editor.folder = folder;
        editor.value = None;
        self.refresh_settings_model();
        self.render_settings(cx);
    }

    /// The tree's and TOC's list commands; false when neither has the focus.
    fn settings_list_key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let editor = &mut self.settings_editor;
        if !editor.open {
            return false;
        }
        if editor.toc_focus.is_focused(window) {
            self.toc_key(key, window, cx);
            return true;
        }
        if !editor.tree_focus.is_focused(window) {
            return false;
        }
        let count = editor.rows.len();
        if count == 0 {
            return true;
        }
        let page = (f32::from(editor.tree.viewport_bounds().size.height) / 80.).floor().max(1.) as usize;
        let current = editor.focused;
        let next = match key {
            "down" => current.map_or(0, |index| (index + 1).min(count - 1)),
            "up" => current.map_or(0, |index| index.saturating_sub(1)),
            "pagedown" => current.map_or(0, |index| (index + page).min(count - 1)),
            "pageup" => current.map_or(0, |index| index.saturating_sub(page)),
            "home" => 0,
            "end" => count - 1,
            "enter" | "space" => {
                self.focus_setting_control(window, cx);
                return true;
            }
            _ => return true,
        };
        self.focus_settings_tree(Some(next), window, cx);
        true
    }

    /// TOC navigation; the focused entry filters the tree (`onDidChangeFocus` sets `categoryFilter`).
    fn toc_key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let editor = &mut self.settings_editor;
        let count = editor.toc.len();
        if count == 0 {
            return;
        }
        let current = editor.toc_focused;
        let next = match key {
            "down" => current.map_or(0, |index| (index + 1).min(count - 1)),
            "up" => current.map_or(0, |index| index.saturating_sub(1)),
            "pagedown" => current.map_or(0, |index| (index + 10).min(count - 1)),
            "pageup" => current.map_or(0, |index| index.saturating_sub(10)),
            "home" => 0,
            "end" => count - 1,
            "right" | "left" | "space" => {
                let Some(row) = current.map(|index| editor.toc[index].clone()) else { return };
                let expanded = editor.expanded.contains(&row.id);
                match key {
                    "left" if !expanded || !row.has_children => {
                        let parent = (0..current.unwrap_or(0)).rev().find(|&index| editor.toc[index].level < row.level);
                        if let Some(parent) = parent {
                            self.focus_toc_row(parent, window, cx);
                        }
                        return;
                    }
                    "right" if expanded => return,
                    _ if row.has_children => {
                        if expanded {
                            editor.expanded.remove(&row.id);
                        } else {
                            editor.expanded.insert(row.id);
                        }
                    }
                    _ => return,
                }
                self.render_settings(cx);
                return;
            }
            "enter" => {
                self.focus_settings_tree(Some(0), window, cx);
                return;
            }
            _ => return,
        };
        self.focus_toc_row(next, window, cx);
    }

    fn focus_toc_row(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let editor = &mut self.settings_editor;
        editor.toc_focused = Some(index);
        editor.toc_list.scroll_to_reveal_item(index);
        editor.category = Some(editor.toc[index].id.clone());
        editor.focused = None;
        let focus = editor.toc_focus.clone();
        window.focus(&focus, cx);
        self.render_settings(cx);
        self.settings_editor.tree.scroll_to(gpui::ListOffset::default());
    }

    /// `onDidClickSetting`: a `#key#` link shows its setting's TOC entry and focuses it, else searches `@id:key`.
    fn reveal_setting_link(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let editor = &self.settings_editor;
        let Some(index) = editor.model.index_of(key) else {
            self.set_settings_search(format!("@id:{key}"), window, cx);
            return self.flush_settings_search(cx);
        };
        let group = model::group_of(&editor.model, index).map(|group| group.id.clone());
        let Some(group) = group else {
            self.set_settings_search(format!("@id:{key}"), window, cx);
            return self.flush_settings_search(cx);
        };
        if editor.results.is_some() {
            self.set_settings_search(String::new(), window, cx);
            self.flush_settings_search(cx);
        }
        for ancestor in model::group_path(&self.settings_editor.model, &group) {
            self.settings_editor.expanded.insert(ancestor);
        }
        self.settings_editor.category = Some(group.clone());
        self.render_settings(cx);
        let editor = &mut self.settings_editor;
        editor.toc_focused = editor.toc.iter().position(|row| row.id == group);
        let row = editor.rows.iter().position(|row| matches!(row, Row::Setting(found, _) if *found == index));
        self.focus_settings_tree(row, window, cx);
    }

    fn show_setting_context_menu(&mut self, index: usize, anchor: gpui::Bounds<gpui::Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let actions = vec![
            MenuAction::Command { id: "settings.resetSetting".into(), title: "Reset Setting".into(), enabled: true, checked: false, alt: None },
            MenuAction::Separator,
            MenuAction::Command { id: "settings.copySettingId".into(), title: "Copy Setting ID".into(), enabled: true, checked: false, alt: None },
            MenuAction::Command { id: "settings.copySettingAsJSON".into(), title: "Copy Setting as JSON".into(), enabled: true, checked: false, alt: None },
            MenuAction::Command { id: "settings.copySettingAsURL".into(), title: "Copy Setting as URL".into(), enabled: true, checked: false, alt: None },
        ];
        let context = self.key_context(window);
        let key = self.settings_editor.model.settings[index].key.clone();
        self.show_context_menu_actions(actions, &context, anchor, vec![json!(key)], window, cx);
    }

    fn show_settings_filter_menu(&mut self, anchor: gpui::Bounds<gpui::Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let actions = filter_menu_actions(&self.settings_editor.search.value);
        let context = self.key_context(window);
        self.show_context_menu_actions(actions, &context, anchor, Vec::new(), window, cx);
    }

    /// `FolderSettingsActionViewItem`: the folders of a multi-root workspace in a menu.
    fn show_settings_folder_menu(&mut self, anchor: gpui::Bounds<gpui::Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let folders: Vec<(PathBuf, String)> = self.settings.lock().unwrap().workspace.iter().flat_map(|workspace| workspace.folders.iter().map(|folder| (folder.path.clone(), folder.name.clone()))).collect();
        let mut actions = Vec::new();
        for (index, (path, name)) in folders.into_iter().enumerate() {
            let id = format!("_settingsEditor.folder.{index}");
            if self.commands.get(&id).is_none() {
                self.commands.register(id.clone(), move |workbench, _, _, cx| {
                    let path = workbench.settings.lock().unwrap().workspace.as_ref().and_then(|workspace| workspace.folders.get(index)).map(|folder| folder.path.clone());
                    workbench.set_settings_target(Target::WorkspaceFolder, path, cx);
                    Outcome::done()
                });
            }
            let checked = self.settings_editor.folder.as_ref() == Some(&path);
            actions.push(MenuAction::Command { id, title: name, enabled: true, checked, alt: None });
        }
        let context = self.key_context(window);
        self.show_context_menu_actions(actions, &context, anchor, Vec::new(), window, cx);
    }

    pub(super) fn settings_editor_widget(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.settings_editor.open {
            return None;
        }
        let header = ModalHeader { icon: "settings", title: "Settings", json_title: "Open Settings (JSON)", json_command: "workbench.action.openSettingsJson" };
        let focus = self.settings_editor.modal_focus.clone();
        Some(self.modal_editor_part(header, &focus, window, cx, |workbench, width, height, window, cx| workbench.settings_editor_pane(width, height, window, cx)))
    }

    /// `.settings-editor`: the header with the search and the target tabs, then the TOC and the tree.
    fn settings_editor_pane(&mut self, width: f32, height: f32, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let narrow = width < NARROW_TOTAL_WIDTH;
        let body_width = width.min(1400.);
        let no_results = self.settings_editor.results.as_ref().is_some_and(Vec::is_empty);
        let body = if no_results {
            div().mt(px(20.)).px(px(24.)).child("No Settings Found").into_any_element()
        } else if narrow {
            div().flex_1().min_h_0().flex().mt(px(14.)).child(self.settings_tree(narrow, window, cx)).into_any_element()
        } else {
            let toc_width = self.layout_settings_split(body_width);
            div()
                .relative()
                .w(px(body_width))
                .mx_auto()
                .flex_1()
                .min_h_0()
                .flex()
                .mt(px(14.))
                .child(self.settings_toc(toc_width, window, cx))
                .child(self.settings_tree(narrow, window, cx))
                .child(self.settings_sash(toc_width, cx))
                .when(self.settings_editor.split.as_ref().is_some_and(|split| split.dragged_sash().is_some()), |body| body.child(self.settings_sash_drag(cx)))
                .into_any_element()
        };
        div()
            .w(px(width))
            .h(px(height))
            .flex()
            .flex_col()
            .text_size(px(13.))
            .text_color(self.color("foreground"))
            .child(self.settings_header(window, cx))
            .child(body)
            .into_any_element()
    }

    fn settings_header(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        div().w_full().max_w(px(1400.)).mx_auto().mt(px(11.)).pt(px(3.)).px(px(24.)).child(self.settings_search(window, cx)).child(self.settings_tabs(cx))
    }

    /// The `SuggestEnabledInputWithHistory` with the count badge, Clear Settings Search Input and Filter Settings.
    fn settings_search(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let editor = &self.settings_editor;
        let focused = editor.search_focus.is_focused(window);
        let border = if focused { self.color("focusBorder") } else { self.theme.color("settings.textInputBorder").unwrap_or(transparent()) };
        let has_value = !editor.search.value.is_empty();
        let foreground = self.foreground_or("icon.foreground");
        let count = editor.results.as_ref().map(|results| match results.len() {
            0 => "No Settings Found".to_string(),
            1 => "1 Setting Found".to_string(),
            count => format!("{count} Settings Found"),
        });
        // actionbar.css: a disabled codicon action-label takes disabledForeground.
        let clear_color = if has_value { foreground } else { self.color("disabledForeground") };
        let clear = self
            .icon("clear-all", 16., 16., clear_color)
            .id("settings.action.clearSearchResults")
            .p(px(3.))
            .rounded(px(5.))
            .when(has_value, |clear| {
                clear.cursor_pointer().hover(|clear| clear.bg(self.color("toolbar.hoverBackground"))).on_click(cx.listener(|workbench, _, window, cx| {
                    workbench.set_settings_search(String::new(), window, cx);
                    workbench.focus_settings_search(false, window, cx);
                }))
            })
            .child(self.action_hover("Clear Settings Search Input", "settings.action.clearSearchResults", super::hover::HoverOptions::mouse()));
        let filter = self
            .icon("filter", 16., 16., foreground)
            .id("settings.action.suggestFilters")
            .p(px(3.))
            .mr(px(3.))
            .rounded(px(5.))
            .cursor_pointer()
            .hover(|filter| filter.bg(self.color("toolbar.hoverBackground")))
            .on_mouse_down(MouseButton::Left, cx.listener(|workbench, event: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                let anchor = gpui::Bounds { origin: event.position, size: gpui::Size::default() };
                workbench.show_settings_filter_menu(anchor, window, cx);
            }))
            .child(self.action_hover("Filter Settings", "settings.action.suggestFilters", super::hover::HoverOptions::mouse()));
        let widgets = div()
            .absolute()
            .top_0()
            .right(px(1.))
            .h_full()
            .flex()
            .items_center()
            .when_some(count, |widgets, count| {
                widgets.child(div().mr(px(3.)).px(px(3.)).pt(px(2.)).pb(px(3.)).rounded(px(2.)).text_size(px(11.)).text_color(self.color("descriptionForeground")).child(count))
            })
            .child(clear)
            .child(filter);
        div()
            .relative()
            .py(px(2.))
            .flex()
            .items_center()
            .rounded(px(4.))
            .bg(self.color("input.background"))
            .border_1()
            .border_color(border)
            .child(div().flex_1().h(px(20.)).pl(px(6.)).mr(px(160.)).flex().text_size(px(14.)).child(self.input_text(
                InputId::SettingsSearch,
                InputLook { password: false, placeholder: Some(SEARCH_BOX_LABEL.into()), focused },
                cx,
            )))
            .child(widgets)
            .children(self.settings_suggest_widget(7., window, cx))
    }

    /// `SettingsTargetsWidget` under modern UI: rounded tabs, the checked one on `modernTab.activeBackground`.
    fn settings_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let editor = &self.settings_editor;
        let settings = self.settings.lock().unwrap();
        let workspace = settings.workspace.as_ref();
        let has_workspace = workspace.is_some();
        let multi_root = workspace.is_some_and(|workspace| workspace.file.is_some() && !workspace.folders.is_empty());
        drop(settings);
        let count = editor.results.as_ref().map(Vec::len).filter(|&count| count > 0);
        let label = |name: &str, target: Target| match count.filter(|_| editor.target == target) {
            Some(count) => format!("{name} ({count})"),
            None => name.to_string(),
        };
        let tab = |id: &'static str, title: String, checked: bool| {
            let (background, foreground) = match checked {
                true => (Some(self.color("modernTab.activeBackground")), self.foreground_or("modernTab.activeForeground")),
                false => (None, self.color("foreground")),
            };
            div()
                .id(id)
                .mr(px(4.))
                .px(px(8.))
                .py(px(2.))
                .rounded(px(4.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(foreground)
                .when_some(background, |tab, background| tab.bg(background))
                .when(!checked, |tab| tab.opacity(0.9).cursor_pointer().hover(|tab| tab.opacity(1.).bg(self.color("modernTab.hoverBackground"))))
                .child(title)
        };
        div()
            .mt(px(10.))
            .flex()
            .child(tab("settings-tab-user", label("User", Target::UserLocal), editor.target == Target::UserLocal).on_click(cx.listener(|workbench, _, _, cx| {
                workbench.set_settings_target(Target::UserLocal, None, cx)
            })))
            .when(has_workspace, |tabs| {
                tabs.child(tab("settings-tab-workspace", label("Workspace", Target::Workspace), editor.target == Target::Workspace).on_click(cx.listener(|workbench, _, _, cx| {
                    workbench.set_settings_target(Target::Workspace, None, cx)
                })))
            })
            .when(multi_root, |tabs| {
                tabs.child(tab("settings-tab-folder", label("Folder", Target::WorkspaceFolder), editor.target == Target::WorkspaceFolder).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|workbench, event: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        let anchor = gpui::Bounds { origin: event.position, size: gpui::Size::default() };
                        workbench.show_settings_folder_menu(anchor, window, cx);
                    }),
                ))
            })
    }

    /// `.settings-toc-wrapper`: the TOC tree at the split's first view width, rows 22px with their counts in search mode.
    fn settings_toc(&self, width: f32, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let focused = self.settings_editor.toc_focus.is_focused(window);
        let rows = list(self.settings_editor.toc_list.clone(), cx.processor(move |workbench: &mut Workbench, index: usize, _, cx| workbench.toc_row(index, focused, cx))).size_full();
        div()
            .id("settings-toc")
            .w(px(width))
            .flex_none()
            .h_full()
            .pl(px(24.))
            .track_focus(&self.settings_editor.toc_focus)
            .child(rows)
    }

    /// Lays the TOC and tree split out over `width`, the TOC first at its stored width; returns the TOC width.
    fn layout_settings_split(&mut self, width: f32) -> f32 {
        let split = match self.settings_editor.split.take() {
            Some(mut split) => {
                if split.size() != width {
                    split.layout(width);
                }
                split
            }
            None => {
                let stored = self.profile_storage.lock().unwrap().get(SPLIT_VIEW_WIDTH_KEY).and_then(|raw| raw.parse::<f32>().ok());
                let toc = stored.unwrap_or(TOC_RESET_WIDTH).clamp(TOC_MIN_WIDTH, width - EDITOR_MIN_WIDTH);
                let toc_view = View::new(toc, TOC_MIN_WIDTH, f32::INFINITY);
                let tree_view = View::new(width - toc, EDITOR_MIN_WIDTH, f32::INFINITY);
                SplitView::new(width, vec![toc_view, tree_view])
            }
        };
        let toc_width = split.sizes()[0];
        self.settings_editor.split = Some(split);
        toc_width
    }

    /// The split view's sash over the TOC edge with its `settings.sashBorder` separator; a double click resets the
    /// TOC to 200px (`onDidSashReset`).
    fn settings_sash(&self, toc_width: f32, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let editor = &self.settings_editor;
        let dragging = editor.split.as_ref().is_some_and(|split| split.dragged_sash().is_some());
        let separator = self.theme.color("settings.sashBorder").unwrap_or(transparent());
        let hover = self.theme.color("sash.hoverBorder").unwrap_or(self.color("focusBorder"));
        div()
            .id("settings-sash")
            .debug_selector(|| "settings-sash".into())
            .absolute()
            .top_0()
            .bottom_0()
            .left(px(toc_width - SASH_SIZE / 2.))
            .w(px(SASH_SIZE))
            .cursor(gpui::CursorStyle::ResizeColumn)
            .child(div().absolute().top_0().bottom_0().left(px(SASH_SIZE / 2.)).w(px(1.)).bg(separator))
            .when(editor.sash_hovered || dragging, |sash| sash.bg(hover))
            .on_hover(cx.listener(|workbench, hovered: &bool, window, cx| {
                let editor = &mut workbench.settings_editor;
                editor.sash_hovered = false;
                editor.sash_hover = hovered.then(|| {
                    cx.spawn_in(window, async move |workbench, cx| {
                        cx.background_executor().timer(SASH_HOVER_DELAY).await;
                        let _ = workbench.update(cx, |workbench, cx| {
                            workbench.settings_editor.sash_hovered = true;
                            cx.notify();
                        });
                    })
                });
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|workbench, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    let Some(split) = workbench.settings_editor.split.as_mut() else { return };
                    if event.click_count == 2 {
                        let total: f32 = split.sizes().iter().sum();
                        split.resize_view(0, TOC_RESET_WIDTH);
                        split.resize_view(1, total - TOC_RESET_WIDTH);
                        workbench.store_settings_split_width();
                    } else {
                        split.start_sash_drag(0, f32::from(event.position.x), event.modifiers.alt);
                    }
                    cx.notify();
                }),
            )
    }

    /// Follows the pointer anywhere in the window while the sash drags; `onDidSashChange` stores the TOC width.
    fn settings_sash_drag(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let workbench = cx.entity();
        canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let moved = workbench.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, _, _, cx| {
                    moved.update(cx, |workbench, cx| {
                        let Some(split) = workbench.settings_editor.split.as_mut() else { return };
                        split.drag_sash(f32::from(event.position.x), event.modifiers.alt);
                        workbench.store_settings_split_width();
                        cx.notify();
                    });
                });
                let released = workbench.clone();
                window.on_mouse_event(move |_: &MouseUpEvent, _, _, cx| {
                    released.update(cx, |workbench, cx| {
                        if let Some(split) = workbench.settings_editor.split.as_mut() {
                            split.end_sash_drag();
                        }
                        cx.notify();
                    });
                });
            },
        )
        .size_0()
    }

    fn store_settings_split_width(&mut self) {
        let Some(split) = &self.settings_editor.split else { return };
        self.profile_storage.lock().unwrap().set(SPLIT_VIEW_WIDTH_KEY, split.sizes()[0].to_string());
    }

    fn toc_row(&mut self, index: usize, list_focused: bool, cx: &mut Context<Self>) -> AnyElement {
        let editor = &self.settings_editor;
        let row = editor.toc[index].clone();
        let selected = editor.category.as_deref() == Some(row.id.as_str());
        let focused = editor.toc_focused == Some(index) && list_focused;
        let search_mode = editor.results.is_some();
        let foreground = if selected { self.color("settings.headerForeground") } else { self.color("foreground") };
        let chevron = if editor.expanded.contains(&row.id) { "chevron-down" } else { "chevron-right" };
        let id = row.id.clone();
        div()
            .id(("settings-toc-row", index))
            .h(px(TOC_ROW_HEIGHT))
            .flex()
            .items_center()
            .cursor_default()
            .text_color(foreground)
            .when(!selected, |row| row.opacity(0.9))
            .when(focused, |row| row.border_1().border_color(self.color("list.focusOutline")))
            .hover(|row| row.text_color(self.color("settings.settingsHeaderHoverForeground")))
            // tree.css `.monaco-tl-twistie`: padding-left 8px per depth, 16px wide, padding-right 6px, shifted 3px right.
            .child(
                div()
                    .flex_none()
                    .pl(px(8. * row.level as f32))
                    .pr(px(6.))
                    .child(div().w(px(16.)).ml(px(3.)).mr(px(-3.)).when(row.has_children, |twistie| twistie.child(self.icon(chevron, 16., 16., self.foreground_or("icon.foreground"))))),
            )
            .child(div().truncate().line_height(px(TOC_ROW_HEIGHT)).when(selected, |label| label.font_weight(FontWeight::BOLD)).child(row.label.clone()))
            .when(search_mode, |entry| entry.child(div().ml(px(3.)).opacity(0.8).child(format!("({})", row.count))))
            .on_click(cx.listener(move |workbench, _, window, cx| {
                if let Some(position) = workbench.settings_editor.toc.iter().position(|row| row.id == id) {
                    if workbench.settings_editor.toc[position].has_children && !workbench.settings_editor.expanded.contains(&id) {
                        workbench.settings_editor.expanded.insert(id.clone());
                    }
                    workbench.focus_toc_row(position, window, cx);
                }
            }))
            .into_any_element()
    }

    /// `.settings-tree-container`: the group titles and setting items, at most 1400px wide.
    fn settings_tree(&self, narrow: bool, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let focused = self.settings_editor.tree_focus.is_focused(window) || self.settings_editor.control_focus.is_focused(window) || self.settings_editor.value_focus.is_focused(window);
        let rows = list(self.settings_editor.tree.clone(), cx.processor(move |workbench: &mut Workbench, index: usize, window, cx| workbench.settings_tree_row(index, focused, narrow, window, cx))).size_full();
        div()
            .id("settings-tree")
            .flex_1()
            .min_w_0()
            .h_full()
            .track_focus(&self.settings_editor.tree_focus)
            .on_key_down(cx.listener(|workbench, event: &KeyDownEvent, window, cx| workbench.setting_control_key(event, window, cx)))
            .child(rows)
    }

    fn settings_tree_row(&mut self, index: usize, list_focused: bool, narrow: bool, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let row = self.settings_editor.rows[index].clone();
        let contents = match &row {
            Row::Group { label, level, .. } => {
                let size = match level {
                    1 => 26.,
                    2 => 22.,
                    _ => 18.,
                };
                let focused = list_focused && self.settings_editor.focused == Some(index);
                div()
                    .w_full()
                    .p(px(10.))
                    .pl(px(15.))
                    .text_size(px(size))
                    // CSS `line-height: normal` of the system font, where GPUI defaults to phi.
                    .line_height(gpui::relative(1.2))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(self.color("settings.headerForeground"))
                    .when(focused, |title| title.border_1().border_color(self.color("settings.focusedRowBorder")))
                    .truncate()
                    .child(label.clone())
                    .into_any_element()
            }
            Row::Setting(setting, group) => self.setting_item(index, *setting, group, list_focused, window, cx),
        };
        div()
            .id(("settings-tree-row", index))
            .w_full()
            .cursor_default()
            .on_hover(cx.listener(move |workbench, hovered: &bool, _, cx| {
                let editor = &mut workbench.settings_editor;
                if *hovered {
                    editor.hovered = Some(index);
                } else if editor.hovered == Some(index) {
                    editor.hovered = None;
                }
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, _, window, cx| {
                if workbench.settings_editor.focused != Some(index) || !workbench.settings_editor.tree_focus.is_focused(window) {
                    workbench.settings_write_now(cx);
                    workbench.focus_settings_tree(Some(index), window, cx);
                }
            }))
            .child(div().w_full().max_w(px(1400.)).mx_auto().px(px(24.)).when(narrow, |row| row.pl(px(33.))).child(contents))
            .into_any_element()
    }

    /// `renderSettingElement`: the title with category, label and indicators, the description and the control.
    fn setting_item(&mut self, row: usize, index: usize, group: &str, list_focused: bool, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let editor = &self.settings_editor;
        let setting = &editor.model.settings[index];
        let inspection = &editor.inspections[index];
        let kind = value_type(setting);
        let is_bool = kind == ValueType::Boolean;
        let is_list = is_list_control(kind);
        let focused = list_focused && editor.focused == Some(row);
        let hovered = editor.hovered == Some(row);
        let (category, label) = setting_key_to_display_format(&setting.key, group, !setting.overrides.is_empty());
        let header = self.color("settings.headerForeground");
        let indicators = self.setting_indicators(setting, inspection);
        let title = div()
            .flex()
            .flex_wrap()
            .pb(px(2.))
            .child(div().font_weight(FontWeight::SEMIBOLD).opacity(0.9).when(!category.is_empty(), |category_label| category_label.child(format!("{category}: "))))
            .child(div().mr(px(7.)).font_weight(FontWeight::SEMIBOLD).text_color(header).child(label))
            .children(indicators);
        let description = self.setting_description(index, window, cx);
        let deprecation = setting.deprecation_message.clone();
        let mut contents = div()
            .relative()
            .pt(px(12.))
            .px(px(14.))
            .pb(px(if is_bool || is_list { 26. } else { 18. }))
            .rounded(px(6.))
            .border_1()
            .border_color(if focused { self.color("settings.focusedRowBorder") } else { transparent() })
            .when(focused, |contents| contents.bg(self.color("settings.focusedRowBackground")))
            .when(hovered && !focused, |contents| contents.bg(self.color("settings.rowHoverBackground")))
            .child(title);
        if inspection.is_configured {
            contents = contents.child(
                div()
                    .id(("setting-modified", index))
                    .absolute()
                    .left(px(5.))
                    .top(px(15.))
                    .bottom(px(if is_bool || is_list { 23. } else { 18. }))
                    .w(px(6.))
                    .border_l_2()
                    .border_color(self.color("settings.modifiedItemIndicator"))
                    .child(self.hover_target(("setting-modified-hover", index), HoverContent::Text("The setting has been configured in the current scope.".into()), HoverOptions::mouse())),
            );
        }
        if focused || hovered {
            contents = contents.child(
                self.icon("gear", 22., 16., self.foreground_or("icon.foreground"))
                    .id(("setting-toolbar", index))
                    .absolute()
                    .left(px(-22.))
                    .top(px(8.))
                    .rounded(px(5.))
                    .cursor_pointer()
                    .hover(|gear| gear.bg(self.color("toolbar.hoverBackground")))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        workbench.show_setting_context_menu(index, gpui::Bounds { origin: event.position, size: gpui::Size::default() }, window, cx);
                    }))
                    .child(self.action_hover("More Actions...", "settings.action.showContextMenu", super::hover::HoverOptions::mouse())),
            );
        }
        contents = if is_bool {
            contents.child(self.bool_control(index, inspection.value == json!(true), description, cx))
        } else {
            let read_only = inspection.has_policy_value;
            let control = self.setting_control(index, kind, window, cx);
            // shortcut: a disabled select or input box takes Chromium's disabled look; the port only blocks the mouse.
            let control = if read_only { div().occlude().child(control).into_any_element() } else { control };
            contents.child(description).child(div().mt(px(9.)).flex().when(is_list, |control_box| control_box.w_full()).child(control))
        };
        contents = contents.when_some(deprecation, |contents, message| {
            contents.child(div().mt(px(3.)).flex().gap(px(4.)).text_color(self.color("errorForeground")).child(self.icon("error", 16., 14., self.color("errorForeground"))).child(message))
        });
        contents
            .on_mouse_down(MouseButton::Right, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                workbench.focus_settings_tree(Some(row), window, cx);
                workbench.show_setting_context_menu(index, gpui::Bounds { origin: event.position, size: gpui::Size::default() }, window, cx);
            }))
            .into_any_element()
    }

    /// settingsEditorSettingIndicators.ts: Preview or Experimental, Advanced, then the parenthesized scope notes.
    fn setting_indicators(&self, setting: &crate::default_settings::Setting, inspection: &Inspection) -> Vec<AnyElement> {
        let mut indicators = Vec::new();
        let badge = |label: &'static str| {
            div().mr(px(4.)).px(px(4.)).pb(px(2.)).rounded(px(4.)).italic().bg(self.color("badge.background")).text_color(self.color("badge.foreground")).child(label).into_any_element()
        };
        if model::has_tag(setting, "preview") {
            indicators.push(badge("Preview"));
        } else if model::has_tag(setting, "experimental") {
            indicators.push(badge("Experimental"));
        }
        if model::has_tag(setting, model::ADVANCED_SETTING_TAG) {
            indicators.push(badge("Advanced"));
        }
        let mut notes = Vec::new();
        if setting.restricted {
            notes.push("Requires workspace trust".to_string());
        }
        if inspection.has_policy_value {
            // shortcut: the hover lacks the "View policy settings" action; type `@hasPolicy` in the search box instead.
            let hover = HoverContent::Text("This setting is managed by your organization and its actual value cannot be changed.".into());
            indicators.push(
                div()
                    .relative()
                    .flex()
                    .items_center()
                    .italic()
                    .opacity(0.9)
                    .child(self.icon("briefcase", 16., 14., self.foreground_or("foreground")))
                    .child("Managed by your organization")
                    .child(div().absolute().inset_0().child(self.hover_target(gpui::ElementId::Name(format!("setting-policy-{}", setting.key).into()), hover, HoverOptions::mouse())))
                    .into_any_element(),
            );
        } else if !inspection.overridden_scopes.is_empty() {
            let scopes: Vec<String> = inspection.overridden_scopes.iter().map(|scope| scope_label(scope)).collect();
            let prefix = if inspection.is_configured { "Also modified in" } else { "Modified in" };
            notes.push(format!("{prefix}: {}", scopes.join(", ")));
        }
        if !inspection.overridden_default_languages.is_empty() {
            notes.push("Default value changed".into());
        }
        if !notes.is_empty() {
            indicators.push(div().italic().opacity(0.9).child(format!("({})", notes.join(" \u{2022} "))).into_any_element());
        }
        indicators
    }

    /// `renderSettingMarkdown` or the plain description: code spans in the monospace font on `textPreformat`, links open their target.
    fn setting_description(&self, index: usize, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let setting = &self.settings_editor.model.settings[index];
        let source = setting.description.join("\n");
        let (text, runs) = if setting.description_is_markdown { markdown_runs(&model::fix_setting_links(&source, true)) } else { (source, Vec::new()) };
        let foreground = self.color("foreground");
        let base = window.text_style().to_run(0);
        let plain = TextRun { color: foreground.into(), ..base.clone() };
        let link = TextRun { color: self.color("textLink.foreground").into(), ..base.clone() };
        let code = TextRun {
            font: font("Menlo"),
            color: self.color("textPreformat.foreground").into(),
            background_color: Some(self.color("textPreformat.background").into()),
            ..base
        };
        let mut text_runs = Vec::new();
        let mut links = Vec::new();
        let mut hrefs = Vec::new();
        let mut end = 0;
        for (range, run) in runs {
            if range.start > end {
                text_runs.push(TextRun { len: range.start - end, ..plain.clone() });
            }
            let style = match run {
                MarkdownRun::Code => &code,
                MarkdownRun::Link(href) => {
                    links.push(range.clone());
                    hrefs.push(href);
                    &link
                }
            };
            text_runs.push(TextRun { len: range.len(), ..style.clone() });
            end = range.end;
        }
        if text.len() > end {
            text_runs.push(TextRun { len: text.len() - end, ..plain });
        }
        let weak = cx.entity().downgrade();
        let text = InteractiveText::new(("setting-description", index), StyledText::new(text).with_runs(text_runs)).on_click(links, move |link, window, cx| {
            let href = hrefs[link].clone();
            let _ = weak.update(cx, |workbench, cx| workbench.open_setting_link(&href, window, cx));
        });
        // `.monaco-list-row` line-height 1.4em. shortcut: GPUI text runs carry no font size, so code spans keep 13px
        // where settingsEditor2.css sets 11px.
        div().mt(px(-1.)).line_height(gpui::relative(1.4)).opacity(0.9).child(text).into_any_element()
    }

    /// `onDidClickSettingLink` for `#key` links, the opener service otherwise.
    fn open_setting_link(&mut self, href: &str, window: &mut Window, cx: &mut Context<Self>) {
        match href.strip_prefix('#') {
            Some(key) => self.reveal_setting_link(key, window, cx),
            None => self.open_notification_link(href, window, cx),
        }
    }

    /// `SettingBoolRenderer`: the 18px checkbox before the description; a press on either toggles it.
    fn bool_control(&self, index: usize, checked: bool, description: AnyElement, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let border = self.theme.color("settings.checkboxBorder").unwrap_or(transparent());
        let checkbox = div()
            .size(px(18.))
            .flex_none()
            .mr(px(9.))
            .rounded(px(3.))
            .border_1()
            .border_color(border)
            .bg(self.color("settings.checkboxBackground"))
            .when(checked, |checkbox| checkbox.child(self.icon("check", 16., 16., self.color("settings.checkboxForeground"))));
        let row = div().id(("setting-bool", index)).mt(px(9.)).flex();
        // `.monaco-custom-toggle.disabled` under a policy: dimmed, the row does not toggle it.
        let row = match self.settings_editor.inspections[index].has_policy_value {
            true => row.child(checkbox.opacity(0.4)),
            false => row.cursor_pointer().on_click(cx.listener(move |workbench, _, _, cx| workbench.write_setting(index, Some(json!(!checked)), false, cx))).child(checkbox),
        };
        row
            .child(div().flex_1().min_w_0().child(description))
    }

    /// The control of a non-boolean setting: a select, a text input, a list of values, or Edit in settings.json.
    fn setting_control(&mut self, index: usize, kind: ValueType, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match kind {
            ValueType::Enum => self.enum_control(index, cx),
            kind if is_text_control(kind) => self.text_control(index, kind, window, cx),
            kind if is_list_control(kind) => self.list_widget(index, kind, window, cx),
            _ => self.edit_in_settings_json(index, cx),
        }
    }

    /// `.edit-in-settings-button`: opens the target's settings.json at the setting.
    /// `SettingComplexRenderer.renderValidations`: a configured value of the wrong type shows its error.
    fn edit_in_settings_json(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let setting = &self.settings_editor.model.settings[index];
        let inspection = &self.settings_editor.inspections[index];
        let error = inspection.is_configured.then(|| invalid_type_error(&inspection.value, &setting.schema["type"])).flatten();
        let key = setting.key.clone();
        let link = div()
            .id(("edit-in-settings", index))
            .mt(px(9.))
            .text_color(self.color("textLink.foreground"))
            .cursor_pointer()
            .hover(|link| link.underline().text_color(self.color("textLink.activeForeground")))
            .child("Edit in settings.json")
            .on_click(cx.listener(move |workbench, _, window, cx| {
                let command = match workbench.settings_editor.target {
                    Target::Workspace => "workbench.action.openWorkspaceSettingsFile",
                    _ => "workbench.action.openSettingsJson",
                };
                workbench.close_settings_editor(window, cx);
                let outcome = workbench.execute_command(command, vec![json!({ "revealSetting": { "key": key, "edit": true } })], window, cx);
                super::keyboard::report_failure(outcome, cx);
            }));
        let message = error.map(|error| self.validation_message(error).w(px(420.)));
        div().relative().child(link).children(message.map(|message| deferred(anchored().position_mode(gpui::AnchoredPositionMode::Local).offset(point(px(0.), px(31.))).child(message)).with_priority(1))).into_any_element()
    }

    /// `SettingEnumRenderer`: a 320px select box; its dropdown lists the options with the highlighted one's description.
    fn enum_control(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let setting = &self.settings_editor.model.settings[index];
        let value = &self.settings_editor.inspections[index].value;
        let options = setting.enum_values.clone().unwrap_or_default();
        let label = text_of(value);
        let dropdown = self.settings_editor.dropdown.filter(|_| self.settings_editor.focused_setting() == Some(index));
        let select = div()
            .id(("setting-enum", index))
            .w(px(320.))
            .h(px(26.))
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
            .child(div().truncate().child(label))
            .child(self.icon("chevron-down", 16., 16., self.color("settings.dropdownForeground")))
            .on_click(cx.listener(move |workbench, _, window, cx| {
                let selected = workbench.selected_enum_option(index);
                workbench.focus_setting_control(window, cx);
                workbench.settings_editor.dropdown = Some(selected);
                cx.notify();
            }));
        let Some(highlighted) = dropdown else { return select.into_any_element() };
        let descriptions = setting.enum_descriptions.clone().unwrap_or_default();
        let items = options.iter().enumerate().map(|(option, value)| {
            let active = option == highlighted;
            div()
                .id(("setting-enum-option", option))
                .h(px(22.))
                .pl(px(6.))
                .flex()
                .items_center()
                .cursor_pointer()
                .when(active, |item| item.bg(self.color("list.activeSelectionBackground")).text_color(self.foreground_or("list.activeSelectionForeground")))
                .child(text_of(value))
                .on_click(cx.listener(move |workbench, _, window, cx| workbench.select_enum_option(index, option, window, cx)))
        });
        let description = descriptions.get(highlighted).and_then(Value::as_str).map(str::to_string);
        let menu = div()
            .w(px(320.))
            .flex()
            .flex_col()
            .py(px(2.))
            .rounded(px(8.))
            .border_1()
            .border_color(self.theme.color("settings.dropdownListBorder").or(self.theme.color("settings.dropdownBorder")).unwrap_or(transparent()))
            .bg(self.color("settings.dropdownBackground"))
            .text_color(self.color("settings.dropdownForeground"))
            .shadow_lg()
            .children(items)
            .when_some(description, |menu, description| menu.child(div().px(px(6.)).py(px(5.)).border_t_1().border_color(self.color("settings.dropdownBorder")).text_size(px(12.)).child(description)));
        div()
            .relative()
            .child(select)
            .child(deferred(anchored().position_mode(gpui::AnchoredPositionMode::Local).offset(point(px(0.), px(26.))).child(menu)).with_priority(2))
            .into_any_element()
    }

    fn selected_enum_option(&self, index: usize) -> usize {
        let value = &self.settings_editor.inspections[index].value;
        self.settings_editor.model.settings[index].enum_values.as_ref().and_then(|values| values.iter().position(|option| option == value)).unwrap_or(0)
    }

    fn select_enum_option(&mut self, index: usize, option: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.settings_editor.dropdown = None;
        let value = self.settings_editor.model.settings[index].enum_values.as_ref().and_then(|values| values.get(option).cloned());
        self.write_setting(index, value, false, cx);
        let control = self.settings_editor.control_focus.clone();
        window.focus(&control, cx);
    }

    /// `.setting-item-validation-message` of an `invalid-input` setting.
    pub(super) fn validation_message(&self, error: String) -> gpui::Div {
        div()
            .p(px(5.))
            .bg(self.color("inputValidation.errorBackground"))
            .text_color(self.foreground_or("inputValidation.errorForeground"))
            .border_1()
            .border_color(self.color("inputValidation.errorBorder"))
            .child(error)
    }

    /// `SettingTextRenderer` (420px) and `SettingNumberRenderer` (200px min), with the validation message.
    fn text_control(&self, index: usize, kind: ValueType, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let editor = &self.settings_editor;
        let number = !matches!(kind, ValueType::String | ValueType::MultilineString);
        let (background, foreground, border) = match number {
            true => ("settings.numberInputBackground", "settings.numberInputForeground", "settings.numberInputBorder"),
            false => ("settings.textInputBackground", "settings.textInputForeground", "settings.textInputBorder"),
        };
        let editing = editor.value.as_ref().filter(|(editing, _)| *editing == index);
        let focused = editing.is_some() && editor.value_focus.is_focused(window);
        let schema = &editor.model.settings[index].schema;
        let shown = editing.map_or_else(|| text_of(&editor.inspections[index].value), |(_, input)| input.value.clone());
        let error = parse_typed_value(schema, kind, &shown).err();
        let border = match (&error, focused) {
            (Some(_), _) => self.color("inputValidation.errorBorder"),
            (None, true) => self.color("focusBorder"),
            (None, false) => self.theme.color(border).unwrap_or(transparent()),
        };
        let text = match editing {
            Some(_) => self.input_text(InputId::SettingsValue, InputLook { password: false, placeholder: None, focused }, cx).into_any_element(),
            None => div().truncate().child(text_of(&editor.inspections[index].value)).into_any_element(),
        };
        let input = div()
            .id(("setting-text", index))
            .when(number, |input| input.min_w(px(200.)))
            .when(!number, |input| input.w(px(420.)))
            .h(px(26.))
            .px(px(4.))
            .flex()
            .items_center()
            .rounded(px(2.))
            .border_1()
            .border_color(border)
            .bg(self.color(background))
            .text_color(self.color(foreground))
            .cursor_text()
            .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, _, window, cx| {
                if workbench.settings_editor.value.as_ref().is_some_and(|(editing, _)| *editing == index) {
                    return;
                }
                workbench.focus_setting_control(window, cx);
            }))
            .child(text);
        // settingsEditor2.css: the message is absolute under the input, over the next row.
        div()
            .relative()
            .child(input)
            .when_some(error, |control, error| {
                let message = self.validation_message(error).w(px(if number { 200. } else { 420. }));
                control.child(deferred(anchored().position_mode(gpui::AnchoredPositionMode::Local).offset(point(px(0.), px(25.))).child(message)).with_priority(1))
            })
            .into_any_element()
    }

    /// Keys on the focused control that no keybinding took: space toggles a checkbox, the dropdown moves and picks.
    fn setting_control_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let editor = &mut self.settings_editor;
        if !editor.control_focus.is_focused(window) {
            return;
        }
        let Some(index) = editor.focused_setting() else { return };
        let key = event.keystroke.key.as_str();
        match value_type(&editor.model.settings[index]) {
            ValueType::Boolean if key == "space" || key == "enter" => {
                let checked = editor.inspections[index].value == json!(true);
                cx.stop_propagation();
                self.write_setting(index, Some(json!(!checked)), false, cx);
            }
            ValueType::Enum => {
                let count = editor.model.settings[index].enum_values.as_ref().map_or(0, Vec::len);
                let dropdown = editor.dropdown;
                cx.stop_propagation();
                match (dropdown, key) {
                    (None, "space" | "enter" | "down" | "up") => self.settings_editor.dropdown = Some(self.selected_enum_option(index)),
                    (Some(option), "enter" | "space") => self.select_enum_option(index, option, window, cx),
                    (Some(option), "down") => self.settings_editor.dropdown = Some((option + 1).min(count.saturating_sub(1))),
                    (Some(option), "up") => self.settings_editor.dropdown = Some(option.saturating_sub(1)),
                    (Some(_), "escape") => self.settings_editor.dropdown = None,
                    _ => {}
                }
                cx.notify();
            }
            _ => {}
        }
    }
}

fn editor_focused(workbench: &Workbench) -> Option<usize> {
    workbench.settings_editor.focused
}

fn setting_arg(workbench: &Workbench, args: &[Value]) -> Option<usize> {
    let key = args.first()?.as_str()?;
    workbench.settings_editor.model.index_of(key)
}

/// `sanitizeOpenSettingsArgs`: a string argument is the query.
fn open_settings_query(args: &[Value]) -> Option<String> {
    match args.first()? {
        Value::String(query) => Some(query.clone()),
        Value::Object(options) => options.get("query").and_then(Value::as_str).map(str::to_string),
        _ => None,
    }
}

fn is_text_control(kind: ValueType) -> bool {
    matches!(kind, ValueType::String | ValueType::MultilineString | ValueType::Integer | ValueType::Number | ValueType::NullableInteger | ValueType::NullableNumber)
}

/// A value as its control shows it: strings bare, null empty, others as JSON.
fn text_of(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// The rows of a list setting's widget: array items, or object keys with their values.
fn is_list_control(kind: ValueType) -> bool {
    matches!(kind, ValueType::Array | ValueType::Exclude | ValueType::Include | ValueType::Object | ValueType::BooleanObject)
}

/// `getLabel` of an overridden scope: `User`, `Workspace`, or `<language> > <scope>`.
fn scope_label(scope: &str) -> String {
    let (target, language) = scope.split_once(':').unwrap_or((scope, ""));
    let target = if target == "workspace" { "Workspace" } else { "User" };
    if language.is_empty() { target.to_string() } else { format!("{language} > {target}") }
}

fn transparent() -> Rgba {
    gpui::rgba(0)
}
