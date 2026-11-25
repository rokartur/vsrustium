//! platform/userDataProfile/common/userDataProfile.ts and node/userDataProfile.ts: `UserDataProfilesService`, the
//! profiles stored under `userDataProfiles` and their workspace and empty window associations under
//! `profileAssociations` in `storage.json`. Upstream's change events are the `Change` each mutation returns.
//! The Agents window profile is left out (no AI by decision).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::state::FileStorage;

pub const DEFAULT_PROFILE_ID: &str = "__default__profile__";
const PROFILES_KEY: &str = "userDataProfiles";
const PROFILE_ASSOCIATIONS_KEY: &str = "profileAssociations";
const SYSTEM_PROFILES_HOME: &str = "builtin";

/// `UseDefaultProfileFlags`: resource type (`settings`, `keybindings`, `globalState`, ...) to whether the profile
/// shares the default profile's resource.
pub type UseDefaultFlags = BTreeMap<String, bool>;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Profile {
    pub id: String,
    pub is_default: bool,
    pub name: String,
    pub icon: Option<String>,
    pub location: PathBuf,
    pub global_storage_home: PathBuf,
    pub settings_resource: PathBuf,
    pub keybindings_resource: PathBuf,
    pub tasks_resource: PathBuf,
    pub snippets_home: PathBuf,
    pub prompts_home: PathBuf,
    pub extensions_resource: PathBuf,
    pub mcp_resource: PathBuf,
    pub language_models_resource: PathBuf,
    pub cache_home: PathBuf,
    pub use_default_flags: Option<UseDefaultFlags>,
    pub is_transient: bool,
    /// The workspace and folder URIs that open in this profile; `None` for none, as upstream never keeps `[]`.
    pub workspaces: Option<Vec<String>>,
}

#[derive(Clone, Debug, Default)]
pub struct Options {
    pub icon: Option<String>,
    pub use_default_flags: Option<UseDefaultFlags>,
    pub transient: bool,
    pub workspaces: Option<Vec<String>>,
}

/// `IUserDataProfileUpdateOptions`: `None` keeps the current value; `icon: Some(None)` removes the icon.
#[derive(Clone, Debug, Default)]
pub struct UpdateOptions {
    pub name: Option<String>,
    pub icon: Option<Option<String>>,
    pub transient: Option<bool>,
    pub use_default_flags: Option<UseDefaultFlags>,
    pub workspaces: Option<Vec<String>>,
}

/// `IAnyWorkspaceIdentifier` as `getWorkspace` reduces it: a folder or workspace file URI, or an empty window's id.
#[derive(Clone, Debug, PartialEq)]
pub enum WorkspaceId {
    Uri(String),
    EmptyWindow(String),
}

/// `DidChangeProfilesEvent` without `all`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Change {
    pub added: Vec<Profile>,
    pub removed: Vec<Profile>,
    pub updated: Vec<Profile>,
}

/// The folders the profiles live in: `userRoamingDataHome` (the `User` folder, the default profile's location),
/// `cacheHome` and the extensions folder whose `extensions.json` the default profile uses.
#[derive(Clone, Debug)]
pub struct Paths {
    pub user_home: PathBuf,
    pub cache_home: PathBuf,
    pub extensions_dir: Option<PathBuf>,
}

pub struct Profiles {
    pub profiles_home: PathBuf,
    profiles_cache_home: PathBuf,
    /// The default profile first, then the stored ones.
    persisted: Vec<Profile>,
    transient: Vec<Profile>,
    empty_windows: BTreeMap<String, String>,
    transient_empty_windows: BTreeMap<String, String>,
}

