//! Language packs (platform/languagePacks/node/languagePacks.ts) and the display language the window starts with
//! (base/node/nls.ts `resolveNLSConfiguration`). `languagepacks.json` in the user data folder maps each language
//! to the installed extensions contributing `localizations` and their translation files by id.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use md5::Digest;
use serde_json::{Map, Value, json};

use crate::scan::ScannedExtension;

/// `INLSConfiguration` fields the port reads: the language the window shows and its translation files by id.
#[derive(Clone, Debug, PartialEq)]
pub struct Nls {
    pub user_locale: String,
    /// `resolvedLanguage`: the language pack's language, else "en".
    pub language: String,
    /// The language pack's `translations` (id to file path); empty for "en".
    pub translations: Map<String, Value>,
}

impl Nls {
    pub fn english(user_locale: &str) -> Nls {
        Nls { user_locale: user_locale.into(), language: "en".into(), translations: Map::new() }
    }
}

static NLS: OnceLock<Nls> = OnceLock::new();
static USER_DATA: OnceLock<PathBuf> = OnceLock::new();

/// Set once at startup: the display language changes only with a restart, as upstream.
pub fn set_nls(user_data: &Path, nls: Nls) {
    let _ = NLS.set(nls);
    let _ = USER_DATA.set(user_data.to_path_buf());
}

/// `MainThreadLocalization`: `$fetchBuiltInBundleUri` answers the translation file's URI or undefined,
/// `$fetchBundleContents` the file's text, rejecting as `fileService.readFile` does when it cannot be read.
pub fn serve_localization(method: &str, args: &[Value]) -> Result<Value, Value> {
    match method {
        "$fetchBuiltInBundleUri" => {
            let (Some(id), Some(language)) = (args.first().and_then(Value::as_str), args.get(1).and_then(Value::as_str)) else { return Ok(Value::Null) };
            let path = USER_DATA.get().and_then(|user_data| built_in_bundle_path(user_data, id, language));
            Ok(path.map_or(Value::Null, |path| crate::scan::file_uri(&path)))
        }
        "$fetchBundleContents" => {
            let path = args.first().and_then(|uri| uri["path"].as_str()).unwrap_or_default();
            std::fs::read_to_string(path).map(Value::String).map_err(|error| crate::main_thread::serialized_error(&format!("Unable to read file '{path}' ({error})")))
        }
        _ => Ok(Value::Null),
    }
}

/// `platform.language` and the NLS configuration of this process.
pub fn nls() -> &'static Nls {
    NLS.get_or_init(|| Nls::english("en"))
}

/// `LanguagePacksCache.languagePacksFilePath`.
pub fn language_packs_file(user_data: &Path) -> PathBuf {
    user_data.join("languagepacks.json")
}

/// The cached language packs, `{}` when the file is missing or broken (`withLanguagePacks`).
pub fn read_language_packs(user_data: &Path) -> Map<String, Value> {
    let text = std::fs::read_to_string(language_packs_file(user_data)).unwrap_or_default();
    serde_json::from_str::<Map<String, Value>>(&text).unwrap_or_default()
}

/// `LanguagePacksCache.update`: the packs rebuilt from the installed extensions, written back.
pub fn update_language_packs(user_data: &Path, installed: &[ScannedExtension]) -> anyhow::Result<Map<String, Value>> {
    let packs = language_packs_from(installed);
    std::fs::write(language_packs_file(user_data), serde_json::to_string(&packs)?)?;
    Ok(packs)
}

/// Whether installing or uninstalling `extension` changes the language packs (`postInstallExtension`, `postUninstallExtension`).
pub fn contributes_localizations(extension: &ScannedExtension) -> bool {
    extension.description["contributes"]["localizations"].as_array().is_some_and(|localizations| !localizations.is_empty())
}

