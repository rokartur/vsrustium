//! The color theme picker (workbench/contrib/themes/browser/themes.contribution.ts `SelectColorThemeCommandId`
//! and `InstalledThemesPicker`) and `setColorTheme` of workbenchThemeService.ts.

use std::time::Duration;

use gpui::{Context, Task, Window};
use serde_json::json;

use super::Workbench;
use super::quick_access::show_failure;
use crate::command_palette::locale_compare;
use crate::commands::Outcome;
use crate::main_thread_quick_open::Owner;
use crate::quick_input::{Button, Entry, Event, Item, Kind, QuickInput};
use crate::theme::{ColorThemeEntry, ThemeKind};

const COLOR_THEME_SETTING: &str = "workbench.colorTheme";
/// `ThemeSettingDefaults.COLOR_THEME_DARK` and `COLOR_THEME_LIGHT`: pinned first in their group.
const DEFAULT_DARK: &str = "Dark 2026";
const DEFAULT_LIGHT: &str = "Light 2026";
/// `selectTheme`: a theme is previewed once the focus rested on it this long.
const PREVIEW_DELAY: Duration = Duration::from_millis(200);

/// `ThemeSettingTarget`: `Preview` shows the theme without writing `workbench.colorTheme`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SettingTarget {
    Preview,
    Auto,
}

/// What the open picker remembers: the theme to restore, the settings id of each item by handle, and the
/// preview waiting for its delay.
pub struct ColorThemePicker {
    current: String,
    picks: Vec<String>,
    completed: bool,
    preview: Option<Task<()>>,
}

impl Workbench {
    pub(super) fn register_theme_commands(&mut self) {
        self.commands.register("workbench.action.selectTheme", |workbench, _, window, cx| {
            workbench.show_color_theme_picker(window, cx);
            Outcome::done()
        });
    }

    /// `InstalledThemesPicker.openQuickPick` with the picks of the `selectTheme` action.
    fn show_color_theme_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (entries, picks) = color_theme_entries(&self.color_themes);
        let current = self.theme.settings_id.clone();
        let mut input = QuickInput::new(0, Kind::QuickPick);
        // getTitle with `getPreferredColorScheme()` undefined: the port does not read window.autoDetectColorScheme.
        input.placeholder = Some("Select Color Theme (detect system color mode disabled)".into());
        input.match_on_description = true;
        input.buttons = vec![Button {
            handle: 0,
            codicon: Some("color-mode".into()),
            tooltip: Some("Detect system color mode disabled. Click to configure.".into()),
            location: Some(2),
            ..Button::default()
        }];
        input.set_items(entries);
        if let Some(handle) = picks.iter().position(|id| *id == current) {
            input.set_active_items(&[handle as i64]);
        }
        let picker = ColorThemePicker { current, picks, completed: false, preview: None };
        let id = self.quick_inputs.add(input, Owner::ColorThemes(picker));
        self.show_quick_input(id, window, cx);
        self.flush_quick_input_events(window, cx);
    }

    /// The picker's listeners: moving previews, accepting keeps the theme, hiding otherwise restores it.
    pub fn color_theme_picker_event(&mut self, id: i64, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Owner::ColorThemes(picker)) = self.quick_inputs.owner(id) else { return };
        match event {
            Event::ChangeActive(handles) => {
                let theme = handles.first().map_or_else(|| picker.current.clone(), |handle| picker.picks[*handle as usize].clone());
                let preview = cx.spawn_in(window, async move |workbench, cx| {
                    cx.background_executor().timer(PREVIEW_DELAY).await;
                    let _ = workbench.update(cx, |workbench, cx| workbench.set_color_theme(&theme, SettingTarget::Preview, cx));
                });
                if let Some(Owner::ColorThemes(picker)) = self.quick_inputs.owner_mut(id) {
                    picker.preview = Some(preview);
                }
            }
            Event::Accept => {
                let Some(input) = self.quick_inputs.get(id) else { return };
                let Some(&handle) = input.selected_handles().first() else { return };
                let theme = picker.picks[handle as usize].clone();
                if let Some(Owner::ColorThemes(picker)) = self.quick_inputs.owner_mut(id) {
                    picker.completed = true;
                    picker.preview = None;
                }
                self.set_color_theme(&theme, SettingTarget::Auto, cx);
                self.hide_quick_input_by_id(id, window, cx);
            }
            Event::TriggerButton(..) => {
                self.hide_quick_input_by_id(id, window, cx);
                // `preferencesService.openSettings({ query })` runs as the command the Settings editor registers.
                let outcome = self.execute_command("workbench.action.openSettings", vec![json!("window.autoDetectColorScheme")], window, cx);
                show_failure("Preferences: Open Settings (UI)".into(), outcome, window, cx);
            }
            Event::TriggerItemButton(handle, ..) => {
                let theme = &picker.picks[handle as usize];
                let extension = self.color_themes.iter().find(|entry| entry.settings_id == *theme).map(|entry| entry.extension_id.clone());
                let Some(extension) = extension else { return };
                // `extensionsWorkbenchService.openSearch` runs as the command the Extensions view registers.
                let outcome = self.execute_command("workbench.extensions.search", vec![json!(format!("@id:{extension}"))], window, cx);
                show_failure("Search Extensions".into(), outcome, window, cx);
            }
            Event::Hide => {
                let restore = (!picker.completed).then(|| picker.current.clone());
                self.quick_inputs.remove(id);
                if let Some(theme) = restore {
                    self.set_color_theme(&theme, SettingTarget::Auto, cx);
                }
            }
            Event::ChangeValue(_) | Event::ChangeSelection(_) => {}
        }
    }

    /// `WorkbenchThemeService.setColorTheme`: applies the theme, tells the extension host its kind, and writes
    /// `workbench.colorTheme` unless previewing (`ThemeConfiguration.writeConfiguration`). A theme that fails to
    /// load leaves the current one, as the picker's error path does.
    pub fn set_color_theme(&mut self, settings_id: &str, target: SettingTarget, cx: &mut Context<Self>) {
        if self.theme.settings_id != settings_id {
            let Some(entry) = self.color_themes.iter().find(|entry| entry.settings_id == settings_id) else { return };
            let theme = match entry.load() {
                Ok(theme) => theme,
                Err(error) => {
                    eprintln!("loading color theme {settings_id}: {error:#}");
                    return;
                }
            };
            self.theme = theme;
            for editor in &mut self.editors.editors {
                editor.document.tokens.theme_changed();
            }
            self.host.send_color_theme(self.theme.kind);
            cx.notify();
        }
        if target == SettingTarget::Preview {
            return;
        }
        let mut settings = self.settings.lock().unwrap();
        match settings.update_value_derived(COLOR_THEME_SETTING, Some(&json!(settings_id))) {
            Ok(change) if !change.is_empty() => self.host.send_configuration_change(&settings, &change),
            Ok(_) => {}
            Err(error) => eprintln!("writing {COLOR_THEME_SETTING}: {error}"),
        }
    }
}