/// `toUserDataProfile`.
pub fn to_profile(id: &str, name: &str, location: &Path, profiles_cache_home: &Path, options: &Options, default: Option<&Profile>) -> Profile {
    let mut profile = Profile {
        id: id.into(),
        name: name.into(),
        icon: options.icon.clone(),
        location: location.into(),
        cache_home: profiles_cache_home.join(id),
        use_default_flags: options.use_default_flags.clone(),
        is_transient: options.transient,
        workspaces: options.workspaces.clone(),
        ..Profile::default()
    };
    let uses_default = |resource: &str| options.use_default_flags.as_ref().is_some_and(|flags| flags.get(resource) == Some(&true));
    let shared = |resource: &str| default.filter(|_| uses_default(resource));
    profile.global_storage_home = shared("globalState").map_or_else(|| location.join("globalStorage"), |default| default.global_storage_home.clone());
    profile.settings_resource = shared("settings").map_or_else(|| location.join("settings.json"), |default| default.settings_resource.clone());
    profile.keybindings_resource = shared("keybindings").map_or_else(|| location.join("keybindings.json"), |default| default.keybindings_resource.clone());
    profile.tasks_resource = shared("tasks").map_or_else(|| location.join("tasks.json"), |default| default.tasks_resource.clone());
    profile.snippets_home = shared("snippets").map_or_else(|| location.join("snippets"), |default| default.snippets_home.clone());
    profile.prompts_home = shared("prompts").map_or_else(|| location.join("prompts"), |default| default.prompts_home.clone());
    profile.extensions_resource = shared("extensions").map_or_else(|| location.join("extensions.json"), |default| default.extensions_resource.clone());
    profile.mcp_resource = shared("mcp").map_or_else(|| location.join("mcp.json"), |default| default.mcp_resource.clone());
    profile.language_models_resource =
        shared("languageModels").map_or_else(|| location.join("chatLanguageModels.json"), |default| default.language_models_resource.clone());
    profile
}

impl Profiles {
    /// `profilesObject`: the default profile, the valid stored profiles and their associations from `state`.
    pub fn load(state: &FileStorage, paths: &Paths) -> Profiles {
        let profiles_home = paths.user_home.join("profiles");
        let profiles_cache_home = paths.cache_home.join("CachedProfilesData");
        let mut default = to_profile(DEFAULT_PROFILE_ID, "Default", &paths.user_home, &profiles_cache_home, &Options::default(), None);
        default.is_default = true;
        if let Some(extensions_dir) = &paths.extensions_dir {
            default.extensions_resource = extensions_dir.join("extensions.json");
        }
        let mut persisted = vec![default];
        for stored in state.get_item(PROFILES_KEY).and_then(Value::as_array).into_iter().flatten() {
            let Some((name, location)) = valid_stored_profile(stored, &profiles_home) else {
                eprintln!("[profiles] Skipping the invalid stored profile {stored}");
                continue;
            };
            let id = location.file_name().map_or_else(String::new, |id| id.to_string_lossy().into_owned());
            let options = Options {
                icon: stored["icon"].as_str().map(String::from),
                use_default_flags: serde_json::from_value(stored["useDefaultFlags"].clone()).ok(),
                ..Options::default()
            };
            let profile = to_profile(&id, &name, &location, &profiles_cache_home, &options, Some(&persisted[0]));
            persisted.push(profile);
        }
        let associations = state.get_item(PROFILE_ASSOCIATIONS_KEY);
        for (workspace, id) in associations.and_then(|associations| associations["workspaces"].as_object()).into_iter().flatten() {
            if let Some(profile) = persisted.iter_mut().find(|profile| Some(profile.id.as_str()) == id.as_str()) {
                profile.workspaces.get_or_insert_with(Vec::new).push(workspace.clone());
            }
        }
        let mut empty_windows = BTreeMap::new();
        for (window, id) in associations.and_then(|associations| associations["emptyWindows"].as_object()).into_iter().flatten() {
            if let Some(id) = id.as_str().filter(|id| persisted.iter().any(|profile| profile.id == *id)) {
                empty_windows.insert(window.clone(), id.to_string());
            }
        }
        Profiles { profiles_home, profiles_cache_home, persisted, transient: Vec::new(), empty_windows, transient_empty_windows: BTreeMap::new() }
    }

