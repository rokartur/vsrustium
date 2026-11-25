//! editorAutoSave.ts with the auto save part of filesConfigurationService.ts: `files.autoSave` saves a dirty
//! model after `files.autoSaveDelay`, when the editor loses the focus or when the window does.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use config::configuration::Overrides;
use gpui::Context;
use serde_json::{Value, json};

use super::Workbench;
use super::search_editor_view::SEARCH_EDITOR_SCHEME;
use crate::editor::OpenEditor;
use crate::markers::{ERROR, ReadOptions};

#[cfg(test)]
mod tests;

/// `SaveReason`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveReason {
    Explicit,
    Auto,
    FocusChange,
    WindowChange,
}

/// `IAutoSaveMode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoSaveMode {
    Off(Disabled),
    AfterShortDelay,
    AfterLongDelay,
    OnFocusChange,
    OnWindowChange,
}

/// `AutoSaveDisabledReason`; `DISABLED` comes from `disableAutoSave`, which only chat and inline chat call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disabled {
    Settings,
    OutOfWorkspace,
    Errors,
}

/// `FilesConfigurationService.DEFAULT_AUTO_SAVE_DELAY`.
const DEFAULT_AUTO_SAVE_DELAY: u64 = 1000;

/// `ICachedAutoSaveConfiguration` of the `files` settings node of a resource.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AutoSaveConfiguration {
    pub auto_save: Option<&'static str>,
    pub delay: Option<u64>,
    pub when_no_errors: bool,
    pub out_of_workspace: bool,
    pub short_delay: bool,
}

/// `computeAutoSaveConfiguration`; `inside_workspace` is asked only with `autoSaveWorkspaceFilesOnly`.
pub fn auto_save_configuration(files: &Value, inside_workspace: impl FnOnce() -> bool) -> AutoSaveConfiguration {
    let mut configuration = AutoSaveConfiguration::default();
    // DEFAULT_AUTO_SAVE_MODE is `off` outside the web.
    match files["autoSave"].as_str() {
        Some("afterDelay") => {
            let delay = files["autoSaveDelay"].as_f64().filter(|delay| *delay >= 0.).map_or(DEFAULT_AUTO_SAVE_DELAY, |delay| delay as u64);
            configuration.auto_save = Some("afterDelay");
            configuration.delay = Some(delay);
            configuration.short_delay = delay <= DEFAULT_AUTO_SAVE_DELAY;
        }
        Some("onFocusChange") => configuration.auto_save = Some("onFocusChange"),
        Some("onWindowChange") => configuration.auto_save = Some("onWindowChange"),
        _ => {}
    }
    if files["autoSaveWorkspaceFilesOnly"] == true && !inside_workspace() {
        configuration.out_of_workspace = true;
        configuration.short_delay = false;
    }
    if files["autoSaveWhenNoErrors"] == true {
        configuration.when_no_errors = true;
        configuration.short_delay = false;
    }
    configuration
}

/// `getAutoSaveMode`; `has_errors` is asked only for a resource with `autoSaveWhenNoErrors`.
pub fn auto_save_mode(configuration: &AutoSaveConfiguration, reason: Option<SaveReason>, has_errors: impl FnOnce() -> bool) -> AutoSaveMode {
    let Some(auto_save) = configuration.auto_save else { return AutoSaveMode::Off(Disabled::Settings) };
    let wrong_reason = match (auto_save, reason) {
        (_, None) => false,
        ("afterDelay", Some(reason)) => reason != SaveReason::Auto,
        ("onFocusChange", Some(reason)) => !matches!(reason, SaveReason::FocusChange | SaveReason::WindowChange),
        (_, Some(reason)) => reason != SaveReason::WindowChange,
    };
    if wrong_reason {
        return AutoSaveMode::Off(Disabled::Settings);
    }
    if configuration.out_of_workspace {
        return AutoSaveMode::Off(Disabled::OutOfWorkspace);
    }
    if configuration.when_no_errors && has_errors() {
        return AutoSaveMode::Off(Disabled::Errors);
    }
    match auto_save {
        // A save waiting for no errors counts as long: errors can come in after it was scheduled.
        "afterDelay" if configuration.delay.is_some_and(|delay| delay <= DEFAULT_AUTO_SAVE_DELAY) && !configuration.when_no_errors => AutoSaveMode::AfterShortDelay,
        "afterDelay" => AutoSaveMode::AfterLongDelay,
        "onFocusChange" => AutoSaveMode::OnFocusChange,
        _ => AutoSaveMode::OnWindowChange,
    }
}

/// A model by scheme and path.
type Key = (String, PathBuf);

#[derive(Default)]
pub(super) struct AutoSave {
    /// Each dirty model's version when its delayed save was scheduled, and that timer's generation: a later
    /// content change schedules again and the older timer does nothing (`scheduleAutoSave` clears it).
    scheduled: HashMap<Key, (u32, u64)>,
    generation: u64,
    /// The global mode last seen: a change saves every dirty model (`onDidChangeAutoSaveConfiguration`).
    mode: Option<AutoSaveMode>,
    /// `lastActiveEditor`: the model of the active editor last seen.
    active: Option<Key>,
    /// `waitingOnConditionAutoSaveWorkingCopies`: saves held back by error markers, run once they are gone.
    waiting: HashMap<Key, SaveReason>,
}

