//! The display language (mainImpl.ts, base/node/nls.ts): the locale from `--locale`, argv.json `locale` or the OS,
//! the language pack it resolves to, and the workbench strings that pack translates. Upstream looks each message up by
//! module and key; the port's strings are English literals, so the index of src/vs `localize` calls (build.rs) maps
//! an English message to its module and key, and the pack's `vscode` translation file gives the text.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

use config::json::{self, ParseOptions, Segment};
use config::json_edit::{FormattingOptions, apply_edits, set_property};
use exthost::language_packs::Nls;
use serde_json::Value;

const INDEX: &str = include_str!(concat!(env!("OUT_DIR"), "/nls_index.tsv"));

static TRANSLATIONS: OnceLock<HashMap<String, String>> = OnceLock::new();

/// The window's translations: the language pack's main translation file read through the index.
pub fn init(nls: &Nls) {
    let main = nls.translations.get("vscode").and_then(Value::as_str);
    let Some(text) = main.and_then(|path| std::fs::read_to_string(path).ok()) else { return };
    let Ok(pack) = serde_json::from_str::<Value>(&text) else { return };
    let _ = TRANSLATIONS.set(translations_from(INDEX, &pack["contents"]));
}

/// English message to translation; the first module translating a message wins.
// shortcut: a message two modules translate differently gets the first module's text; upstream knows the module.
fn translations_from(index: &str, contents: &Value) -> HashMap<String, String> {
    let mut translations = HashMap::new();
    for line in index.lines() {
        let mut fields = line.splitn(3, '\t');
        let (Some(module), Some(key), Some(message)) = (fields.next(), fields.next(), fields.next()) else { continue };
        let Some(translated) = contents[module][key].as_str().filter(|translated| !translated.is_empty()) else { continue };
        translations.entry(unescape(message)).or_insert_with(|| translated.to_owned());
    }
    translations
}

/// A TypeScript string literal's body as text.
fn unescape(literal: &str) -> String {
    let mut text = String::with_capacity(literal.len());
    let mut chars = literal.chars();
    while let Some(char) = chars.next() {
        if char != '\\' {
            text.push(char);
            continue;
        }
        match chars.next() {
            Some('n') => text.push('\n'),
            Some('t') => text.push('\t'),
            Some('r') => text.push('\r'),
            Some('u') => {
                let hex: String = chars.by_ref().take(4).collect();
                text.extend(u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32));
            }
            Some(other) => text.push(other),
            None => {}
        }
    }
    text
}

/// `localize`: the active language pack's text for an English workbench message.
pub fn localize(english: &str) -> Cow<'_, str> {
    match TRANSLATIONS.get().and_then(|translations| translations.get(english)) {
        Some(translated) => Cow::Owned(translated.clone()),
        None => Cow::Borrowed(english),
    }
}

/// `getUserDefinedLocale` then `app.getLocale()`: `--locale`, argv.json `locale`, else the OS's first preferred
/// language, lowercased; Chinese becomes zh-cn or zh-tw (`processZhLocale`).
pub fn user_locale(locale_flag: Option<&str>, argv_file: &Path) -> String {
    let argv_locale = || {
        let content = std::fs::read_to_string(argv_file).ok()?;
        let argv = json::parse(&content, &mut Vec::new(), ParseOptions::default())?;
        argv["locale"].as_str().map(str::to_owned)
    };
    match locale_flag.map(str::to_owned).or_else(argv_locale) {
        Some(locale) => locale.to_lowercase(),
        None => process_zh_locale(&os_locale().to_lowercase()),
    }
}

fn os_locale() -> String {
    let languages = objc2_foundation::NSLocale::preferredLanguages();
    languages.firstObject().map_or_else(|| "en".to_owned(), |language| language.to_string())
}

pub fn process_zh_locale(locale: &str) -> String {
    if !locale.starts_with("zh") {
        return locale.to_owned();
    }
    match locale.split('-').nth(1) {
        Some("hans" | "cn" | "sg" | "my") => "zh-cn".into(),
        _ => "zh-tw".into(),
    }
}

/// localeService.ts `writeLocaleValue`: argv.json `locale` set, or removed for `None`.
pub fn write_argv_locale(argv_file: &Path, locale: Option<&str>, options: impl FnOnce(&str) -> FormattingOptions) -> std::io::Result<()> {
    let content = std::fs::read_to_string(argv_file).unwrap_or_default();
    let value = locale.map(Value::from);
    let edits = set_property(&content, &[Segment::Key("locale".into())], value.as_ref(), &options(&content));
    if let Some(folder) = argv_file.parent() {
        std::fs::create_dir_all(folder)?;
    }
    std::fs::write(argv_file, apply_edits(&content, &edits))
}

#[cfg(test)]
mod tests;