/// `createLanguagePacksFromExtensions`.
pub fn language_packs_from(installed: &[ScannedExtension]) -> Map<String, Value> {
    let mut packs = Map::new();
    for extension in installed.iter().filter(|extension| contributes_localizations(extension)) {
        let manifest = &extension.description;
        let identifier = json!({ "id": extension.id, "uuid": manifest["uuid"].as_str().or(manifest["__metadata"]["id"].as_str()) });
        let version = manifest["version"].clone();
        for localization in manifest["contributes"]["localizations"].as_array().into_iter().flatten().filter(|localization| is_valid_localization(localization)) {
            let language = localization["languageId"].as_str().expect("validated");
            let pack = packs.entry(language).or_insert_with(|| {
                let label = localization["localizedLanguageName"].as_str().or(localization["languageName"].as_str());
                json!({ "hash": "", "extensions": [], "translations": {}, "label": label })
            });
            let extensions = pack["extensions"].as_array_mut().expect("created with an array");
            match extensions.iter_mut().find(|entry| entry["extensionIdentifier"]["id"].as_str().is_some_and(|id| id.eq_ignore_ascii_case(&extension.id))) {
                Some(entry) => entry["version"] = version.clone(),
                None => extensions.push(json!({ "extensionIdentifier": identifier, "version": version })),
            }
            for translation in localization["translations"].as_array().into_iter().flatten() {
                let path = extension.location.join(translation["path"].as_str().expect("validated"));
                pack["translations"][translation["id"].as_str().expect("validated")] = json!(path.to_string_lossy());
            }
        }
    }
    for pack in packs.values_mut() {
        let mut md5 = md5::Md5::new();
        for extension in pack["extensions"].as_array().into_iter().flatten() {
            let identifier = &extension["extensionIdentifier"];
            md5.update(identifier["uuid"].as_str().or(identifier["id"].as_str()).unwrap_or_default());
            md5.update(extension["version"].as_str().unwrap_or_default());
        }
        pack["hash"] = json!(format!("{:x}", md5.finalize()));
    }
    packs
}

fn is_valid_localization(localization: &Value) -> bool {
    let translations = localization["translations"].as_array();
    localization["languageId"].is_string()
        && translations.is_some_and(|translations| !translations.is_empty() && translations.iter().all(|translation| translation["id"].is_string() && translation["path"].is_string()))
        && (localization["languageName"].is_null() || localization["languageName"].is_string())
        && (localization["localizedLanguageName"].is_null() || localization["localizedLanguageName"].is_string())
}

/// `resolveNLSConfiguration` for a product build: `en*` and `pseudo` stay English, else the installed pack of the
/// locale or of its shorter prefix (`zh-cn` then `zh`) whose `vscode` translation file exists.
pub fn resolve_nls(user_data: &Path, user_locale: &str) -> Nls {
    if user_locale == "pseudo" || user_locale.starts_with("en") {
        return Nls::english(user_locale);
    }
    let packs = read_language_packs(user_data);
    let Some(language) = resolve_language(&packs, user_locale) else { return Nls::english(user_locale) };
    let translations = packs[&language]["translations"].as_object().cloned().unwrap_or_default();
    let main_exists = translations.get("vscode").and_then(Value::as_str).is_some_and(|path| Path::new(path).is_file());
    if !main_exists {
        return Nls::english(user_locale);
    }
    Nls { user_locale: user_locale.into(), language, translations }
}

/// `resolveLanguagePackLanguage`.
pub fn resolve_language(packs: &Map<String, Value>, locale: &str) -> Option<String> {
    let mut locale = locale;
    loop {
        if packs.contains_key(locale) {
            return Some(locale.to_owned());
        }
        let index = locale.rfind('-').filter(|index| *index > 0)?;
        locale = &locale[..index];
    }
}

/// `getLocalizedMessages`: the messages `%key%` placeholders take, the defaults of package.nls.json overlaid with
/// the language pack's `contents.package` for this id, else with `package.nls.<locale>.json` (trying shorter locales).
pub fn localized_messages(folder: &Path, id: &str, nls: &Nls) -> Option<Map<String, Value>> {
    let read = |path: &Path| std::fs::read(path).ok().and_then(|bytes| serde_json::from_slice::<Map<String, Value>>(&bytes).ok());
    let defaults_path = folder.join("package.nls.json");
    let translation = nls.translations.get(id).and_then(Value::as_str);
    if let Some(translation) = translation {
        let mut messages = read(&defaults_path).unwrap_or_default();
        let bundle = read(Path::new(translation)).unwrap_or_default();
        if let Some(package) = bundle.get("contents").and_then(|contents| contents.get("package")).and_then(Value::as_object) {
            messages.extend(package.clone());
        }
        return Some(messages);
    }
    let mut messages = read(&defaults_path)?;
    let mut locale = nls.language.as_str();
    loop {
        if let Some(localized) = read(&folder.join(format!("package.nls.{locale}.json"))) {
            messages.extend(localized);
            break;
        }
        let Some(index) = locale.rfind('-') else { break };
        locale = &locale[..index];
    }
    Some(messages)
}

/// `MainThreadLocalization.$fetchBuiltInBundleUri`: the active pack's translation file of a built-in extension.
pub fn built_in_bundle_path(user_data: &Path, id: &str, language: &str) -> Option<PathBuf> {
    let packs = read_language_packs(user_data);
    packs.get(language)?["translations"].get(id)?.as_str().map(PathBuf::from)
}

#[cfg(test)]
mod tests;
