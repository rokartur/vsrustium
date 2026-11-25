//! The workbench side of configurationService.ts's file watchers: settings files edited outside the port reach
//! the extension host (mainThreadConfiguration.ts), the color theme (ThemeService) and the text models
//! (modelService.ts `_updateModelOptions`).

use std::time::Duration;

use config::indentation::{CreationOptions, creation_options};
use config::model::Change;
use gpui::Context;

use crate::editor::{Indentation, guess_indentation};
use crate::workbench::Workbench;

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

    /// nativePolicyService.ts `_onDidPolicyChange`: a managed preferences change reads the policies again and applies
    /// the ones that changed.
    pub fn watch_policies(&self, cx: &mut Context<Self>) {
        let (sender, mut receiver) = futures::channel::mpsc::unbounded();
        crate::policy::watch_policy_changes(crate::policy::MANAGED_PREFERENCES, move || {
            let _ = sender.unbounded_send(());
        });
        cx.spawn(async move |workbench, cx| {
            while futures::StreamExt::next(&mut receiver).await.is_some() {
                if workbench.update(cx, |workbench, cx| workbench.reload_policies(cx)).is_err() {
                    return;
                }
            }
        })
        .detach();
    }

    pub(super) fn reload_policies(&mut self, cx: &mut Context<Self>) {
        let before = self.model_creation_options();
        let mut settings = self.settings.lock().unwrap();
        let values = crate::policy::read_native_policies(&settings.registry, crate::policy::BUNDLE_IDENTIFIER);
        let change = settings.set_policy_values(values);
        drop(settings);
        if !change.is_empty() {
            self.configuration_changed(&change, before, cx);
        }
    }

    /// Reloads the settings files that changed on disk and applies the change.
    pub fn reload_changed_settings_files(&mut self, cx: &mut Context<Self>) {
        let before = self.model_creation_options();
        let Some(change) = self.settings.lock().unwrap().reload_changed_files() else { return };
        self.configuration_changed(&change, before, cx);
    }

    /// `onDidChangeConfiguration` listeners of the workbench for a change already in the settings model.
    pub(super) fn configuration_changed(&mut self, change: &Change, before: Vec<CreationOptions>, cx: &mut Context<Self>) {
        self.host.send_configuration_change(&self.settings.lock().unwrap(), change);
        let theme_keys = ["workbench.colorTheme", "workbench.preferredDarkColorTheme", "workbench.preferredLightColorTheme", "workbench.preferredHighContrastColorTheme", "workbench.preferredHighContrastLightColorTheme", "window.autoDetectColorScheme", "window.autoDetectHighContrast"];
        if theme_keys.iter().any(|key| affects(change, key)) {
            self.restore_color_theme(cx);
        }
        // `restoreFileIconTheme`
        if affects(change, "workbench.iconTheme") {
            let file_icons = crate::settings_file_icons(&self.extensions, &self.settings.lock().unwrap());
            match file_icons {
                Ok(file_icons) => self.file_icons = file_icons,
                Err(error) => eprintln!("loading the file icon theme: {error:#}"),
            }
        }
        if ["workbench.colorCustomizations", "editor.tokenColorCustomizations", "editor.semanticTokenColorCustomizations"].iter().any(|key| affects(change, key)) {
            self.customize_theme();
            self.semantic_tokens_settings_changed(cx);
        }
        self.update_model_options(before);
        if ["editor.folding", "editor.foldingStrategy", "editor.foldingMaximumRegions", "editor.defaultFoldingRangeProvider"].iter().any(|key| affects(change, key)) {
            self.trigger_folding_model_changed(cx);
        }
        if affects(change, "editor.semanticHighlighting.enabled") {
            self.semantic_tokens_settings_changed(cx);
        }
        if affects(change, "editor.linkedEditing") || affects(change, "editor.renameOnType") {
            self.linked_editing_reinitialize(false, cx);
        }
        if affects(change, "problems.visibility") || affects(change, "problems.decorations.enabled") {
            self.update_markers_file_decorations();
        }
        if affects(change, "editor.defaultFormatter") {
            self.refresh_language_status(cx);
        }
        if affects(change, "files.associations") {
            self.files_associations_changed(cx);
        }
        self.settings_editor_configuration_changed(cx);
        cx.notify();
    }

    pub(super) fn model_creation_options(&self) -> Vec<CreationOptions> {
        let settings = self.settings.lock().unwrap();
        let models = self.editors.editors.iter().chain(&self.editors.models);
        models.map(|model| creation_options(&settings.configuration, &model.document().path, &model.document().language_id)).collect()
    }

    /// modelService.ts `_updateModelOptions` for each model whose creation options changed.
    fn update_model_options(&mut self, before: Vec<CreationOptions>) {
        let after = self.model_creation_options();
        let active_before = self.editors.active().map(|editor| editor.document().indentation);
        let models = self.editors.editors.iter_mut().chain(&mut self.editors.models);
        for ((model, old), new) in models.zip(before).zip(after) {
            if old == new {
                continue;
            }
            let indentation = match new.detect_indentation {
                true => guess_indentation(&model.document().lines, Indentation { tab_size: new.tab_size, indent_size: None, insert_spaces: new.insert_spaces }),
                false => Indentation { tab_size: new.tab_size, indent_size: new.indent_size, insert_spaces: new.insert_spaces },
            };
            model.set_indentation(indentation);
        }
        if let Some(editor) = self.editors.active()
            && active_before != Some(editor.document().indentation)
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