    pub fn default_profile(&self) -> &Profile {
        &self.persisted[0]
    }

    /// `profiles`: the default profile, the stored ones, then the transient ones.
    pub fn profiles(&self) -> Vec<Profile> {
        self.persisted.iter().chain(&self.transient).cloned().collect()
    }

    /// `createTransientProfile`: named `Temp <n>` after the highest `Temp <n>` so far.
    pub fn create_transient_profile(&mut self, workspace: Option<&WorkspaceId>, state: &mut FileStorage) -> Result<(Profile, Change), String> {
        let mut index = 0;
        for profile in self.persisted.iter().chain(&self.transient) {
            let number = profile.name.split_once("Temp ").and_then(|(_, rest)| rest.split(|c: char| !c.is_ascii_digit()).next()?.parse::<u32>().ok());
            index = index.max(number.unwrap_or(0));
        }
        let name = format!("Temp {}", index + 1);
        self.create_profile(&new_profile_id(), &name, Options { transient: true, ..Options::default() }, workspace, state)
    }

    /// `createNamedProfile`: a new id, `hash(generateUuid()).toString(16)`.
    pub fn create_named_profile(&mut self, name: &str, options: Options, workspace: Option<&WorkspaceId>, state: &mut FileStorage) -> Result<(Profile, Change), String> {
        self.create_profile(&new_profile_id(), name, options, workspace, state)
    }

    /// `createProfile`: creates the profile's folder; a persistent profile's name must be unique.
    pub fn create_profile(&mut self, id: &str, name: &str, mut options: Options, workspace: Option<&WorkspaceId>, state: &mut FileStorage) -> Result<(Profile, Change), String> {
        if name.is_empty() {
            return Err("Name of the profile is mandatory and must be of type `string`".into());
        }
        let exists = self.persisted.iter().chain(&self.transient).any(|profile| profile.id == id || (!profile.is_transient && !options.transient && profile.name == name));
        if exists {
            return Err(format!("Profile with {name} name already exists"));
        }
        if let Some(WorkspaceId::Uri(uri)) = workspace {
            options.workspaces = Some(vec![uri.clone()]);
        }
        let profile = to_profile(id, name, &self.profiles_home.join(id), &self.profiles_cache_home, &options, Some(self.default_profile()));
        fs::create_dir_all(&profile.location).map_err(|error| format!("{}: {error}", profile.location.display()))?;
        if let Some(WorkspaceId::EmptyWindow(window)) = workspace {
            self.update_empty_window_association(window, Some(&profile), profile.is_transient);
        }
        let change = self.update_profiles(vec![profile.clone()], Vec::new(), Vec::new(), state);
        let created = self.find(id).cloned().unwrap_or(profile);
        Ok((created, change))
    }

