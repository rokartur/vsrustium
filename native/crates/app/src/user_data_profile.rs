//! The window's profile: the profiles of `storage.json` (userDataProfile.ts `UserDataProfilesService`), the one
//! windowsMainService.ts `resolveProfileForBrowserWindow` picks for the workspace, and the user files the
//! configuration service reads from it.

use std::path::{Path, PathBuf};

use config::service::UserFiles;
use config::workspace::Workspace;
use storage::profiles::{Options, Paths, Profile, Profiles, WorkspaceId};
use storage::state::FileStorage;

pub struct ProfileState {
    /// `User/globalStorage/storage.json`, opened again for each change so the telemetry ids the extension host
    /// session writes into the same file are kept.
    state_file: PathBuf,
    pub profiles: Profiles,
    /// `userDataProfileService.currentProfile`.
    pub current: Profile,
    /// The workspace identifier the window's associations use; `None` for an empty window.
    pub workspace: Option<WorkspaceId>,
}

impl ProfileState {
    /// `--profile <name>` opens (or creates) that profile, `--profile-temp` a new transient one, else the profile
    /// associated with the workspace or the default. The choice is associated with the workspace unless the window
    /// develops an extension.
    pub fn resolve(user_data: &Path, extensions_dir: &Path, workspace: Option<&Workspace>, force_profile: Option<&str>, force_temp: bool, extension_development: bool) -> ProfileState {
        let user_home = user_data.join("User");
        let state_file = user_home.join("globalStorage/storage.json");
        if let Err(error) = std::fs::create_dir_all(user_home.join("globalStorage")) {
            eprintln!("[profiles] {}: {error}", user_home.display());
        }
        let mut state = FileStorage::open(&state_file);
        let paths = Paths { user_home, cache_home: user_data.to_path_buf(), extensions_dir: Some(extensions_dir.to_path_buf()) };
        let mut profiles = Profiles::load(&state, &paths);
        let workspace = workspace.map(workspace_id);
        let forced = match (force_profile, force_temp) {
            (Some(name), _) => match profiles.profiles().into_iter().find(|profile| profile.name == name) {
                Some(profile) => Some(Ok(profile)),
                None => Some(profiles.create_named_profile(name, Options::default(), None, &mut state).map(|(profile, _)| profile)),
            },
            (None, true) => Some(profiles.create_transient_profile(None, &mut state).map(|(profile, _)| profile)),
            (None, false) => None,
        };
        let current = match forced {
            Some(Ok(profile)) => profile,
            Some(Err(error)) => {
                eprintln!("[profiles] {error}");
                profiles.default_profile().clone()
            }
            None => workspace.as_ref().and_then(|workspace| profiles.profile_for_workspace(workspace)).unwrap_or(profiles.default_profile()).clone(),
        };
        if let (Some(workspace), false) = (&workspace, extension_development)
            && let Err(error) = profiles.set_profile_for_workspace(workspace, &current.id, &mut state)
        {
            eprintln!("[profiles] {error}");
        }
        ProfileState { state_file, profiles, current, workspace }
    }

    /// The configuration files of the current profile; a profile with its own settings also reads the default
    /// profile's settings.json for the settings applied to all profiles.
    pub fn user_files(&self) -> UserFiles {
        user_files(&self.current, self.profiles.default_profile())
    }

    /// Runs a change of the profiles against `storage.json` as it is on disk now.
    pub fn edit<T>(&mut self, change: impl FnOnce(&mut Profiles, &mut FileStorage) -> T) -> T {
        let mut state = FileStorage::open(&self.state_file);
        change(&mut self.profiles, &mut state)
    }
}

pub fn user_files(profile: &Profile, default: &Profile) -> UserFiles {
    let shares_settings = profile.is_default || profile.settings_resource == default.settings_resource;
    UserFiles {
        settings: profile.settings_resource.clone(),
        tasks: profile.tasks_resource.clone(),
        mcp: profile.mcp_resource.clone(),
        default_settings: (!shares_settings).then(|| default.settings_resource.clone()),
    }
}

/// `toWorkspaceIdentifier(workspace).uri.toString()`: the workspace file, else the folder.
pub fn workspace_id(workspace: &Workspace) -> WorkspaceId {
    let path = workspace.file.as_deref().unwrap_or(&workspace.folders[0].path);
    WorkspaceId::Uri(crate::editor_history::file_uri_string(path))
}

#[cfg(test)]
mod tests;
