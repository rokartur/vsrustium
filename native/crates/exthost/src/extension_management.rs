//! The user extensions folder: `extensions.json` as extensionsProfileScannerService.ts keeps it, the `.obsolete`
//! list and `.vsctmp` folders of extensionManagementService.ts `ExtensionsScanner`, and the install and uninstall
//! tasks of a VSIX (`InstallExtensionInProfileTask`, `UninstallExtensionInProfileTask`).

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, bail};
use serde_json::{Map, Value, json};

use config::registry::Registry;

use crate::extension_validator::{is_engine_valid, semver_version, validate_extension_manifest};
use crate::gallery::GalleryExtension;
use crate::main_thread_configuration::changed_args;
use crate::scan::{ScannedExtension, file_uri, scan_extension};
use crate::session::Session;
use crate::zip::Archive;

/// `DELETED_FOLDER_POSTFIX`.
const DELETED_FOLDER_POSTFIX: &str = ".vsctmp";

pub struct UserExtensions {
    pub folder: PathBuf,
    pub product_version: String,
}

impl UserExtensions {
    /// `scanUserExtensions` without invalid extensions: the profile's entries, the latest version of each id.
    /// An extension that fails validation is left out with its reason on stderr, as the scanner logs it.
    pub fn scan(&self) -> anyhow::Result<Vec<ScannedExtension>> {
        let mut extensions: Vec<ScannedExtension> = Vec::new();
        for entry in self.profile()? {
            let location = self.entry_location(&entry);
            let extension = match self.scan_valid(&location, &entry["metadata"]) {
                Ok(extension) => extension,
                Err(error) => {
                    eprintln!("[extensions] skipping {}: {error:#}", location.display());
                    continue;
                }
            };
            match extensions.iter().position(|other| other.id.eq_ignore_ascii_case(&extension.id)) {
                Some(index) if version_of(&extensions[index]) < version_of(&extension) => extensions[index] = extension,
                Some(_) => {}
                None => extensions.push(extension),
            }
        }
        Ok(extensions)
    }

    /// `install(vsix)`: checks the engine, extracts into `<publisher>.<name>-<version>` and adds it to the profile,
    /// replacing other versions. Returns the installed extension as the host gets it.
    pub fn install_vsix(&self, vsix: &Path) -> anyhow::Result<ScannedExtension> {
        self.install(vsix, None)
    }

    /// `InstallGalleryExtensionTask`: the downloaded VSIX of `gallery`, which must be the extension and version it names.
    /// The same version already installed only gets the gallery metadata.
    pub fn install_gallery(&self, vsix: &Path, gallery: &GalleryExtension) -> anyhow::Result<ScannedExtension> {
        self.install(vsix, Some(gallery))
    }