    /// `updateProfile`: a profile's name, icon, flags or workspaces; giving workspaces to one profile takes them
    /// from the others. The default profile only takes workspaces.
    pub fn update_profile(&mut self, id: &str, options: UpdateOptions, state: &mut FileStorage) -> Result<(Profile, Change), String> {
        let Some(profile) = self.find(id).cloned() else { return Err(format!("Profile '{id}' does not exist")) };
        let mut updated = Vec::new();
        for existing in self.persisted.iter().chain(&self.transient) {
            if existing.id == profile.id {
                if !existing.is_default {
                    let flags = options.use_default_flags.clone().or_else(|| existing.use_default_flags.clone());
                    let next = Options {
                        icon: options.icon.clone().unwrap_or_else(|| existing.icon.clone()),
                        transient: options.transient.unwrap_or(existing.is_transient),
                        use_default_flags: flags,
                        workspaces: options.workspaces.clone().or_else(|| existing.workspaces.clone()),
                    };
                    let name = options.name.as_deref().unwrap_or(&existing.name);
                    updated.push(to_profile(&existing.id, name, &existing.location, &self.profiles_cache_home, &next, Some(self.default_profile())));
                } else if let Some(workspaces) = &options.workspaces {
                    updated.push(Profile { workspaces: Some(workspaces.clone()), ..existing.clone() });
                }
            } else if let Some(taken) = &options.workspaces {
                let kept: Option<Vec<String>> = existing.workspaces.as_ref().map(|workspaces| workspaces.iter().filter(|workspace| !taken.contains(workspace)).cloned().collect());
                if existing.workspaces.as_ref().map(Vec::len) != kept.as_ref().map(Vec::len) {
                    updated.push(Profile { workspaces: kept, ..existing.clone() });
                }
            }
        }
        if updated.is_empty() {
            return Err(match profile.is_default {
                true => "Cannot update default profile".into(),
                false => format!("Profile '{}' does not exist", profile.name),
            });
        }
        let change = self.update_profiles(Vec::new(), Vec::new(), updated, state);
        let updated = self.find(id).cloned().ok_or_else(|| format!("Profile '{}' was not updated", profile.name))?;
        Ok((updated, change))
    }

