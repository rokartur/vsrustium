//! `EditSettingRenderer` of workbench/contrib/preferences/browser/preferencesRenderers.ts: in a settings file, the
//! pencil in the glyph margin of a boolean or enum setting under the cursor (`editPreferenceWidgetForCursorPosition`)
//! or under the pointer after a 75 ms pause (`editPreferenceWidgetForMouseMove`); a click lists the setting's values
//! and writes the one picked into the file (`UserSettingsRenderer.updatePreference`).

use std::time::Duration;

use config::json::Segment;
use config::json_edit::{FormattingOptions, set_property};
use config::registry::Scope;
use config::service::Target;
use gpui::{Context, Div, MouseButton, MouseDownEvent, MouseMoveEvent, Task, Window, div, prelude::*, px};
use serde_json::{Value, json};

use super::super::Workbench;
use super::unsupported_settings::{ParsedSetting, parse_settings, settings_file_target};
use crate::commands::Outcome;
use crate::editor::{EditReason, OpenEditor, Position, Selection};
use crate::menus::MenuAction;

/// `toggleEditPreferencesForMouseMoveDelayer`.
const MOUSE_MOVE_DELAY: Duration = Duration::from_millis(75);

#[derive(Default)]
pub(in crate::workbench) struct EditSetting {
    /// The pointer's pencil: the model, its 1-based line, and the cursor it showed under (a cursor change hides it).
    mouse: Option<(u32, usize, Selection)>,
    /// The cursor's pencil was hidden by the pointer's while the cursor was this one.
    cursor_hidden: Option<(u32, Selection)>,
    toggle: Option<Task<()>>,
    /// `SettingHighlighter`: the setting's lines while the pointer is on its pencil, and after an update while the
    /// selection set on its value stays.
    hovered: Option<(u32, usize, usize)>,
    fixed: Option<(u32, usize, usize, Selection)>,
    /// The open menu's items: the setting each one writes and its value.
    menu: Vec<(EditableSetting, Value)>,
    commands: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub(in crate::workbench) struct EditableSetting {
    key: String,
    /// The `[language]` key of an override setting.
    override_of: Option<String>,
    /// 1-based lines of the setting and the line its value starts on.
    first_line: usize,
    last_line: usize,
    value_line: usize,
}

/// `getSettingsAtLineNumber`: the settings whose range holds `line` (1-based); inside a `[language]` block with
/// settings, its settings instead of the block.
fn settings_at_line(settings: &[ParsedSetting], line: usize) -> Vec<EditableSetting> {
    let on_line = |setting: &ParsedSetting| setting.range[0] <= line && line <= setting.range[2];
    let editable = |setting: &ParsedSetting, override_of: Option<String>| EditableSetting { key: setting.key.clone(), override_of, first_line: setting.range[0], last_line: setting.range[2], value_line: setting.value_range[0] };
    let mut found = Vec::new();
    for setting in settings.iter().filter(|setting| on_line(setting)) {
        match setting.overrides.is_empty() {
            true => found.push(editable(setting, None)),
            false => found.extend(setting.overrides.iter().filter(|setting| on_line(setting)).map(|nested| editable(nested, Some(setting.key.clone())))),
        }
    }
    found
}

/// `getActions`: `true` and `false` for a boolean, the JSON of each enum value, else nothing.
fn setting_values(schema: &Value) -> Vec<(String, Value)> {
    if schema["type"] == "boolean" {
        return vec![("true".into(), json!(true)), ("false".into(), json!(false))];
    }
    schema["enum"].as_array().into_iter().flatten().map(|value| (value.to_string(), value.clone())).collect()
}

impl Workbench {
    /// `getSettings`: the settings at `line` of `editor` the pencil can edit: known, not set by policy, a boolean or an
    /// enum, and in a folder's settings of a workspace only resource or language overridable ones.
    fn editable_settings(&self, editor: &OpenEditor, line: usize) -> Vec<EditableSetting> {
        let document = editor.document();
        if document.scheme != "file" {
            return Vec::new();
        }
        let settings = self.settings.lock().unwrap();
        let Some((target, in_workspace_file)) = settings_file_target(&settings, &document.path) else { return Vec::new() };
        let parsed = parse_settings(&document.lines.join("\n"), in_workspace_file);
        let mut editable = settings_at_line(&parsed, line);
        editable.retain(|setting| {
            let Some(property) = settings.registry.property(&setting.key) else { return false };
            if !property.schema["policy"].is_null() && settings.configuration.policy.get_value(Some(&setting.key)).is_some() {
                return false;
            }
            let scoped = target != Target::WorkspaceFolder || matches!(property.scope, Scope::Resource | Scope::LanguageOverridable);
            !setting_values(&property.schema).is_empty() && scoped
        });
        editable
    }

