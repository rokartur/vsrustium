//! contrib/userDataProfile/browser/userDataProfile.ts and userDataProfileActions.ts with the window side of
//! userDataProfileManagement.ts: the Profiles commands and menus, the `currentProfile` and `hasProfiles` context
//! keys, and `changeCurrentProfile`, which restarts the extension host on the new profile's files (the port has no
//! window reload).

use std::sync::{Arc, Mutex};

use gpui::{Context, Window};
use serde_json::{Value, json};
use storage::Storage;
use storage::profiles::Profile;

use super::Workbench;
use crate::command_palette::{MenuItem as PaletteItem, locale_compare};
use crate::commands::{Outcome, Registration};
use crate::contextkey::Expr;
use crate::menus::{Entry, MenuId, MenuItem, Toggled};
use crate::quick_input::{Entry as PickEntry, Item, Kind, QuickInput};
use crate::user_data_profile::{ProfileState, user_files};

/// userDataProfile.ts `registerHelpAction`.
const PROFILES_HELP_URL: &str = "https://aka.ms/vscode-profiles-help";

#[derive(Default)]
pub struct UserDataProfiles {
    pub state: Option<ProfileState>,
    /// `registerProfilesActions`: the `profileEntry` and `openProfile` command of each profile, by command id.
    registrations: Vec<(String, Registration)>,
    pub editor: super::profiles_editor_view::ProfilesEditor,
    /// The gallery extensions of an applied template, installed once the window runs the new profile.
    pub pending_installs: Vec<String>,
}

impl Workbench {
    pub(super) fn register_user_data_profile_commands(&mut self) {
        self.commands.register("workbench.profiles.actions.switchProfile", |workbench, _, window, cx| workbench.show_switch_profile(window, cx));
        self.commands.register("workbench.profiles.actions.deleteProfile", |workbench, _, window, cx| workbench.show_delete_profiles(window, cx));
        self.commands.register("workbench.profiles.actions.newWindowWithProfile", |workbench, _, window, cx| workbench.show_new_window_with_profile(window, cx));
        self.commands.register("workbench.profiles.actions.createTemporaryProfile", |workbench, _, _, _| workbench.open_window_with_profile_flags(vec!["--profile-temp".into()]));
        self.commands.register("workbench.profiles.actions.cleanupProfiles", |workbench, _, _, cx| {
            let Some(state) = workbench.user_data_profiles.state.as_mut() else { return Outcome::done() };
            state.edit(|profiles, storage| profiles.clean_up(storage));
            workbench.profiles_changed(cx);
            Outcome::done()
        });
        self.commands.register("workbench.profiles.actions.resetWorkspaces", |workbench, _, _, cx| {
            let Some(state) = workbench.user_data_profiles.state.as_mut() else { return Outcome::done() };
            state.edit(|profiles, storage| profiles.reset_workspaces(storage));
            workbench.profiles_changed(cx);
            Outcome::done()
        });
        self.commands.register("workbench.profiles.actions.help", |workbench, _, window, cx| {
            let target = super::opener::Target::Text(PROFILES_HELP_URL.into());
            drop(workbench.open_link(target, Default::default(), window, cx));
            Outcome::done()
        });
    }

    /// Takes the profiles main.rs resolved and publishes them to the context keys, menus and commands.
    pub fn set_user_data_profiles(&mut self, state: ProfileState, cx: &mut Context<Self>) {
        self.user_data_profiles.state = Some(state);
        self.user_data_profiles.editor.name_focus = Some(cx.focus_handle());
        self.profiles_changed(cx);
    }

