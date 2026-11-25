//! Display language (contrib/localization/common/localizationsActions.ts, services/localization/electron-browser/
//! localeService.ts, platform/languagePacks): Configure Display Language picks an installed or Open VSX language pack,
//! writes argv.json `locale` and restarts; Clear Display Language Preference removes it.

use std::path::PathBuf;

use exthost::gallery::{Gallery, GalleryExtension, QueryOptions};
use exthost::language_packs;
use gpui::{Context, PromptLevel, Window};
use serde_json::Value;

use super::Workbench;
use crate::commands::Outcome;
use crate::dialog;
use crate::main_thread_quick_open::Owner;
use crate::notifications::{Notification, Progress, Severity};
use crate::quick_input::{Button, Entry, Event, Item, Kind, QuickInput};

/// An `ILanguagePackItem`: the locale, its label and the pack providing it.
#[derive(Clone, Debug)]
pub struct LanguageItem {
    pub locale: String,
    pub label: String,
    pub extension_id: Option<String>,
    pub gallery: Option<GalleryExtension>,
}

impl Workbench {
    pub(super) fn register_localization_commands(&mut self) {
        self.commands.register("workbench.action.configureLocale", |workbench, _, window, cx| {
            workbench.configure_display_language(window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.clearLocalePreference", |workbench, _, window, cx| {
            if let Err(error) = workbench.write_locale(None) {
                return Outcome::error(error.to_string());
            }
            if language_packs::nls().language != "en" {
                workbench.confirm_restart("English", window, cx);
            }
            Outcome::done()
        });
    }

    fn user_data(&self) -> PathBuf {
        self.settings.lock().unwrap().user_home().parent().expect("the user home is <user data>/User").to_path_buf()
    }

    /// `LanguagePacksCache.update` after an install or uninstall changed the installed extensions.
    pub(super) fn sync_language_packs(&self) {
        let user_data = self.user_data();
        let packs = language_packs::language_packs_from(&self.extensions_viewlet.installed);
        if packs != language_packs::read_language_packs(&user_data)
            && let Err(error) = language_packs::update_language_packs(&user_data, &self.extensions_viewlet.installed)
        {
            eprintln!("[localization] writing languagepacks.json: {error:#}");
        }
    }

    /// `ConfigureDisplayLanguageAction.run`: the installed languages, then those Open VSX offers.
    fn configure_display_language(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let installed = installed_languages(&language_packs::read_language_packs(&self.user_data()));
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some("Select Display Language".into());
        input.match_on_description = true;
        input.busy = true;
        input.set_items(language_entries(&installed, 0, "Installed"));
        let id = self.quick_inputs.add(input, Owner::DisplayLanguage(installed.clone()));
        self.show_quick_input(id, window, cx);
        self.flush_quick_input_events(window, cx);
        let fetch = cx.background_executor().spawn(async move {
            let options = QueryOptions { text: "category:\"language packs\"".into(), page_size: Some(20), page_number: 1, sort_by: None, sort_order: None, include_pre_release: false };
            Gallery::open_vsx(crate::VERSION).query(&options)
        });
        cx.spawn_in(window, async move |workbench, cx| {
            let available = fetch.await.map(|(found, _)| available_languages(&found)).unwrap_or_default();
            let _ = workbench.update_in(cx, |workbench, window, cx| {
                let Some(Owner::DisplayLanguage(items)) = workbench.quick_inputs.owner_mut(id) else { return };
                let new: Vec<LanguageItem> = available.into_iter().filter(|language| !items.iter().any(|installed| installed.locale == language.locale)).collect();
                let mut entries = language_entries(items, 0, "Installed");
                entries.extend(language_entries(&new, items.len(), "Available"));
                items.extend(new);
                let Some(input) = workbench.quick_inputs.get_mut(id) else { return };
                input.set_items(entries);
                input.busy = false;
                workbench.flush_quick_input_events(window, cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub fn display_language_pick_event(&mut self, id: i64, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Owner::DisplayLanguage(items)) = self.quick_inputs.owner(id) else { return };
        match event {
            Event::Accept(_) => {
                let Some(item) = self.quick_inputs.get(id).and_then(|input| input.active_handles().first().copied()).map(|handle| items[handle as usize].clone()) else { return };
                self.hide_quick_input_by_id(id, window, cx);
                self.set_locale(item, window, cx);
            }
            Event::TriggerItemButton(handle, ..) => {
                let extension_id = items[handle as usize].extension_id.clone();
                self.hide_quick_input_by_id(id, window, cx);
                if let Some(extension_id) = extension_id {
                    self.search_extensions(&format!("@id:{extension_id}"), window, cx);
                }
            }
            Event::Hide => self.quick_inputs.remove(id),
            _ => {}
        }
    }

    /// `NativeLocaleService.setLocale`: a pack not installed yet is installed first (Microsoft's) or shown in the
    /// Extensions view (anyone else's); then the restart dialog, argv.json and the restart.
    fn set_locale(&mut self, item: LanguageItem, window: &mut Window, cx: &mut Context<Self>) {
        let current = language_packs::nls().language.as_str();
        if item.locale == current {
            return;
        }
        let installed = installed_languages(&language_packs::read_language_packs(&self.user_data()));
        if installed.iter().any(|language| language.locale == item.locale) {
            return self.confirm_restart_to(item, window, cx);
        }
        let Some(gallery) = item.gallery.clone() else { return };
        if !gallery.publisher.eq_ignore_ascii_case("ms-ceintl") {
            return self.search_extensions(&format!("@id:{}", gallery.id), window, cx);
        }
        let mut progress = Notification::new(Severity::Info, format!("Installing {} language support...", item.label));
        progress.progress = Some(Progress { infinite: true, ..Progress::default() });
        let key = self.notify(progress);
        let installed = self.install_from_gallery(gallery.id.clone(), None, false, cx);
        cx.spawn_in(window, async move |workbench, cx| {
            let result = match installed {
                Outcome::Ready(result) => result,
                Outcome::Pending(pending) => pending.await.unwrap_or(Ok(Value::Null)),
            };
            let _ = workbench.update_in(cx, |workbench, window, cx| {
                if let Some(key) = key {
                    workbench.close_notification(key, window, cx);
                }
                if result.is_ok() {
                    workbench.confirm_restart_to(item, window, cx);
                }
            });
        })
        .detach();
    }

    fn confirm_restart_to(&mut self, item: LanguageItem, window: &mut Window, cx: &mut Context<Self>) {
        let answer = self.restart_dialog(&item.label, window, cx);
        cx.spawn_in(window, async move |workbench, cx| {
            if answer.await.map(|(index, _)| index) != Ok(0) {
                return;
            }
            let _ = workbench.update_in(cx, |workbench, _, cx| match workbench.write_locale(Some(&item.locale)) {
                Ok(()) => relaunch(cx),
                Err(error) => workbench.notify_error(error.to_string()),
            });
        })
        .detach();
    }

    /// `clearLocalePreference`: the dialog restarts into English; argv.json already lost its `locale`.
    fn confirm_restart(&mut self, language: &str, window: &mut Window, cx: &mut Context<Self>) {
        let answer = self.restart_dialog(language, window, cx);
        cx.spawn_in(window, async move |workbench, cx| {
            if answer.await.map(|(index, _)| index) == Ok(0) {
                let _ = workbench.update(cx, |_, cx| relaunch(cx));
            }
        })
        .detach();
    }

    /// `showRestartDialog`.
    fn restart_dialog(&self, language: &str, window: &mut Window, cx: &mut Context<Self>) -> futures::channel::oneshot::Receiver<(usize, bool)> {
        let message = format!("Restart VSRustium to switch to {language}?");
        let detail = format!("To change the display language to {language}, VSRustium needs to restart.");
        dialog::show_message_box(window, PromptLevel::Info, &message, Some(&detail), &["Restart", "Cancel"], None, cx)
    }

    /// `writeLocaleValue` into argv.json.
    fn write_locale(&self, locale: Option<&str>) -> std::io::Result<()> {
        let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        let argv_file = crate::output::log_levels::argv_file(&home);
        let configuration = self.settings.lock().unwrap().configuration.clone();
        crate::nls::write_argv_locale(&argv_file, locale, |content| config::service::formatting_options(content, &configuration, &argv_file))
    }
}

/// `NativeLanguagePackService.getInstalledLanguages`: each pack and English, by label.
fn installed_languages(packs: &serde_json::Map<String, Value>) -> Vec<LanguageItem> {
    let mut languages: Vec<LanguageItem> = packs
        .iter()
        .map(|(locale, pack)| LanguageItem {
            locale: locale.clone(),
            label: pack["label"].as_str().unwrap_or(locale).to_owned(),
            extension_id: pack["extensions"][0]["extensionIdentifier"]["id"].as_str().map(str::to_owned),
            gallery: None,
        })
        .collect();
    languages.push(english());
    languages.sort_by(|a, b| crate::command_palette::locale_compare(&a.label, &b.label));
    languages
}

fn english() -> LanguageItem {
    LanguageItem { locale: "en".into(), label: "English".into(), extension_id: None, gallery: None }
}

/// `getAvailableLanguages`: the gallery's packs with a localized language and an `lp-` tag, then English.
fn available_languages(found: &[GalleryExtension]) -> Vec<LanguageItem> {
    let mut languages: Vec<LanguageItem> = found
        .iter()
        .filter(|pack| !pack.localized_languages.is_empty())
        .filter_map(|pack| {
            let locale = pack.tags.iter().find_map(|tag| tag.strip_prefix("lp-"))?;
            Some(LanguageItem { locale: locale.to_owned(), label: pack.localized_languages[0].clone(), extension_id: Some(pack.id.clone()), gallery: Some(pack.clone()) })
        })
        .collect();
    languages.push(english());
    languages
}

/// `createQuickPickItem` and `withMoreInfoButton` under a separator; handles index the owner's item list.
fn language_entries(items: &[LanguageItem], first_handle: usize, separator: &str) -> Vec<Entry> {
    if items.is_empty() {
        return Vec::new();
    }
    let current = language_packs::nls().language.as_str();
    let mut entries = vec![Entry::Separator(Some(separator.into()))];
    for (index, item) in items.iter().enumerate() {
        let mut description = (item.label != item.locale).then(|| format!("({})", item.locale));
        if item.locale.eq_ignore_ascii_case(current) {
            *description.get_or_insert_default() += " (Current)";
        }
        if let Some(count) = item.gallery.as_ref().map(|gallery| gallery.install_count).filter(|count| *count > 0.) {
            *description.get_or_insert_default() += &format!(" $(cloud-download) {}", install_count_label(count));
        }
        let buttons = match &item.extension_id {
            Some(_) => vec![Button { handle: 0, codicon: Some("info".into()), tooltip: Some("More Info".into()), ..Button::default() }],
            None => Vec::new(),
        };
        entries.push(Entry::Item(Item { handle: (first_handle + index) as i64, label: item.label.clone(), description, buttons, pickable: true, ..Item::default() }));
    }
    entries
}

/// languagePacks.ts: `1.2M`, `34K` or the count.
fn install_count_label(count: f64) -> String {
    if count > 1_000_000. {
        format!("{}M", (count / 100_000.).floor() / 10.)
    } else if count > 1000. {
        format!("{}K", (count / 1000.).floor())
    } else {
        format!("{count}")
    }
}

/// `hostService.restart`: the same binary, arguments and working folder start once this process has exited.
// shortcut: gpui's `App::restart` relaunches with `open`, which loses the working folder of an unbundled binary;
// milestone 19's packaged `.app` can switch to it.
fn relaunch(cx: &mut Context<Workbench>) {
    use std::os::unix::process::CommandExt as _;
    let script = r#"while kill -0 "$0" 2>/dev/null; do sleep 0.1; done; cd "$1"; shift; exec "$@""#;
    let exe = std::env::current_exe().expect("the running binary");
    let cwd = std::env::current_dir().expect("the working folder");
    let spawned = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(script)
        .arg(std::process::id().to_string())
        .arg(cwd)
        .arg(exe)
        .args(std::env::args_os().skip(1))
        .process_group(0)
        .spawn();
    match spawned {
        Ok(_) => cx.quit(),
        Err(error) => eprintln!("[localization] restarting: {error}"),
    }
}

#[cfg(test)]
mod tests;