    fn install(&self, vsix: &Path, gallery: Option<&GalleryExtension>) -> anyhow::Result<ScannedExtension> {
        let archive = Archive::open(std::fs::read(vsix).with_context(|| format!("reading {}", vsix.display()))?)?;
        let manifest_entry = archive.entry("extension/package.json").context("extension/package.json not found in zip")?;
        let manifest: Value = serde_json::from_slice(&archive.read(manifest_entry)?).map_err(|_| anyhow::anyhow!("VSIX invalid: package.json is not a JSON file."))?;
        let id = gallery_extension_id(&manifest);
        let version = manifest["version"].as_str().unwrap_or_default();
        if let Some(gallery) = gallery
            && (!gallery.id.eq_ignore_ascii_case(&id) || gallery.version != version)
        {
            bail!("Cannot install '{}' extension because of manifest mismatch with Marketplace", gallery.id);
        }
        if let Some(engine) = manifest["engines"]["vscode"].as_str().filter(|engine| !engine.is_empty())
            && !is_engine_valid(engine, &self.product_version, None)
        {
            bail!("Unable to install extension '{id}' as it is not compatible with VS Code '{}'.", self.product_version);
        }
        let key = gallery.map_or_else(|| format!("{id}-{version}"), crate::gallery::extension_key);
        // The folder name comes from the VSIX: a `/` or `..` in `name` or `version` would reach outside the folder.
        if key.contains(['/', '\\']) || key.starts_with('.') || key.contains("..") {
            bail!("VSIX invalid: '{key}' is not a valid extension folder name.");
        }
        let profile = self.profile()?;
        let existing = profile.iter().find(|entry| entry_id(entry).eq_ignore_ascii_case(&id));

        let mut metadata = Map::new();
        if let Some(existing) = existing {
            for key in ["isApplicationScoped", "isMachineScoped", "isBuiltin", "pinned"] {
                metadata.insert(key.into(), json!(existing["metadata"][key] == true));
            }
        }
        metadata.insert("installedTimestamp".into(), json!(now_millis()));
        metadata.insert("source".into(), json!(if gallery.is_some() { "gallery" } else { "vsix" }));
        let location = self.folder.join(&key);
        match gallery {
            None => {
                // Installing a VSIX always replaces the folder of the same version, installed or marked as removed.
                self.unset_for_removal(&key)?;
                if location.exists() {
                    delete_folder(&location).with_context(|| format!("Please restart VS Code before reinstalling {}.", display_name(&manifest)))?;
                }
                self.extract(&archive, &location)?;
            }
            Some(gallery) => {
                let has_pre_release = existing.is_some_and(|existing| existing["metadata"]["hasPreReleaseVersion"] == true);
                let pre_release = existing.is_some_and(|existing| existing["metadata"]["preRelease"] == true);
                metadata.insert("id".into(), json!(gallery.uuid));
                metadata.insert("publisherId".into(), json!(gallery.publisher_id));
                metadata.insert("publisherDisplayName".into(), json!(gallery.publisher_display_name));
                metadata.insert("targetPlatform".into(), json!(gallery.target_platform));
                metadata.insert("updated".into(), json!(existing.is_some()));
                metadata.insert("private".into(), json!(gallery.private));
                metadata.insert("isPreReleaseVersion".into(), json!(gallery.is_pre_release_version));
                metadata.insert("hasPreReleaseVersion".into(), json!(has_pre_release || gallery.is_pre_release_version));
                metadata.insert("preRelease".into(), json!(gallery.is_pre_release_version || pre_release));
                // A version marked as removed comes back as it is (`unsetIfRemoved`); else it is extracted.
                let was_removed = self.removed()?.contains_key(&key);
                self.unset_for_removal(&key)?;
                let installed = existing.is_some_and(|existing| existing["version"] == version);
                if !(was_removed || installed) || !location.join("package.json").is_file() {
                    if location.exists() {
                        delete_folder(&location)?;
                    }
                    self.extract(&archive, &location)?;
                }
            }
        }
        self.add_to_profile(&id, version, &location, Value::Object(metadata))?;
        self.scan_valid(&location, &Value::Null)
    }

    /// `UninstallExtensionInProfileTask`, then the folder is marked for removal as no profile has it
    /// (ExtensionsWatcher `deleteExtensions`); the next start deletes it.
    pub fn uninstall(&self, id: &str) -> anyhow::Result<()> {
        let mut profile = self.profile()?;
        let removed: Vec<Value> = profile.extract_if(.., |entry| entry_id(entry).eq_ignore_ascii_case(id)).collect();
        if removed.is_empty() {
            bail!("Extension '{id}' is not installed.");
        }
        self.write_profile(&profile)?;
        self.set_for_removal(&removed)
    }

    /// `ExtensionsScanner.cleanUp` at startup: deletes `.vsctmp` folders and the folders marked as removed.
    pub fn clean_up(&self) -> anyhow::Result<()> {
        let Ok(children) = std::fs::read_dir(&self.folder) else { return Ok(()) };
        for child in children.flatten() {
            if child.file_name().to_string_lossy().ends_with(DELETED_FOLDER_POSTFIX) && child.path().is_dir() {
                let _ = std::fs::remove_dir_all(child.path());
            }
        }
        let removed = self.removed()?;
        for key in removed.keys() {
            let location = self.folder.join(key);
            // Only folders the installer wrote (with `installedTimestamp`) are deleted.
            let installed = scan_extension(&location, false).is_ok_and(|extension| extension.description["__metadata"]["installedTimestamp"].is_number());
            if installed && delete_folder(&location).is_ok() {
                self.unset_for_removal(key)?;
            }
        }
        Ok(())
    }