    /// `onDidChangeProfiles`: the context keys, the Profiles menus and the commands of each profile.
    pub(super) fn profiles_changed(&mut self, cx: &mut Context<Self>) {
        let Some(state) = &self.user_data_profiles.state else { return };
        let profiles: Vec<Profile> = state.profiles.profiles();
        let current = state.current.clone();
        self.context_keys.insert("currentProfile".into(), json!(current.id));
        self.context_keys.insert("hasProfiles".into(), json!(profiles.len() > 1));
        for (id, registration) in std::mem::take(&mut self.user_data_profiles.registrations) {
            self.commands.dispose(&id, registration);
        }
        let mut registrations = Vec::new();
        for profile in &profiles {
            let entry_id = format!("workbench.profiles.actions.profileEntry.{}", profile.id);
            let switch_to = profile.clone();
            let registration = self.commands.register(entry_id.clone(), move |workbench, _, window, cx| {
                workbench.switch_profile(&switch_to, window, cx);
                Outcome::done()
            });
            registrations.push((entry_id, registration));
            let open_id = open_profile_command(&profile.name);
            let name = profile.name.clone();
            let registration = self.commands.register(open_id.clone(), move |workbench, _, _, _| workbench.open_window_with_profile_flags(vec!["--profile".into(), name.clone()]));
            registrations.push((open_id, registration));
        }
        self.user_data_profiles.registrations = registrations;
        self.menus.retain(|item| !matches!(item.menu, MenuId::ProfilesMenu | MenuId::OpenProfileMenu) && !is_profiles_submenu(item));
        self.menus.extend(profile_menus(&profiles, &current));
        let palette = std::mem::take(&mut self.quick_access.menu)
            .into_iter()
            .filter(|item| !item.id.starts_with("workbench.action.openProfile.") && item.id != IMPORT_SETTINGS_COMMAND)
            .chain(open_profile_palette_items(&profiles))
            .chain([import_settings_palette_item()])
            .collect();
        self.set_command_palette_menu(palette);
        cx.notify();
    }