    /// Whether `editor` edits a settings file, whose lines track the pointer for the pencil.
    pub(in crate::workbench) fn edits_settings_file(&self, editor: &OpenEditor) -> bool {
        let document = editor.document();
        document.scheme == "file" && settings_file_target(&self.settings.lock().unwrap(), &document.path).is_some()
    }

    /// The line (1-based) of `editor`'s pencil, if one shows: the pointer's, else the cursor's unless the pointer's
    /// hid it (`showEditPreferencesWidget` with glyph margins on).
    pub(in crate::workbench) fn edit_setting_line(&self, editor: &OpenEditor) -> Option<usize> {
        if self.setting("editor.glyphMargin") == json!(false) {
            return None;
        }
        let state = &self.edit_setting;
        let selection = editor.selection();
        if let Some((_, line, _)) = state.mouse.filter(|(model, _, shown_under)| *model == editor.model_id && *shown_under == selection) {
            return Some(line);
        }
        if state.cursor_hidden == Some((editor.model_id, selection)) {
            return None;
        }
        Some(self.editable_settings(editor, selection.head.line + 1).first()?.value_line)
    }

    /// The settings highlight on line `index` (0-based) of `editor`: `rangeHighlight` across the line.
    pub(in crate::workbench) fn edit_setting_highlights(&self, editor: &OpenEditor, index: usize) -> bool {
        let state = &self.edit_setting;
        let line = index + 1;
        let hovered = state.hovered.is_some_and(|(model, first, last)| model == editor.model_id && first <= line && line <= last);
        let fixed = state.fixed.is_some_and(|(model, first, last, selection)| model == editor.model_id && editor.selection() == selection && first <= line && line <= last);
        hovered || fixed
    }

    /// `onMouseMoved` off the pencil: the highlight goes, and after the pause the pointer's line gets its pencil.
    pub(in crate::workbench) fn edit_setting_mouse_moved(&mut self, model: u32, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.edit_setting.hovered.take().is_some() {
            cx.notify();
        }
        self.edit_setting.toggle = Some(cx.spawn_in(window, async move |workbench, cx| {
            cx.background_executor().timer(MOUSE_MOVE_DELAY).await;
            let _ = workbench.update(cx, |workbench, cx| workbench.toggle_edit_setting_for_mouse(model, index + 1, cx));
        }));
    }

    fn toggle_edit_setting_for_mouse(&mut self, model: u32, line: usize, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.all_editors().find(|editor| editor.model_id == model) else { return };
        let selection = editor.selection();
        let shown = self.editable_settings(editor, line).first().map(|setting| setting.value_line);
        let state = &mut self.edit_setting;
        match shown {
            Some(line) => {
                state.mouse = Some((model, line, selection));
                state.cursor_hidden = Some((model, selection));
            }
            None => state.mouse = None,
        }
        cx.notify();
    }

    /// `EditPreferenceWidget`: the `settings-edit` codicon in the glyph margin of line `index`; the pointer on it
    /// highlights its setting, a press shows the values.
    pub(in crate::workbench) fn edit_setting_glyph(&self, editor: &OpenEditor, index: usize, glyph_margin_width: f32, line_height: f32, shown_line: Option<usize>, cx: &mut Context<Self>) -> Option<Div> {
        if shown_line != Some(index + 1) {
            return None;
        }
        let model = editor.model_id;
        let icon = self.icon("edit", glyph_margin_width, 16., self.color("editor.foreground")).h(px(line_height));
        let glyph = div()
            .absolute()
            .top_0()
            .left_0()
            .cursor_pointer()
            .child(icon)
            .on_mouse_move(cx.listener(move |workbench, _: &MouseMoveEvent, _, cx| {
                cx.stop_propagation();
                workbench.edit_setting.toggle = None;
                let Some(editor) = workbench.editors.all_editors().find(|editor| editor.model_id == model) else { return };
                let setting = workbench.editable_settings(editor, index + 1).into_iter().next();
                let hovered = setting.map(|setting| (model, setting.first_line, setting.last_line));
                if workbench.edit_setting.hovered != hovered {
                    workbench.edit_setting.hovered = hovered;
                    cx.notify();
                }
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                workbench.show_edit_setting_menu(model, index + 1, event.position, window, cx);
            }));
        Some(glyph)
    }

    /// `onEditSettingClicked`: one setting's values, or a submenu of values per setting on the line.
    pub(in crate::workbench) fn show_edit_setting_menu(&mut self, model: u32, line: usize, position: gpui::Point<gpui::Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.all_editors().find(|editor| editor.model_id == model) else { return };
        let settings = self.editable_settings(editor, line);
        let mut items = Vec::new();
        let mut menus = Vec::new();
        for setting in &settings {
            let schema = self.settings.lock().unwrap().registry.property(&setting.key).map(|property| property.schema.clone()).unwrap_or_default();
            let mut actions = Vec::new();
            for (label, value) in setting_values(&schema) {
                actions.push(MenuAction::Command { id: format!("editSetting.value.{}", items.len()), title: label, enabled: true, checked: false, alt: None });
                items.push((setting.clone(), value));
            }
            menus.push((setting.key.clone(), actions));
        }
        while self.edit_setting.commands < items.len() {
            let index = self.edit_setting.commands;
            self.commands.register(format!("editSetting.value.{index}"), move |workbench, _, _, cx| workbench.update_edit_setting(index, cx));
            self.edit_setting.commands += 1;
        }
        self.edit_setting.menu = items;
        let actions = match menus.len() {
            1 => menus.remove(0).1,
            _ => menus.into_iter().map(|(key, actions)| MenuAction::Submenu { title: key, actions, checked: false }).collect(),
        };
        let anchor = gpui::Bounds { origin: position, size: gpui::Size::default() };
        let context = self.key_context(window);
        self.show_context_menu_actions(actions, &context, anchor, Vec::new(), window, cx);
    }