impl Workbench {
    /// The `files.autoSave` configuration of a resource, or the global one without (`getAutoSaveConfiguration`).
    fn auto_save_configuration(&self, model: Option<&OpenEditor>) -> AutoSaveConfiguration {
        let overrides = match model {
            Some(model) => Overrides { resource: Some(&model.document().path), override_identifier: Some(&model.document().language_id) },
            None => Overrides::default(),
        };
        let files = self.settings.lock().unwrap().get_value(Some("files"), &overrides).unwrap_or(Value::Null);
        let inside_workspace = || model.is_none() || self.workspace().is_some_and(|workspace| workspace.folders.iter().any(|folder| model.unwrap().document().path.starts_with(&folder.path)));
        auto_save_configuration(&files, inside_workspace)
    }

    fn auto_save_mode(&self, model: Option<&OpenEditor>, reason: Option<SaveReason>) -> AutoSaveMode {
        let configuration = self.auto_save_configuration(model);
        let has_errors = || {
            let Some(model) = model else { return false };
            let resource = model.document().uri_string();
            !self.diagnostics.service.read(ReadOptions { resource: Some(&resource), severities: Some(ERROR), take: Some(1), ..Default::default() }).is_empty()
        };
        auto_save_mode(&configuration, reason, has_errors)
    }

    /// A working copy the backup tracker auto saves on shutdown: not untitled, and its auto save mode is on.
    pub(super) fn auto_saved_on_shutdown(&self, model: &OpenEditor) -> bool {
        !self.untitled_working_copy(model) && !matches!(self.auto_save_mode(Some(model), None), AutoSaveMode::Off(_))
    }

    /// fileEditorInput.ts `isSaving`: a dirty file with a short auto save delay shows as clean, the save is due.
    pub(super) fn editor_is_saving(&self, model: &OpenEditor) -> bool {
        !self.untitled_working_copy(model) && model.is_dirty() && self.auto_save_configuration(Some(model)).short_delay
    }

    /// `WorkingCopyCapabilities.Untitled`: never auto saved. A search editor is untitled until it has a file.
    fn untitled_working_copy(&self, model: &OpenEditor) -> bool {
        match model.document().scheme.as_str() {
            "untitled" => true,
            SEARCH_EDITOR_SCHEME => self.search_editors.get(&model.document().path).is_none_or(|editor| editor.backing_file.is_none()),
            _ => false,
        }
    }

    /// Runs from every frame, as every model change and editor switch renders: content changes of dirty models
    /// schedule their delayed save, an auto save setting change saves every dirty model, and the editor that was
    /// active gets its focus change save.
    pub(super) fn auto_save_models_changed(&mut self, cx: &mut Context<Self>) {
        let mode = self.auto_save_mode(None, None);
        if self.auto_save.mode.replace(mode).is_some_and(|previous| previous != mode) {
            let reason = match mode {
                AutoSaveMode::OnFocusChange => Some(SaveReason::FocusChange),
                AutoSaveMode::OnWindowChange => Some(SaveReason::WindowChange),
                AutoSaveMode::AfterShortDelay | AutoSaveMode::AfterLongDelay => Some(SaveReason::Auto),
                AutoSaveMode::Off(_) => None,
            };
            if let Some(reason) = reason {
                self.save_all_dirty_auto_saveables(reason, cx);
            }
        }
        let active = self.editors.active().map(|editor| (editor.document().scheme.clone(), editor.document().path.clone()));
        if active != self.auto_save.active {
            if let Some(previous) = std::mem::replace(&mut self.auto_save.active, active) {
                self.maybe_trigger_auto_save(SaveReason::FocusChange, Some(previous), cx);
            }
        }
        let dirty: Vec<(Key, u32)> = self
            .editors
            .editors
            .iter()
            .chain(&self.editors.models)
            .filter(|model| model.is_dirty() && !self.untitled_working_copy(model))
            .map(|model| ((model.document().scheme.clone(), model.document().path.clone()), model.text_model().version_id))
            .collect();
        self.auto_save.scheduled.retain(|key, _| dirty.iter().any(|(dirty, _)| dirty == key));
        self.auto_save.waiting.retain(|key, _| dirty.iter().any(|(dirty, _)| dirty == key));
        for (key, version) in dirty {
            if self.auto_save.scheduled.get(&key).is_none_or(|(scheduled, _)| *scheduled != version) {
                self.schedule_auto_save(key, version, cx);
            }
        }
    }