    /// `SwitchProfileAction`: the profiles by name, the current one checked.
    fn show_switch_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let Some(state) = &self.user_data_profiles.state else { return Outcome::done() };
        let mut profiles = state.profiles.profiles();
        profiles.sort_by(|a, b| locale_compare(&a.name, &b.name));
        let current = state.current.id.clone();
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some("Select Profile".into());
        let items = profiles.iter().enumerate().map(|(handle, profile)| {
            let label = if profile.id == current { format!("$(check) {}", profile.name) } else { profile.name.clone() };
            PickEntry::Item(Item { handle: handle as i64, label, pickable: true, ..Item::default() })
        });
        input.set_items(items.collect());
        let picked = self.choose(input, window, cx);
        cx.spawn_in(window, async move |workbench, cx| {
            let Ok(Ok(Value::Number(handle))) = picked.await else { return };
            let profile = profiles[handle.as_u64().expect("a handle") as usize].clone();
            let _ = workbench.update_in(cx, |workbench, window, cx| workbench.switch_profile(&profile, window, cx));
        })
        .detach();
        Outcome::done()
    }

    /// `DeleteProfileAction`: the profiles but the default, the current one described as such.
    fn show_delete_profiles(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let Some(state) = &self.user_data_profiles.state else { return Outcome::done() };
        let profiles: Vec<Profile> = state.profiles.profiles().into_iter().filter(|profile| !profile.is_default).collect();
        if profiles.is_empty() {
            return Outcome::done();
        }
        let current = state.current.id.clone();
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.title = Some("Delete Profile...".into());
        input.placeholder = Some("Select Profiles to Delete".into());
        input.can_select_many = true;
        let items = profiles.iter().enumerate().map(|(handle, profile)| {
            let description = (profile.id == current).then(|| "Current".to_owned());
            PickEntry::Item(Item { handle: handle as i64, label: profile.name.clone(), description, pickable: true, ..Item::default() })
        });
        input.set_items(items.collect());
        let picked = self.choose(input, window, cx);
        cx.spawn_in(window, async move |workbench, cx| {
            let Ok(Ok(Value::Array(handles))) = picked.await else { return };
            let picked: Vec<Profile> = handles.iter().filter_map(Value::as_u64).map(|handle| profiles[handle as usize].clone()).collect();
            let _ = workbench.update_in(cx, |workbench, window, cx| {
                for profile in picked {
                    workbench.remove_profile(&profile, window, cx);
                }
            });
        })
        .detach();
        Outcome::done()
    }

    /// `NewWindowWithProfileAction`: the profiles, each opening a window with it.
    fn show_new_window_with_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let Some(state) = &self.user_data_profiles.state else { return Outcome::done() };
        let profiles = state.profiles.profiles();
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.title = Some("New Window with Profile".into());
        input.placeholder = Some("Select Profile".into());
        let items = profiles.iter().enumerate().map(|(handle, profile)| PickEntry::Item(Item { handle: handle as i64, label: profile.name.clone(), pickable: true, ..Item::default() }));
        input.set_items(items.collect());
        let picked = self.choose(input, window, cx);
        cx.spawn_in(window, async move |workbench, cx| {
            let Ok(Ok(Value::Number(handle))) = picked.await else { return };
            let name = profiles[handle.as_u64().expect("a handle") as usize].name.clone();
            let _ = workbench.update(cx, |workbench, _| workbench.open_window_with_profile_flags(vec!["--profile".into(), name]));
        })
        .detach();
        Outcome::done()
    }

    /// `hostService.openWindow({ forceProfile })`: another app process on no folder with the profile flags.
    pub(super) fn open_window_with_profile_flags(&mut self, flags: Vec<String>) -> Outcome {
        let mut args: Vec<String> = std::env::args().collect();
        for index in crate::positional_indexes(&args).into_iter().rev() {
            args.remove(index);
        }
        if let Some(index) = args.iter().position(|arg| arg == "--profile") {
            args.drain(index..(index + 2).min(args.len()));
        }
        args.retain(|arg| arg != "--profile-temp");
        args.extend(flags);
        let spawned = std::env::current_exe().and_then(|exe| std::process::Command::new(exe).args(&args[1..]).spawn());
        if let Err(error) = spawned {
            self.notify_error(format!("Failed to open a new window: {error}"));
        }
        Outcome::done()
    }

    /// userDataProfileManagement.ts `switchProfile`: the workspace is associated with the profile, then the window
    /// changes to it.
    pub(crate) fn switch_profile(&mut self, profile: &Profile, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.user_data_profiles.state.as_mut() else { return };
        if state.current.id == profile.id {
            return;
        }
        if let Some(workspace) = state.workspace.clone() {
            if let Err(error) = state.edit(|profiles, storage| profiles.set_profile_for_workspace(&workspace, &profile.id, storage)) {
                self.notify_error(error);
                return;
            }
        }
        self.change_current_profile(profile.clone(), window, cx);
        self.profiles_changed(cx);
    }

    /// `removeProfile`; removing the current profile changes the window to the workspace's or the default one.
    pub(crate) fn remove_profile(&mut self, profile: &Profile, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.user_data_profiles.state.as_mut() else { return };
        if let Err(error) = state.edit(|profiles, storage| profiles.remove_profile(&profile.id, storage)) {
            self.notify_error(error);
            return;
        }
        if state.current.id == profile.id {
            let workspace = state.workspace.clone();
            let next = workspace.and_then(|workspace| state.profiles.profile_for_workspace(&workspace).cloned()).unwrap_or_else(|| state.profiles.default_profile().clone());
            self.change_current_profile(next, window, cx);
        }
        self.profiles_changed(cx);
    }

    /// `changeCurrentProfile`: the configuration, keybindings, storage and user extensions move to the profile's
    /// files, and the extension host restarts when the profile or what it shares with the default one changed.
    pub(crate) fn change_current_profile(&mut self, profile: Profile, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.user_data_profiles.state.as_mut() else { return };
        let previous = std::mem::replace(&mut state.current, profile.clone());
        let restart = previous.id != profile.id || previous.use_default_flags != profile.use_default_flags;
        if previous.is_transient {
            state.edit(|profiles, storage| profiles.clean_up_transient_profiles(storage));
        }
        let files = user_files(&profile, state.profiles.default_profile());
        let before = self.model_creation_options();
        let change = self.settings.lock().unwrap().set_profile(files);
        self.keybindings.user_file = Some(profile.keybindings_resource.clone());
        self.set_user_keybindings(crate::keybindings::user_keybindings(&profile.keybindings_resource));
        if let Err(error) = std::fs::create_dir_all(&profile.global_storage_home) {
            self.notify_error(format!("{}: {error}", profile.global_storage_home.display()));
        }
        let storage = Storage::open(&profile.global_storage_home.join("state.vscdb"));
        self.restore_profile_state(Arc::new(Mutex::new(storage)));
        if let Some(user) = self.extensions_viewlet.user.as_mut() {
            user.extensions_file = profile.extensions_resource.clone();
        }
        self.rescan_installed_extensions();
        self.configuration_changed(&change, before, cx);
        if restart {
            self.restart_extension_host(window, cx);
        }
        // `ExtensionsResource.apply`: the template's extensions the profile does not have yet.
        let installed: Vec<String> = self.extensions.iter().map(|extension| extension.id.to_lowercase()).collect();
        for id in std::mem::take(&mut self.user_data_profiles.pending_installs) {
            if !installed.contains(&id) {
                drop(self.install_from_gallery(id, None, false, cx));
            }
        }
    }
}