    /// `updatePreference` through configurationEditing.ts: the value written into the file's model, which is saved;
    /// then `onSettingUpdated` selects the value and highlights the setting.
    fn update_edit_setting(&mut self, index: usize, cx: &mut Context<Self>) -> Outcome {
        let (setting, value) = self.edit_setting.menu[index].clone();
        let Some(editor) = self.editors.active() else { return Outcome::done() };
        let document = editor.document();
        let in_workspace_file = settings_file_target(&self.settings.lock().unwrap(), &document.path).is_some_and(|(_, in_workspace_file)| in_workspace_file);
        let mut path: Vec<Segment> = setting.override_of.iter().chain([&setting.key]).map(|key| Segment::Key(key.clone())).collect();
        if in_workspace_file {
            path.insert(0, Segment::Key("settings".into()));
        }
        let text = document.lines.join("\n");
        let options = FormattingOptions { tab_size: document.indentation.tab_size, insert_spaces: document.indentation.insert_spaces, eol: document.eol.to_string() };
        let edits: Vec<_> = set_property(&text, &path, Some(&value), &options)
            .into_iter()
            .map(|edit| (super::position_at(&text, edit.offset), super::position_at(&text, edit.offset + edit.length), edit.content.replace("\r\n", "\n"), false))
            .collect();
        let uri = document.uri();
        drop(document);
        self.run_indenting(|editor, _| editor.execute_edits(edits, None, EditReason::Unknown("configurationEditing")).into_iter().collect(), cx);
        drop(self.save_resource(&uri, cx));
        let editor = self.editors.active_mut().expect("the settings editor is active");
        let text = editor.document().lines.join("\n");
        let parsed = parse_settings(&text, in_workspace_file);
        let updated = parsed.iter().find(|parsed| Some(&parsed.key) == setting.override_of.as_ref().or(Some(&setting.key))).and_then(|parsed| match &setting.override_of {
            Some(_) => parsed.overrides.iter().find(|nested| nested.key == setting.key),
            None => Some(parsed),
        });
        if let Some(updated) = updated {
            let [line, column, end_line, end_column] = updated.value_range;
            let selection = Selection { anchor: Position::new(line - 1, column - 1), head: Position::new(end_line - 1, end_column - 1) };
            editor.set_selections(vec![selection]);
            self.edit_setting.fixed = Some((editor.model_id, updated.range[0], updated.range[2], selection));
        }
        cx.notify();
        Outcome::done()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(text: &str) -> Vec<ParsedSetting> {
        parse_settings(text, false)
    }

    #[test]
    fn the_setting_on_a_line_is_found_by_its_range() {
        let text = "{\n  \"editor.wordWrap\": \"on\",\n  \"files.exclude\": {\n    \"a\": true\n  }\n}";
        let at = |line| settings_at_line(&parsed(text), line).into_iter().map(|setting| (setting.key, setting.value_line)).collect::<Vec<_>>();
        assert_eq!(at(2), [("editor.wordWrap".to_string(), 2)]);
        assert_eq!(at(4), [("files.exclude".to_string(), 3)]);
        assert_eq!(at(1), []);
    }

    #[test]
    fn inside_a_language_block_its_settings_are_found() {
        let text = "{\n  \"[json]\": {\n    \"editor.wordWrap\": \"on\"\n  },\n  \"[css]\": {}\n}";
        let found = settings_at_line(&parsed(text), 3);
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].key.as_str(), found[0].override_of.as_deref()), ("editor.wordWrap", Some("[json]")));
        assert_eq!(settings_at_line(&parsed(text), 5)[0].key, "[css]");
    }

    #[test]
    fn booleans_and_enums_have_values() {
        assert_eq!(setting_values(&json!({ "type": "boolean" })).into_iter().map(|(label, _)| label).collect::<Vec<_>>(), ["true", "false"]);
        assert_eq!(setting_values(&json!({ "type": "string", "enum": ["off", "on"] })).into_iter().map(|(label, _)| label).collect::<Vec<_>>(), ["\"off\"", "\"on\""]);
        assert!(setting_values(&json!({ "type": "string" })).is_empty());
    }
}