    /// `scheduleAutoSave`: a save after `files.autoSaveDelay`, if one is configured for the model.
    fn schedule_auto_save(&mut self, key: Key, version: u32, cx: &mut Context<Self>) {
        let model = self.editors.model(&key.0, &key.1).expect("a dirty model");
        let Some(delay) = self.auto_save_configuration(Some(model)).delay else {
            self.auto_save.scheduled.remove(&key);
            return;
        };
        self.auto_save.generation += 1;
        let generation = self.auto_save.generation;
        self.auto_save.scheduled.insert(key.clone(), (version, generation));
        cx.spawn(async move |workbench, cx| {
            cx.background_executor().timer(Duration::from_millis(delay)).await;
            let _ = workbench.update(cx, |workbench, cx| {
                if workbench.auto_save.scheduled.get(&key).is_some_and(|(_, scheduled)| *scheduled == generation) {
                    workbench.auto_save_model(key, SaveReason::Auto, cx);
                }
            });
        })
        .detach();
    }

    /// Saves the model if it is dirty and its mode allows `reason`; a save held back by errors waits for them to go.
    fn auto_save_model(&mut self, key: Key, reason: SaveReason, cx: &mut Context<Self>) {
        let Some(model) = self.editors.model(&key.0, &key.1).filter(|model| model.is_dirty()) else { return };
        // textFileEditorModel.ts `doSave` ignores a save request of a read-only model.
        if self.readonly(&key.0, &key.1).is_some() {
            return;
        }
        match self.auto_save_mode(Some(model), Some(reason)) {
            AutoSaveMode::Off(Disabled::Errors) => _ = self.auto_save.waiting.insert(key, reason),
            AutoSaveMode::Off(_) => {}
            _ => drop(self.save_model(key.0, key.1, reason, cx)),
        }
    }

    /// `maybeTriggerAutoSave`: of the model of the editor that lost the focus, or of every dirty model.
    fn maybe_trigger_auto_save(&mut self, reason: SaveReason, editor: Option<Key>, cx: &mut Context<Self>) {
        let Some(key) = editor else { return self.save_all_dirty_auto_saveables(reason, cx) };
        let Some(model) = self.editors.model(&key.0, &key.1) else { return };
        if !model.is_dirty() || self.readonly(&key.0, &key.1).is_some() || self.untitled_working_copy(model) {
            return;
        }
        let saves = match self.auto_save_mode(Some(model), Some(reason)) {
            AutoSaveMode::Off(Disabled::Errors) => {
                self.auto_save.waiting.insert(key, reason);
                return;
            }
            AutoSaveMode::Off(_) => false,
            AutoSaveMode::OnFocusChange => true,
            AutoSaveMode::OnWindowChange => reason == SaveReason::WindowChange,
            AutoSaveMode::AfterShortDelay | AutoSaveMode::AfterLongDelay => false,
        };
        if saves {
            drop(self.save_model(key.0, key.1, reason, cx));
        }
    }

    /// `saveAllDirtyAutoSaveables`.
    fn save_all_dirty_auto_saveables(&mut self, reason: SaveReason, cx: &mut Context<Self>) {
        let dirty: Vec<Key> = self
            .editors
            .editors
            .iter()
            .chain(&self.editors.models)
            .filter(|model| model.is_dirty() && !self.untitled_working_copy(model))
            .map(|model| (model.document().scheme.clone(), model.document().path.clone()))
            .collect();
        for key in dirty {
            self.auto_save_model(key, reason, cx);
        }
    }

    /// `onActiveWindowChange` and `onWindowFocusChange(false)`.
    pub(super) fn window_lost_focus_auto_save(&mut self, cx: &mut Context<Self>) {
        self.maybe_trigger_auto_save(SaveReason::WindowChange, None, cx);
    }

    /// `activeEditorPane.onDidBlur`: the focus left the editor.
    pub(super) fn editor_blurred_auto_save(&mut self, cx: &mut Context<Self>) {
        let active = self.editors.active().map(|editor| (editor.document().scheme.clone(), editor.document().path.clone()));
        if active.is_some() {
            self.maybe_trigger_auto_save(SaveReason::FocusChange, active, cx);
        }
    }

    /// `onConditionChanged(resources, ERRORS)`: markers changed, the saves held back for these resources run if
    /// their mode allows them now.
    pub(super) fn auto_save_markers_changed(&mut self, resources: &[String], cx: &mut Context<Self>) {
        let waiting: Vec<(Key, SaveReason)> = self.auto_save.waiting.iter().map(|(key, reason)| (key.clone(), *reason)).collect();
        for (key, reason) in waiting {
            let Some(model) = self.editors.model(&key.0, &key.1) else { continue };
            if !resources.contains(&model.document().uri_string()) {
                continue;
            }
            self.auto_save.waiting.remove(&key);
            if model.is_dirty() && !matches!(self.auto_save_mode(Some(model), Some(reason)), AutoSaveMode::Off(_)) {
                drop(self.save_model(key.0, key.1, reason, cx));
            }
        }
    }

    /// `toggleAutoSave`: `afterDelay` when off, else off.
    pub(super) fn toggle_auto_save(&mut self) {
        let on = matches!(self.setting("files.autoSave").as_str(), Some("afterDelay" | "onFocusChange" | "onWindowChange"));
        self.update_setting("files.autoSave", json!(if on { "off" } else { "afterDelay" }));
    }
}