/// `registerNewWindowAction`'s id; upstream's `name.replace('/\s+/', '_')` replaces a literal string, so the name
/// stays as it is.
fn open_profile_command(name: &str) -> String {
    format!("workbench.action.openProfile.{name}")
}

fn is_profiles_submenu(item: &MenuItem) -> bool {
    matches!(item.entry, Entry::Submenu { submenu: MenuId::ProfilesMenu | MenuId::OpenProfileMenu, .. })
}

/// `registerProfileSubMenu`, `registerManageProfilesAction`, `registerProfileEntryAction`,
/// `registerOpenProfileSubMenu` and `registerNewWindowAction`.
fn profile_menus(profiles: &[Profile], current: &Profile) -> Vec<MenuItem> {
    let command = |id: &str| Entry::Command { id: id.into(), precondition: None, toggled: None, alt: None };
    let item = |menu, entry, title: &str, group: &str, order, when: Option<&str>| MenuItem { menu, entry, title: title.into(), group: group.into(), order, when: when.and_then(Expr::deserialize) };
    let submenu = |submenu| Entry::Submenu { submenu, toggled: None };
    let title = format!("Profile ({})", current.name);
    let mut items = vec![
        item(MenuId::GlobalActivity, submenu(MenuId::ProfilesMenu), &title, "2_configuration", 1., Some("hasProfiles")),
        item(MenuId::MenubarPreferencesMenu, submenu(MenuId::ProfilesMenu), &title, "2_configuration", 1., Some("hasProfiles && !isSessionsWindow")),
        item(MenuId::GlobalActivity, command("workbench.profiles.actions.manageProfiles"), "Profiles", "2_configuration", 1., Some("!hasProfiles")),
        item(MenuId::MenubarPreferencesMenu, command("workbench.profiles.actions.manageProfiles"), "&&Profiles", "2_configuration", 1., Some("!hasProfiles && !isSessionsWindow")),
        item(MenuId::ProfilesMenu, command("workbench.profiles.actions.manageProfiles"), "Profiles", "1_manage", 1., None),
        item(MenuId::MenubarFileMenu, submenu(MenuId::OpenProfileMenu), "New Window with Profile", "1_new", 4., None),
        item(MenuId::OpenProfileMenu, command("workbench.profiles.actions.createProfile"), "New Profile...", "1_manage_profiles", 1., None),
    ];
    for profile in profiles {
        let toggled = Toggled::when(Expr::deserialize(&format!("currentProfile == '{}'", profile.id)));
        let entry = Entry::Command { id: format!("workbench.profiles.actions.profileEntry.{}", profile.id), precondition: None, toggled, alt: None };
        items.push(item(MenuId::ProfilesMenu, entry, &profile.name, "0_profiles", 0., None));
        let entry = command(&open_profile_command(&profile.name));
        items.push(item(MenuId::OpenProfileMenu, entry, &profile.name, "0_profiles", 0., Some("hasProfiles")));
    }
    items
}

const IMPORT_SETTINGS_COMMAND: &str = "workbench.action.importSettings";

/// The settings import (port feature, Artur) in the Command Palette.
fn import_settings_palette_item() -> PaletteItem {
    PaletteItem { id: IMPORT_SETTINGS_COMMAND.into(), title: "Import from VS Code, Cursor or VSCodium...".into(), category: Some("Preferences".into()), when: None, precondition: None, source: None }
}

/// `registerNewWindowAction`'s Command Palette item: "Open {name} Profile".
fn open_profile_palette_items(profiles: &[Profile]) -> Vec<PaletteItem> {
    let item = |profile: &Profile| PaletteItem {
        id: open_profile_command(&profile.name),
        title: format!("Open {} Profile", profile.name),
        category: Some("Profiles".into()),
        when: None,
        precondition: Expr::deserialize("hasProfiles"),
        source: None,
    };
    profiles.iter().map(item).collect()
}
