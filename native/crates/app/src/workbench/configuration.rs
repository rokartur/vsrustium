//! The workbench side of configurationService.ts's file watchers: settings files edited outside the port reach
//! the extension host (mainThreadConfiguration.ts), the color theme (ThemeService) and the text models
//! (modelService.ts `_updateModelOptions`).

use std::time::Duration;

use config::indentation::{CreationOptions, creation_options};
use config::model::Change;
use gpui::Context;

use crate::editor::{Indentation, guess_indentation};
use crate::workbench::Workbench;
use crate::workbench::themes::SettingTarget;

/// Upstream batches settings file watcher events by 100 ms.
const SETTINGS_CHECK_INTERVAL: Duration = Duration::from_millis(100);

impl Workbench {
    /// Checks the settings files every 100 ms for edits made outside the port.
    // shortcut: polled by size and modification time; the files crate's FSEvents watcher covers workspace folders only.
    pub fn watch_settings_files(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |workbench, cx| {
            loop {
                cx.background_executor().timer(SETTINGS_CHECK_INTERVAL).await;
                if workbench.update(cx, |workbench, cx| workbench.reload_changed_settings_files(cx)).is_err() {
                    return;
                }
            }
        })
        .detach();
    }

    /// Reloads the settings files that changed on disk and applies the change.
    pub fn reload_changed_settings_files(&mut self, cx: &mut Context<Self>) {
        let before = self.model_creation_options();
        let Some(change) = self.settings.lock().unwrap().reload_changed_files() else { return };
        self.configuration_changed(&change, before, cx);
    }

    /// `onDidChangeConfiguration` listeners of the workbench for a change already in the settings model.
    fn configuration_changed(&mut self, change: &Change, before: Vec<CreationOptions>, cx: &mut Context<Self>) {
        self.host.send_configuration_change(&self.settings.lock().unwrap(), change);
        if affects(change, "workbench.colorTheme") {
            let configured = self.settings.lock().unwrap().get_value(Some("workbench.colorTheme"), &Default::default());
            if let Some(id) = configured.as_ref().and_then(|id| id.as_str()) {
                // ThemeService `restoreColorTheme`: applied without writing the setting back.
                self.set_color_theme(id, SettingTarget::Preview, cx);
            }
        }
        self.update_model_options(before);
        cx.notify();
    }

    fn model_creation_options(&self) -> Vec<CreationOptions> {
        let settings = self.settings.lock().unwrap();
        let models = self.editors.editors.iter().chain(&self.editors.models);
        models.map(|model| creation_options(&settings.configuration, &model.document.path, &model.document.language_id)).collect()
    }

    /// modelService.ts `_updateModelOptions` for each model whose creation options changed.
    fn update_model_options(&mut self, before: Vec<CreationOptions>) {
        let after = self.model_creation_options();
        let active_before = self.editors.active().map(|editor| editor.document.indentation);
        let models = self.editors.editors.iter_mut().chain(&mut self.editors.models);
        for ((model, old), new) in models.zip(before).zip(after) {
            if old == new {
                continue;
            }
            let indentation = match new.detect_indentation {
                true => guess_indentation(&model.document.lines, Indentation { tab_size: new.tab_size, indent_size: None, insert_spaces: new.insert_spaces }),
                false => Indentation { tab_size: new.tab_size, indent_size: new.indent_size, insert_spaces: new.insert_spaces },
            };
            model.set_indentation(indentation);
        }
        if let Some(editor) = self.editors.active()
            && active_before != Some(editor.document.indentation)
        {
            self.host.send_options(editor);
        }
    }
}

/// `IConfigurationChangeEvent.affectsConfiguration(key)` for a key without overrides: the key, a parent or a child changed.
fn affects(change: &Change, key: &str) -> bool {
    let touches = |changed: &str| changed == key || key.starts_with(&format!("{changed}.")) || changed.starts_with(&format!("{key}."));
    change.keys.iter().any(|changed| touches(changed))
}

#[cfg(test)]
mod tests;