/// The `selectTheme` picks without a preferred color scheme: light, dark, then high contrast themes, each
/// group headed by a separator (`toEntries`), with the settings id of each item by handle.
fn color_theme_entries(themes: &[ColorThemeEntry]) -> (Vec<Entry>, Vec<String>) {
    let groups: [(&str, fn(ThemeKind) -> bool); 3] = [
        ("light themes", |kind| kind == ThemeKind::Light),
        ("dark themes", |kind| kind == ThemeKind::Dark),
        ("high contrast themes", ThemeKind::is_high_contrast),
    ];
    let (mut entries, mut picks) = (Vec::new(), Vec::new());
    for (label, in_group) in groups {
        let mut group: Vec<&ColorThemeEntry> = themes.iter().filter(|theme| in_group(theme.kind)).collect();
        group.sort_by(|a, b| is_pinned(b).cmp(&is_pinned(a)).then_with(|| locale_compare(&a.label, &b.label)));
        if !group.is_empty() {
            entries.push(Entry::Separator(Some(label.into())));
        }
        for theme in group {
            entries.push(Entry::Item(Item {
                handle: picks.len() as i64,
                label: theme.label.clone(),
                description: theme_description(theme),
                pickable: true,
                // `manageExtensionIcon` is registered with `Codicon.gear` as its glyph.
                buttons: vec![Button { handle: 0, codicon: Some("gear".into()), tooltip: Some("Manage Extension".into()), ..Button::default() }],
                ..Item::default()
            }));
            picks.push(theme.settings_id.clone());
        }
    }
    (entries, picks)
}

fn is_pinned(theme: &ColorThemeEntry) -> bool {
    theme.settings_id == DEFAULT_DARK || theme.settings_id == DEFAULT_LIGHT
}

/// `toEntry`: the default themes' names, else the theme's description, else its settings id when the label differs.
fn theme_description(theme: &ColorThemeEntry) -> Option<String> {
    match theme.settings_id.as_str() {
        DEFAULT_LIGHT => Some("Default Light".into()),
        DEFAULT_DARK => Some("Default Dark".into()),
        _ => theme.description.clone().or_else(|| (theme.label != theme.settings_id).then(|| theme.settings_id.clone())),
    }
}

#[cfg(test)]
mod tests;
