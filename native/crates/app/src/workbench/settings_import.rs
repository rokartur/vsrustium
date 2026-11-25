//! `workbench.action.importSettings` and its first-run offer (port feature, Artur): pick an installed VS Code,
//! Cursor or VSCodium, see what will be imported, and write it into the current profile; the extensions are
//! installed again from Open VSX.

use gpui::{Context, Window};
use serde_json::Value;

use super::Workbench;
use crate::commands::Outcome;
use crate::notifications::Severity;
use crate::quick_input::{Entry, Item, Kind, QuickInput};
use crate::settings_import::{self, Import, Source, Target};

/// Profile storage: the first-run offer was shown.
const OFFERED_KEY: &str = "settingsImport.offered";

impl Workbench {
    pub(super) fn register_settings_import_commands(&mut self) {
        self.commands.register("workbench.action.importSettings", |workbench, _, window, cx| {
            workbench.pick_settings_import_source(window, cx);
            Outcome::done()
        });
    }

    /// The first start with the default user data and no settings of its own offers the import once.
    pub(crate) fn offer_settings_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let default_user_data = !std::env::args().any(|arg| arg == "--user-data-dir");
        let settings_file = self.settings.lock().unwrap().user_files().settings.clone();
        let offered = self.profile_storage.lock().unwrap().get(OFFERED_KEY).is_some();
        if !default_user_data || offered || settings_file.exists() || self.skip_dialogs || import_sources().is_empty() {
            return;
        }
        self.profile_storage.lock().unwrap().set(OFFERED_KEY, "true".into());
        let reply = self.prompt_notification(Severity::Info, "Import settings, keyboard shortcuts, snippets and extensions from VS Code, Cursor or VSCodium?", &["Import", "Not Now"]);
        cx.spawn_in(window, async move |workbench, cx| {
            let Ok(Ok(Value::Number(choice))) = reply.await else { return };
            if choice.as_u64() == Some(0) {
                let _ = workbench.update_in(cx, |workbench, window, cx| workbench.pick_settings_import_source(window, cx));
            }
        })
        .detach();
    }

    fn pick_settings_import_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let sources = import_sources();
        if sources.is_empty() {
            self.notify_error("No VS Code, Cursor or VSCodium user data was found.");
            return;
        }
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some("Select the editor to import from".into());
        let items = sources.iter().enumerate().map(|(handle, source)| {
            Entry::Item(Item { handle: handle as i64, label: source.name.into(), description: Some(source.user_dir.display().to_string()), pickable: true, ..Item::default() })
        });
        input.set_items(items.collect());
        let picked = self.choose(input, window, cx);
        cx.spawn_in(window, async move |workbench, cx| {
            let Ok(Ok(Value::Number(handle))) = picked.await else { return };
            let source = sources[handle.as_u64().expect("a handle") as usize].clone();
            let _ = workbench.update_in(cx, |workbench, window, cx| workbench.pick_settings_import_contents(source, window, cx));
        })
        .detach();
    }

    /// What will be imported, all checked; the unchecked parts are left out.
    fn pick_settings_import_contents(&mut self, source: Source, window: &mut Window, cx: &mut Context<Self>) {
        let import = settings_import::read(&source);
        let parts = [
            ("Settings", import.settings.len()),
            ("Keyboard Shortcuts", import.keybindings.len()),
            ("Snippets", import.snippets.len()),
            ("Extensions", import.extensions.len()),
        ];
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.title = Some(format!("Import from {}", source.name));
        input.placeholder = Some("Select what to import into the current profile".into());
        input.can_select_many = true;
        let items: Vec<Entry> = parts
            .iter()
            .enumerate()
            .filter(|(_, (_, count))| *count > 0)
            .map(|(handle, (label, count))| Entry::Item(Item { handle: handle as i64, label: (*label).into(), description: Some(count.to_string()), picked: true, pickable: true, ..Item::default() }))
            .collect();
        if items.is_empty() {
            self.notify_error(format!("{} has nothing to import.", source.name));
            return;
        }
        input.set_items(items);
        let picked = self.choose(input, window, cx);
        cx.spawn_in(window, async move |workbench, cx| {
            let Ok(Ok(Value::Array(handles))) = picked.await else { return };
            let chosen: Vec<u64> = handles.iter().filter_map(Value::as_u64).collect();
            let import = Import {
                settings: if chosen.contains(&0) { import.settings } else { Default::default() },
                keybindings: if chosen.contains(&1) { import.keybindings } else { Vec::new() },
                snippets: if chosen.contains(&2) { import.snippets } else { Vec::new() },
                extensions: if chosen.contains(&3) { import.extensions } else { Vec::new() },
            };
            let _ = workbench.update(cx, |workbench, cx| workbench.write_settings_import(source.name, import, cx));
        })
        .detach();
    }

    fn write_settings_import(&mut self, name: &str, import: Import, cx: &mut Context<Self>) {
        let Some(profile) = self.user_data_profiles.state.as_ref().map(|state| state.current.clone()) else { return };
        let settings_file = self.settings.lock().unwrap().user_files().settings.clone();
        let target = Target { settings: &settings_file, keybindings: &profile.keybindings_resource, snippets_home: &profile.snippets_home };
        let written = settings_import::write(&import, &target, &self.settings.lock().unwrap().configuration);
        let written = match written {
            Ok(written) => written,
            Err(error) => return self.notify_error(error),
        };
        self.reload_changed_settings_files(cx);
        let installed: Vec<String> = self.extensions.iter().map(|extension| extension.id.to_lowercase()).collect();
        let missing: Vec<String> = import.extensions.into_iter().filter(|id| !installed.contains(id)).collect();
        for id in &missing {
            drop(self.install_from_gallery(id.clone(), None, false, cx));
        }
        let message = format!(
            "Imported from {name}: {} settings, {} keyboard shortcuts, {} snippet files; installing {} extensions from Open VSX.",
            written.settings,
            written.keybindings,
            written.snippets,
            missing.len()
        );
        drop(self.prompt_notification(Severity::Info, message, &[]));
        cx.notify();
    }
}

fn import_sources() -> Vec<Source> {
    let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    settings_import::sources(&home)
}