    /// The profile's entries; without `extensions.json` the folder's extensions are added first
    /// (`doInitializeDefaultProfileExtensions`).
    fn profile(&self) -> anyhow::Result<Vec<Value>> {
        let file = self.folder.join("extensions.json");
        match std::fs::read_to_string(&file) {
            Ok(text) if text.trim().is_empty() => Ok(Vec::new()),
            Ok(text) => match serde_json::from_str(&text)? {
                Value::Array(entries) => Ok(entries),
                _ => bail!("Invalid extensions content in {}", file.display()),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => self.initialize_profile(),
            Err(error) => Err(error.into()),
        }
    }

    fn initialize_profile(&self) -> anyhow::Result<Vec<Value>> {
        let removed = self.removed()?;
        let mut entries = Vec::new();
        for child in std::fs::read_dir(&self.folder).into_iter().flatten().flatten() {
            let name = child.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || removed.contains_key(&name) || !child.path().join("package.json").is_file() {
                continue;
            }
            let Ok(extension) = scan_extension(&child.path(), false) else { continue };
            let version = extension.description["version"].as_str().unwrap_or_default().to_owned();
            let metadata = extension.description.get("__metadata").cloned().unwrap_or(Value::Null);
            entries.push(profile_entry(&extension.id.to_lowercase(), &version, &child.path(), metadata));
        }
        if self.folder.is_dir() {
            self.write_profile(&entries)?;
        }
        Ok(entries)
    }

    /// `addExtensionsToProfile`: other versions of the id leave the profile and are marked for removal.
    fn add_to_profile(&self, id: &str, version: &str, location: &Path, metadata: Value) -> anyhow::Result<()> {
        let mut profile = self.profile()?;
        let replaced: Vec<Value> = profile.extract_if(.., |entry| entry_id(entry).eq_ignore_ascii_case(id)).collect();
        profile.push(profile_entry(id, version, location, metadata));
        self.write_profile(&profile)?;
        let other_versions: Vec<Value> = replaced.into_iter().filter(|entry| self.entry_location(entry) != location).collect();
        self.set_for_removal(&other_versions)
    }

    fn write_profile(&self, entries: &[Value]) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.folder)?;
        Ok(std::fs::write(self.folder.join("extensions.json"), serde_json::to_string(entries)?)?)
    }

    fn entry_location(&self, entry: &Value) -> PathBuf {
        match entry["relativeLocation"].as_str() {
            Some(relative) if !relative.is_empty() => self.folder.join(relative),
            _ => PathBuf::from(entry["location"]["path"].as_str().unwrap_or_default()),
        }
    }

    /// A user extension as `toExtensionDescription` makes it, after `validateExtensionManifest`.
    fn scan_valid(&self, location: &Path, profile_metadata: &Value) -> anyhow::Result<ScannedExtension> {
        let mut extension = scan_extension(location, false)?;
        let description = extension.description.as_object_mut().expect("package.json is an object");
        let mut metadata = description.remove("__metadata").unwrap_or(json!({}));
        if let (Some(metadata), Some(profile)) = (metadata.as_object_mut(), profile_metadata.as_object()) {
            metadata.extend(profile.clone());
        }
        if let Err(error) = validate_extension_manifest(&self.product_version, None, &extension.description, false) {
            bail!("{error}");
        }
        let description = &mut extension.description;
        if let Some(uuid) = metadata["id"].as_str() {
            description["uuid"] = json!(uuid);
            description["identifier"]["uuid"] = json!(uuid);
        }
        if let Some(name) = metadata["publisherDisplayName"].as_str() {
            description["publisherDisplayName"] = json!(name);
        }
        if let Some(platform) = metadata["targetPlatform"].as_str() {
            description["targetPlatform"] = json!(platform);
        }
        description["isBuiltin"] = json!(metadata["isBuiltin"] == true);
        description["isUserBuiltin"] = json!(metadata["isBuiltin"] == true);
        description["preRelease"] = json!(metadata["preRelease"] == true);
        Ok(extension)
    }

    /// `extractUserExtension`: into a `.<id>` folder first, `__metadata` written, then renamed into place.
    fn extract(&self, archive: &Archive, location: &Path) -> anyhow::Result<()> {
        let temp = self.folder.join(format!(".{}", unique_suffix()));
        std::fs::create_dir_all(&temp)?;
        let extracted = archive.extract("extension", &temp).and_then(|()| {
            let manifest_file = temp.join("package.json");
            let mut manifest: Value = serde_json::from_slice(&std::fs::read(&manifest_file)?)?;
            let size = folder_size(&temp);
            let metadata = manifest.as_object_mut().context("package.json is not an object")?.entry("__metadata").or_insert(json!({}));
            metadata["installedTimestamp"] = json!(now_millis());
            metadata["size"] = json!(size);
            std::fs::write(&manifest_file, serde_json::to_string_pretty(&manifest)?.replace("  ", "\t"))?;
            Ok(std::fs::rename(&temp, location)?)
        });
        if extracted.is_err() {
            let _ = std::fs::remove_dir_all(&temp);
        }
        extracted
    }

    /// `.obsolete`: extension keys mapped to `true`.
    fn removed(&self) -> anyhow::Result<Map<String, Value>> {
        match std::fs::read_to_string(self.folder.join(".obsolete")) {
            Ok(text) => Ok(serde_json::from_str(&text).unwrap_or_default()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Map::new()),
            Err(error) => Err(error.into()),
        }
    }

    fn write_removed(&self, removed: &Map<String, Value>) -> anyhow::Result<()> {
        let file = self.folder.join(".obsolete");
        if removed.is_empty() {
            return match std::fs::remove_file(file) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
                _ => Ok(()),
            };
        }
        Ok(std::fs::write(file, serde_json::to_string(removed)?)?)
    }

    fn set_for_removal(&self, entries: &[Value]) -> anyhow::Result<()> {
        let mut removed = self.removed()?;
        for entry in entries {
            let location = self.entry_location(entry);
            if location.exists() {
                removed.insert(format!("{}-{}", entry_id(entry).to_lowercase(), entry["version"].as_str().unwrap_or_default()), json!(true));
            }
        }
        self.write_removed(&removed)
    }

    fn unset_for_removal(&self, key: &str) -> anyhow::Result<()> {
        let mut removed = self.removed()?;
        if removed.remove(key).is_some() {
            self.write_removed(&removed)?;
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum Job {
    InstallVsix(PathBuf),
    InstallGallery(PathBuf, Box<GalleryExtension>),
    Uninstall(String),
    /// An enablement change: the extensions to start and the ids to stop, nothing on disk.
    Delta(Vec<ScannedExtension>, Vec<String>),
}

/// The window's extensions: the built-ins, the user folder and what the host runs.
pub struct WindowExtensions {
    pub builtins: Vec<ScannedExtension>,
    pub user: UserExtensions,
    pub running: Vec<ScannedExtension>,
}

impl WindowExtensions {
    /// Runs `job` on disk, then `_deltaExtensions`: the configuration extension point, then the host.
    /// Answers the running extensions afterwards; an extension that already activated stays until a restart.
    pub fn run(&mut self, session: &mut Session, job: Job) -> anyhow::Result<Vec<ScannedExtension>> {
        let (to_add, to_remove) = match job {
            Job::InstallVsix(vsix) => {
                let installed = self.user.install_vsix(&vsix)?;
                let id = installed.id.clone();
                (vec![installed], vec![id])
            }
            Job::InstallGallery(vsix, gallery) => {
                let installed = self.user.install_gallery(&vsix, &gallery)?;
                let id = installed.id.clone();
                (vec![installed], vec![id])
            }
            Job::Uninstall(id) => {
                self.user.uninstall(&id)?;
                let builtin = self.builtins.iter().find(|builtin| builtin.id.eq_ignore_ascii_case(&id)).cloned();
                (builtin.into_iter().collect(), vec![id])
            }
            Job::Delta(to_add, to_remove) => (to_add, to_remove),
        };
        let (to_add, to_remove) = session.applicable_delta(&to_add, &to_remove);
        self.running.retain(|extension| !to_remove.contains(&extension.id.to_lowercase()));
        self.running.extend(to_add.iter().map(|extension| (*extension).clone()));
        if let Some(configuration) = &session.configuration {
            let described: Vec<(&str, &Value)> = self.running.iter().map(|extension| (extension.id.as_str(), &extension.description)).collect();
            let mut settings = configuration.settings.lock().unwrap();
            let change = settings.set_registry(Registry::with_extensions(&described));
            let args = changed_args(&settings, configuration.send_scopes, &change);
            drop(settings);
            if !change.is_empty() {
                session.notify("ExtHostConfiguration", "$acceptConfigurationChanged", args);
            }
        }
        session.delta_extensions(&to_add, &to_remove)?;
        Ok(self.running.clone())
    }
}

/// `dedupExtensions`: a user extension replaces the built-in with its id unless the built-in is newer.
pub fn add_user_extensions(extensions: &mut Vec<ScannedExtension>, user: Vec<ScannedExtension>) {
    for extension in user {
        match extensions.iter().position(|other| other.id.eq_ignore_ascii_case(&extension.id)) {
            Some(index) if version_of(&extensions[index]) > version_of(&extension) => {}
            Some(index) => extensions[index] = extension,
            None => extensions.push(extension),
        }
    }
}

/// `getGalleryExtensionId`: lowercase `<publisher>.<name>`.
pub fn gallery_extension_id(manifest: &Value) -> String {
    let publisher = manifest["publisher"].as_str().unwrap_or("undefined_publisher");
    format!("{}.{}", publisher.to_lowercase(), manifest["name"].as_str().unwrap_or_default().to_lowercase())
}

/// `IStoredProfileExtension`: the old `location` is kept beside `relativeLocation` so old clients can read it.
fn profile_entry(id: &str, version: &str, location: &Path, metadata: Value) -> Value {
    let relative = location.file_name().map(|name| name.to_string_lossy().into_owned());
    let mut entry = json!({ "identifier": { "id": id }, "version": version, "location": file_uri(location), "relativeLocation": relative });
    if let Some(uuid) = metadata["id"].as_str() {
        entry["identifier"]["uuid"] = json!(uuid);
    }
    if !metadata.is_null() {
        entry["metadata"] = metadata;
    }
    entry
}

fn entry_id(entry: &Value) -> &str {
    entry["identifier"]["id"].as_str().unwrap_or_default()
}

fn version_of(extension: &ScannedExtension) -> Option<semver::Version> {
    semver_version(extension.description["version"].as_str().unwrap_or_default())
}

fn display_name(manifest: &Value) -> &str {
    manifest["displayName"].as_str().or(manifest["name"].as_str()).unwrap_or_default()
}

/// `deleteExtensionFromLocation`: renamed to `<name>.<suffix>.vsctmp` first, so a half-deleted folder is never scanned.
fn delete_folder(location: &Path) -> anyhow::Result<()> {
    let name = location.file_name().context("extension folder has no name")?.to_string_lossy();
    let renamed = location.with_file_name(format!("{name}.{}{DELETED_FOLDER_POSTFIX}", unique_suffix()));
    std::fs::rename(location, &renamed)?;
    Ok(std::fs::remove_dir_all(renamed)?)
}

fn folder_size(folder: &Path) -> u64 {
    std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .map(|child| match child.file_type() {
            Ok(kind) if kind.is_dir() => folder_size(&child.path()),
            _ => child.metadata().map_or(0, |metadata| metadata.len()),
        })
        .sum()
}

fn now_millis() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).expect("clock after 1970").as_millis() as u64
}

fn unique_suffix() -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).expect("clock after 1970").as_nanos();
    format!("{:x}{:x}", std::process::id(), nanos)
}

#[cfg(test)]
mod tests;