    /// `removeProfile`: drops the profile and its cache folder; its folder goes on the next `clean_up`.
    pub fn remove_profile(&mut self, id: &str, state: &mut FileStorage) -> Result<Change, String> {
        let Some(profile) = self.find(id).cloned() else { return Err(format!("Profile '{id}' does not exist")) };
        if profile.is_default {
            return Err("Cannot remove default profile".into());
        }
        let change = self.update_profiles(Vec::new(), vec![profile.clone()], Vec::new(), state);
        match fs::remove_dir_all(&profile.cache_home) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => eprintln!("[profiles] {}: {error}", profile.cache_home.display()),
            _ => {}
        }
        Ok(change)
    }

    /// `setProfileForWorkspace`.
    pub fn set_profile_for_workspace(&mut self, workspace: &WorkspaceId, id: &str, state: &mut FileStorage) -> Result<Change, String> {
        let Some(profile) = self.find(id).cloned() else { return Err(format!("Profile '{id}' does not exist")) };
        match workspace {
            WorkspaceId::Uri(uri) => {
                let mut workspaces = profile.workspaces.clone().unwrap_or_default();
                if workspaces.contains(uri) {
                    return Ok(Change::default());
                }
                workspaces.push(uri.clone());
                Ok(self.update_profile(id, UpdateOptions { workspaces: Some(workspaces), ..UpdateOptions::default() }, state)?.1)
            }
            WorkspaceId::EmptyWindow(window) => {
                self.update_empty_window_association(window, Some(&profile), false);
                self.update_stored_profiles(state);
                Ok(Change::default())
            }
        }
    }

    /// `unsetWorkspace`.
    pub fn unset_workspace(&mut self, workspace: &WorkspaceId, transient: bool, state: &mut FileStorage) -> Change {
        match workspace {
            WorkspaceId::Uri(uri) => {
                let Some(profile) = self.profile_for_workspace(workspace).cloned() else { return Change::default() };
                let workspaces = profile.workspaces.unwrap_or_default().into_iter().filter(|workspace| workspace != uri).collect();
                self.update_profile(&profile.id, UpdateOptions { workspaces: Some(workspaces), ..UpdateOptions::default() }, state).map(|(_, change)| change).unwrap_or_default()
            }
            WorkspaceId::EmptyWindow(window) => {
                self.update_empty_window_association(window, None, transient);
                self.update_stored_profiles(state);
                Change::default()
            }
        }
    }

    /// `resetWorkspaces`.
    pub fn reset_workspaces(&mut self, state: &mut FileStorage) -> Change {
        self.empty_windows.clear();
        self.transient_empty_windows.clear();
        let updated: Vec<Profile> = self.persisted.iter().chain(&self.transient).map(|profile| Profile { workspaces: None, ..profile.clone() }).collect();
        self.update_profiles(Vec::new(), Vec::new(), updated, state)
    }

    /// `getProfileForWorkspace`.
    pub fn profile_for_workspace(&self, workspace: &WorkspaceId) -> Option<&Profile> {
        match workspace {
            WorkspaceId::Uri(uri) => self.persisted.iter().chain(&self.transient).find(|profile| profile.workspaces.as_ref().is_some_and(|workspaces| workspaces.contains(uri))),
            WorkspaceId::EmptyWindow(window) => {
                let id = self.empty_windows.get(window).or_else(|| self.transient_empty_windows.get(window))?;
                self.find(id)
            }
        }
    }

    /// `cleanUp`: deletes the folders under `profiles` no profile uses, and drops invalid stored profiles.
    pub fn clean_up(&mut self, state: &mut FileStorage) {
        for entry in fs::read_dir(&self.profiles_home).into_iter().flatten().flatten() {
            let path = entry.path();
            let used = self.persisted.iter().chain(&self.transient).any(|profile| profile.location == path);
            if path.is_dir() && entry.file_name() != SYSTEM_PROFILES_HOME && !used {
                if let Err(error) = fs::remove_dir_all(&path) {
                    eprintln!("[profiles] Error deleting redundant profile folders {}: {error}", path.display());
                }
            }
        }
        let stored = state.get_item(PROFILES_KEY).and_then(Value::as_array).cloned().unwrap_or_default();
        let valid: Vec<Value> = stored.iter().filter(|stored| valid_stored_profile(stored, &self.profiles_home).is_some()).cloned().collect();
        if valid.len() != stored.len() {
            state.set_item(PROFILES_KEY, Some(Value::Array(valid)).filter(|valid| valid.as_array().is_some_and(|valid| !valid.is_empty())));
        }
    }

    /// `cleanUpTransientProfiles`: removes the transient profiles no workspace or window uses.
    pub fn clean_up_transient_profiles(&mut self, state: &mut FileStorage) {
        let unused: Vec<String> = self.transient.iter().filter(|profile| !self.is_associated(profile)).map(|profile| profile.id.clone()).collect();
        for id in unused {
            let _ = self.remove_profile(&id, state);
        }
    }

    fn find(&self, id: &str) -> Option<&Profile> {
        self.persisted.iter().chain(&self.transient).find(|profile| profile.id == id)
    }

    fn is_associated(&self, profile: &Profile) -> bool {
        profile.workspaces.as_ref().is_some_and(|workspaces| !workspaces.is_empty())
            || self.empty_windows.values().chain(self.transient_empty_windows.values()).any(|id| *id == profile.id)
    }

    /// `updateProfiles`: applies the change, moves profiles between the stored and transient lists and saves.
    fn update_profiles(&mut self, added: Vec<Profile>, removed: Vec<Profile>, mut updated: Vec<Profile>, state: &mut FileStorage) -> Change {
        for profile in &mut updated {
            if profile.workspaces.as_ref().is_some_and(Vec::is_empty) {
                profile.workspaces = None;
            }
        }
        let all: Vec<Profile> = self.persisted.drain(..).chain(self.transient.drain(..)).chain(added.iter().cloned()).collect();
        let was_transient: Vec<String> = all.iter().filter(|profile| profile.is_transient).map(|profile| profile.id.clone()).collect();
        for mut profile in all {
            if removed.iter().any(|removed| removed.id == profile.id) {
                self.empty_windows.retain(|_, id| *id != profile.id);
                continue;
            }
            if let Some(update) = updated.iter().find(|update| update.id == profile.id) {
                profile = update.clone();
            }
            if profile.workspaces.as_ref().is_some_and(Vec::is_empty) {
                profile.workspaces = None;
            }
            if profile.is_transient {
                self.transient.push(profile);
                continue;
            }
            if was_transient.contains(&profile.id) {
                let window = self.transient_empty_windows.iter().find(|(_, id)| **id == profile.id).map(|(window, _)| window.clone());
                if let Some(window) = window {
                    self.transient_empty_windows.remove(&window);
                    self.empty_windows.insert(window, profile.id.clone());
                }
            }
            self.persisted.push(profile);
        }
        self.update_stored_profiles(state);
        Change { added, removed, updated }
    }

    fn update_empty_window_association(&mut self, window: &str, profile: Option<&Profile>, transient: bool) {
        let transient = transient || profile.is_some_and(|profile| profile.is_transient);
        if !transient {
            self.transient_empty_windows.remove(window);
        }
        let windows = if transient { &mut self.transient_empty_windows } else { &mut self.empty_windows };
        match profile {
            Some(profile) => windows.insert(window.into(), profile.id.clone()),
            None => windows.remove(window),
        };
    }

    /// `updateStoredProfiles`: the stored profiles with their location relative to `profiles`, and the workspace
    /// and empty window associations; transient profiles are not stored.
    fn update_stored_profiles(&self, state: &mut FileStorage) {
        let mut stored = Vec::new();
        let mut workspaces = Map::new();
        for profile in &self.persisted {
            if !profile.is_default {
                let location = profile.location.strip_prefix(&self.profiles_home).map_or_else(|_| profile.location.to_string_lossy(), |relative| relative.to_string_lossy());
                let mut entry = json!({ "location": location, "name": profile.name });
                if let Some(icon) = &profile.icon {
                    entry["icon"] = json!(icon);
                }
                if let Some(flags) = &profile.use_default_flags {
                    entry["useDefaultFlags"] = json!(flags);
                }
                stored.push(entry);
            }
            for workspace in profile.workspaces.iter().flatten() {
                workspaces.insert(workspace.clone(), json!(profile.id));
            }
        }
        let empty_windows: Map<String, Value> = self.empty_windows.iter().map(|(window, id)| (window.clone(), json!(id))).collect();
        let associations = json!({ "workspaces": workspaces, "emptyWindows": empty_windows });
        let stored = Some(Value::Array(stored)).filter(|stored| stored.as_array().is_some_and(|stored| !stored.is_empty()));
        state.set_items(vec![(PROFILE_ASSOCIATIONS_KEY.into(), Some(associations)), (PROFILES_KEY.into(), stored)]);
    }
}

/// `isInvalidProfile` and the stored location: a string relative to `profiles`, or a file URI.
fn valid_stored_profile(stored: &Value, profiles_home: &Path) -> Option<(String, PathBuf)> {
    let name = stored["name"].as_str().filter(|name| !name.is_empty())?;
    let location = match &stored["location"] {
        Value::String(relative) if !relative.is_empty() => profiles_home.join(relative),
        Value::Object(uri) => PathBuf::from(uri.get("path")?.as_str()?),
        _ => return None,
    };
    Some((name.to_string(), location))
}

/// hash.ts `hash(generateUuid()).toString(16)`.
fn new_profile_id() -> String {
    let mut bytes = [0u8; 16];
    let random = fs::File::open("/dev/urandom").and_then(|mut random| std::io::Read::read_exact(&mut random, &mut bytes));
    random.expect("/dev/urandom is readable");
    bytes[6] = bytes[6] & 0x0f | 0x40;
    bytes[8] = bytes[8] & 0x3f | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    let uuid = format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..]);
    hash_hex(&uuid)
}

/// hash.ts `hash(string).toString(16)`: the int32 string hash, a negative value with a minus sign.
pub fn hash_hex(value: &str) -> String {
    let mut hash: i32 = 0;
    for unit in std::iter::once(149417).chain(value.encode_utf16().map(i32::from)) {
        hash = hash.wrapping_shl(5).wrapping_sub(hash).wrapping_add(unit);
    }
    match hash < 0 {
        true => format!("-{:x}", -(hash as i64)),
        false => format!("{hash:x}"),
    }
}

#[cfg(test)]
mod tests;
