//! untitledTextEditor.test.ts 'Untitled text editors' on the workbench's untitled models; the extension host's
//! messages stand in for the service and model events.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use exthost::scan::scan_extension;
use gpui::{TestAppContext, VisualTestContext};
use serde_json::{Value, json};

use crate::host_sync::{HostMessage, HostSync};
use crate::languages::Languages;
use crate::theme::load_theme;
use crate::workbench::Workbench;

struct Untitled<'a> {
    dir: PathBuf,
    workbench: gpui::Entity<Workbench>,
    cx: &'a mut VisualTestContext,
    host: Receiver<HostMessage>,
}

/// A workbench knowing TypeScript and JavaScript over a temp folder whose user settings are `user_settings`.
fn untitled<'a>(name: &str, user_settings: &str, cx: &'a mut TestAppContext) -> Untitled<'a> {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let languages = ["typescript-basics", "javascript"].map(|name| scan_extension(&app_root.join("extensions").join(name), true).unwrap());
    let dir = std::env::temp_dir().join(format!("vsrustium-untitled-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("User")).unwrap();
    std::fs::write(dir.join("User/settings.json"), user_settings).unwrap();
    let settings = config::service::Settings::load(config::registry::Registry::with_extensions(&[]), dir.join("User"), None);
    let (sender, host) = std::sync::mpsc::channel();
    let (workbench, cx) = cx.add_window_view(|_, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&languages), Arc::new(Mutex::new(settings)), HostSync::new(sender), cx);
        workbench.skip_dialogs = true;
        workbench
    });
    Untitled { dir, workbench, cx, host }
}

impl Untitled<'_> {
    fn run(&mut self, id: &str, args: Vec<Value>) {
        self.workbench.update_in(self.cx, |workbench, window, cx| drop(workbench.execute_command(id, args, window, cx)));
        self.cx.run_until_parked();
    }

    fn new_untitled_file(&mut self, language: Option<&str>) {
        let args = language.map(|language| vec![json!({ "languageId": language })]).unwrap_or_default();
        self.run("workbench.action.files.newUntitledFile", args);
    }

    fn host_request(&mut self, proxy: &str, method: &str, args: &[Value]) -> Value {
        self.workbench.update_in(self.cx, |workbench, window, cx| workbench.host_request(proxy, method, args, window, cx)).unwrap()
    }

    /// `untitledTextEditorService.create({ initialValue })` as `$tryCreateDocument` asks for it.
    fn create(&mut self, options: Value) -> PathBuf {
        let uri = self.host_request("MainThreadDocuments", "$tryCreateDocument", &[options]);
        PathBuf::from(uri["path"].as_str().unwrap())
    }

    /// `create({ associatedResource })`: an untitled model for a file path.
    fn create_associated(&mut self, file: &str) -> PathBuf {
        let path = self.dir.join(file);
        self.host_request("MainThreadDocuments", "$tryOpenDocument", &[json!({ "scheme": "untitled", "path": path.to_string_lossy() })]);
        path
    }

    /// `textEditorModel.setValue`, as `modelService.updateModel` applies it.
    fn set_value(&mut self, path: &str, text: &str) {
        self.workbench.update(self.cx, |workbench, _| workbench.update_model("untitled", Path::new(path), text, false));
    }

    fn read<T>(&mut self, read: impl FnOnce(&Workbench) -> T) -> T {
        self.workbench.read_with(self.cx, |workbench, _| read(workbench))
    }

    fn exists(&mut self, path: &str) -> bool {
        self.read(|workbench| workbench.editors.model("untitled", Path::new(path)).is_some())
    }

    fn is_dirty(&mut self, path: impl AsRef<Path>) -> bool {
        self.read(|workbench| workbench.editors.model("untitled", path.as_ref()).unwrap().is_dirty())
    }

    fn dirty_count(&mut self) -> usize {
        self.read(|workbench| workbench.editors.editors.iter().chain(&workbench.editors.models).filter(|model| model.is_dirty()).count())
    }

    fn language(&mut self, path: &str) -> String {
        self.read(|workbench| workbench.editors.model("untitled", Path::new(path)).unwrap().document().language_id.clone())
    }

    fn name(&mut self, path: impl AsRef<Path>) -> String {
        self.read(|workbench| workbench.editor_name(workbench.editors.model("untitled", path.as_ref()).unwrap()))
    }

    fn explicit_language(&mut self, path: &str) -> bool {
        self.read(|workbench| workbench.text_files.preferred_languages.contains(&("untitled".to_string(), PathBuf::from(path))))
    }

    fn set_encoding(&mut self, path: &str, encoding: &str) {
        let uri = json!({ "scheme": "untitled", "path": path });
        self.workbench.update(self.cx, |workbench, cx| workbench.set_model_encoding(&uri, "untitled", Path::new(path), encoding, cx)).unwrap();
    }

    /// The methods of the host messages since the last call.
    fn host_messages(&mut self) -> Vec<&'static str> {
        self.host.try_iter().map(|message| message.method).collect()
    }

    fn count(&mut self, method: &str) -> usize {
        self.host_messages().iter().filter(|sent| **sent == method).count()
    }

    /// The tab labels the host got since the last call, each new one once (`onDidChangeLabel`).
    fn new_tab_labels(&mut self) -> Vec<String> {
        let mut labels: Vec<String> = Vec::new();
        for message in self.host.try_iter().filter(|message| message.method == "$acceptTabOperation" && message.args[0]["kind"] == 2) {
            let label = message.args[0]["tabDto"]["label"].as_str().unwrap().to_string();
            if labels.last() != Some(&label) {
                labels.push(label);
            }
        }
        labels
    }
}

impl Drop for Untitled<'_> {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[gpui::test]
fn new_untitled_models_are_clean_until_edited_and_a_revert_disposes_them(cx: &mut TestAppContext) {
    let mut untitled = untitled("basics", "{}", cx);
    untitled.new_untitled_file(None);
    untitled.new_untitled_file(None);
    assert!(untitled.exists("Untitled-1") && untitled.exists("Untitled-2"));
    assert!(untitled.read(|workbench| workbench.editors.model("file", Path::new("testing")).is_none()));
    assert!(untitled.read(|workbench| !workbench.editors.model("untitled", Path::new("Untitled-1")).unwrap().text_model().associated_file));
    assert!(!untitled.is_dirty("Untitled-2"));
    untitled.host_messages();
    untitled.set_value("Untitled-2", "foo bar");
    assert!(untitled.is_dirty("Untitled-2"));
    assert!(untitled.host_messages().contains(&"$acceptDirtyStateChanged"));
    assert_eq!(untitled.dirty_count(), 1);
    untitled.run("workbench.action.files.revert", Vec::new());
    assert!(!untitled.exists("Untitled-2"), "the revert disposed the dirty model");
    assert_eq!(untitled.dirty_count(), 0);
    untitled.run("workbench.action.files.revert", Vec::new());
    assert!(!untitled.exists("Untitled-1"), "the revert disposed the clean model");
}

#[gpui::test]
fn an_untitled_model_with_an_associated_file_is_dirty_from_the_start(cx: &mut TestAppContext) {
    let mut untitled = untitled("associated-dirty", "{}", cx);
    let path = untitled.create_associated("file.txt");
    assert!(untitled.read(|workbench| workbench.editors.model("untitled", &path).unwrap().text_model().associated_file));
    assert!(untitled.is_dirty(&path));
    assert!(untitled.host_messages().contains(&"$acceptDirtyStateChanged"));
}

#[gpui::test]
fn an_untitled_model_emptied_again_is_clean(cx: &mut TestAppContext) {
    let mut untitled = untitled("emptied", "{}", cx);
    untitled.new_untitled_file(None);
    untitled.set_value("Untitled-1", "foo bar");
    assert!(untitled.is_dirty("Untitled-1"));
    untitled.set_value("Untitled-1", "");
    assert!(!untitled.is_dirty("Untitled-1"));
}

#[gpui::test]
fn create_options_set_the_initial_value_resource_and_associated_file(cx: &mut TestAppContext) {
    let mut untitled = untitled("create-options", "{}", cx);
    let first = untitled.create(json!({}));
    let first = first.to_str().unwrap().to_string();
    untitled.set_value(&first, "foo bar");
    assert!(untitled.is_dirty(&first));
    untitled.set_value(&first, "");
    assert!(!untitled.is_dirty(&first));
    let second = untitled.create(json!({ "content": "Hello World" }));
    assert_eq!(untitled.read(|workbench| workbench.editors.model("untitled", &second).unwrap().document().lines.clone()), ["Hello World"]);
    let models = untitled.read(|workbench| workbench.editors.models.len());
    let uri = untitled.host_request("MainThreadDocuments", "$tryOpenDocument", &[json!({ "scheme": "untitled", "path": first })]);
    assert_eq!((uri["path"].as_str(), untitled.read(|workbench| workbench.editors.models.len())), (Some(first.as_str()), models), "an existing untitled resource gives its model");
    let associated = untitled.create_associated("file44.txt");
    assert!(untitled.read(|workbench| workbench.editors.model("untitled", &associated).unwrap().text_model().associated_file));
    assert!(untitled.is_dirty(&associated));
}

#[gpui::test]
fn an_untitled_model_with_an_associated_file_stays_dirty_when_emptied(cx: &mut TestAppContext) {
    let mut untitled = untitled("associated-emptied", "{}", cx);
    let path = untitled.create_associated("file.txt");
    let path = path.to_str().unwrap().to_string();
    untitled.set_value(&path, "foo bar");
    assert!(untitled.is_dirty(&path));
    untitled.set_value(&path, "");
    assert!(untitled.is_dirty(&path));
}

#[gpui::test]
fn an_untitled_model_with_an_initial_value_is_dirty(cx: &mut TestAppContext) {
    let mut untitled = untitled("initial-value", "{}", cx);
    let path = untitled.create(json!({ "content": "Hello World" }));
    assert!(untitled.is_dirty(&path));
}

#[gpui::test]
fn new_untitled_models_take_the_files_default_language(cx: &mut TestAppContext) {
    let mut untitled = untitled("default-language", r#"{ "files.defaultLanguage": "javascript" }"#, cx);
    untitled.new_untitled_file(None);
    assert_eq!(untitled.language("Untitled-1"), "javascript");
    let created = untitled.create(json!({}));
    assert_eq!(untitled.language(created.to_str().unwrap()), "javascript", "$tryCreateDocument goes through the same service");
    let associated = untitled.create_associated("file.foo");
    assert_eq!(untitled.language(associated.to_str().unwrap()), "plaintext", "an associated file keeps the language of its path");
}

#[gpui::test]
fn the_files_default_language_active_editor_language_takes_the_active_editors_language(cx: &mut TestAppContext) {
    let mut untitled = untitled("active-editor-language", r#"{ "files.defaultLanguage": "${activeEditorLanguage}" }"#, cx);
    untitled.new_untitled_file(Some("typescript"));
    untitled.new_untitled_file(None);
    assert_eq!(untitled.language("Untitled-2"), "typescript");
}

#[gpui::test]
fn a_language_given_on_create_wins_over_the_files_default_language(cx: &mut TestAppContext) {
    let mut untitled = untitled("language-over-default", r#"{ "files.defaultLanguage": "javascript" }"#, cx);
    untitled.new_untitled_file(Some("typescript"));
    assert_eq!(untitled.language("Untitled-1"), "typescript");
}

#[gpui::test]
fn the_language_of_an_untitled_model_can_change_afterwards(cx: &mut TestAppContext) {
    let mut untitled = untitled("change-language", "{}", cx);
    untitled.new_untitled_file(Some("typescript"));
    assert_eq!(untitled.language("Untitled-1"), "typescript");
    untitled.workbench.update(untitled.cx, |workbench, cx| workbench.set_active_language("plaintext", cx));
    assert_eq!(untitled.language("Untitled-1"), "plaintext");
}

#[gpui::test]
fn a_picked_language_is_remembered_as_set_explicitly(cx: &mut TestAppContext) {
    let mut untitled = untitled("explicit-language", "{}", cx);
    untitled.new_untitled_file(None);
    assert!(!untitled.explicit_language("Untitled-1"));
    untitled.workbench.update(untitled.cx, |workbench, cx| workbench.set_active_language("plaintext", cx));
    assert!(untitled.explicit_language("Untitled-1"));
    assert_eq!(untitled.language("Untitled-1"), "plaintext");
}

#[gpui::test]
fn a_language_set_by_an_extension_is_remembered_as_set_explicitly(cx: &mut TestAppContext) {
    // #159202: a language set on the model, not through the input, counts as explicit too.
    let mut untitled = untitled("extension-language", "{}", cx);
    untitled.new_untitled_file(None);
    assert!(!untitled.explicit_language("Untitled-1"));
    untitled.host_request("MainThreadLanguages", "$changeLanguage", &[json!({ "scheme": "untitled", "path": "Untitled-1" }), json!("typescript")]);
    assert!(untitled.explicit_language("Untitled-1"));
    assert_eq!(untitled.language("Untitled-1"), "typescript");
}

#[gpui::test]
fn setting_an_untitled_models_encoding_tells_the_host_once(cx: &mut TestAppContext) {
    let mut untitled = untitled("service-encoding", "{}", cx);
    untitled.new_untitled_file(None);
    untitled.host_messages();
    untitled.set_encoding("Untitled-1", "utf16le");
    assert_eq!(untitled.count("$acceptEncodingChanged"), 1);
    assert_eq!(untitled.read(|workbench| workbench.editors.model("untitled", Path::new("Untitled-1")).unwrap().document().encoding.clone()), "utf16le");
}

#[gpui::test]
fn a_first_line_edit_relabels_the_untitled_tab(cx: &mut TestAppContext) {
    let mut untitled = untitled("service-label", "{}", cx);
    untitled.new_untitled_file(None);
    untitled.host_messages();
    untitled.set_value("Untitled-1", "Foo Bar");
    assert_eq!(untitled.new_tab_labels(), ["Foo Bar"]);
}

#[gpui::test]
fn closing_a_clean_untitled_editor_disposes_its_model(cx: &mut TestAppContext) {
    let mut untitled = untitled("will-dispose", "{}", cx);
    untitled.new_untitled_file(None);
    assert!(untitled.exists("Untitled-1"));
    untitled.run("workbench.action.closeActiveEditor", Vec::new());
    assert!(!untitled.exists("Untitled-1"));
}

#[gpui::test]
fn the_value_of_an_untitled_model_is_its_text(cx: &mut TestAppContext) {
    let mut untitled = untitled("get-value", "{}", cx);
    untitled.new_untitled_file(None);
    untitled.set_value("Untitled-1", "foo bar");
    assert_eq!(untitled.read(|workbench| workbench.editors.model("untitled", Path::new("Untitled-1")).unwrap().document().lines.join("\n")), "foo bar");
    assert!(untitled.read(|workbench| workbench.editors.model("https", Path::new("/www.microsoft.com")).is_none()));
}

#[gpui::test]
fn every_set_value_of_an_untitled_model_is_a_content_change(cx: &mut TestAppContext) {
    let mut untitled = untitled("content", "{}", cx);
    untitled.new_untitled_file(None);
    untitled.host_messages();
    for value in ["foo", "bar", "", "foo"] {
        untitled.set_value("Untitled-1", value);
    }
    assert_eq!(untitled.count("$acceptModelChanged"), 4);
}

#[gpui::test]
fn a_revert_of_an_untitled_model_disposes_its_input(cx: &mut TestAppContext) {
    let mut untitled = untitled("revert", "{}", cx);
    untitled.new_untitled_file(None);
    untitled.set_value("Untitled-1", "foo");
    untitled.run("workbench.action.files.revert", Vec::new());
    assert!(untitled.read(|workbench| workbench.editors.editors.is_empty()));
    assert!(!untitled.exists("Untitled-1"));
}

#[gpui::test]
fn the_name_of_an_untitled_model_follows_its_first_line(cx: &mut TestAppContext) {
    let mut untitled = untitled("name", "{}", cx);
    untitled.new_untitled_file(None);
    untitled.host_messages();
    let mut renames = Vec::new();
    let cases = [
        ("foo", "foo"),
        ("bar", "bar"),
        ("", "Untitled-1"),
        ("        ", "Untitled-1"),
        ("([]}", "Untitled-1"),
        ("([]}hello   ", "([]}hello"),
        ("12345678901234567890123456789012345678901234567890", "1234567890123456789012345678901234567890"),
        ("123456789012345678901234567890123456789\u{1F31E}", "123456789012345678901234567890123456789"),
        ("hello\u{202E}world", "helloworld"),
    ];
    for (value, name) in cases {
        untitled.set_value("Untitled-1", value);
        assert_eq!(untitled.name("Untitled-1"), name, "{value:?}");
        for label in untitled.new_tab_labels() {
            if renames.last() != Some(&label) {
                renames.push(label);
            }
        }
    }
    assert_eq!(renames.len(), 7, "{renames:?}");
    untitled.set_value("Untitled-1", "Hello\nWorld");
    assert_eq!(untitled.new_tab_labels(), ["Hello"]);
    untitled.set_value("Untitled-1", "Hello\nWhelloorld");
    assert!(untitled.new_tab_labels().is_empty(), "the change was not on the first line");
    let with_contents = untitled.create(json!({ "content": "Foo" }));
    assert_eq!(untitled.name(&with_contents), "Foo");
}

#[gpui::test]
fn an_untitled_model_tells_the_host_its_dirty_state_once(cx: &mut TestAppContext) {
    let mut untitled = untitled("dirty", "{}", cx);
    untitled.new_untitled_file(None);
    untitled.host_messages();
    untitled.set_value("Untitled-1", "foo");
    assert_eq!(untitled.count("$acceptDirtyStateChanged"), 1);
    untitled.set_value("Untitled-1", "bar");
    assert_eq!(untitled.count("$acceptDirtyStateChanged"), 0, "another change does not fire");
}

#[gpui::test]
fn setting_the_same_encoding_again_changes_nothing(cx: &mut TestAppContext) {
    let mut untitled = untitled("model-encoding", "{}", cx);
    untitled.new_untitled_file(None);
    untitled.host_messages();
    untitled.set_encoding("Untitled-1", "utf16le");
    assert_eq!(untitled.count("$acceptEncodingChanged"), 1);
    untitled.set_encoding("Untitled-1", "utf16le");
    assert_eq!(untitled.count("$acceptEncodingChanged"), 0);
}

#[gpui::test]
fn a_dirty_untitled_model_is_kept_until_it_is_clean(cx: &mut TestAppContext) {
    let mut untitled = untitled("can-dispose", "{}", cx);
    untitled.new_untitled_file(None);
    untitled.set_value("Untitled-1", "foo");
    untitled.workbench.update(untitled.cx, |workbench, _| {
        workbench.editors.close(0);
        workbench.editors.release_models(Instant::now());
    });
    assert!(untitled.exists("Untitled-1"), "canDispose waits while the model is dirty");
    untitled.set_value("Untitled-1", "");
    untitled.workbench.update(untitled.cx, |workbench, _| workbench.editors.release_models(Instant::now()));
    assert!(!untitled.exists("Untitled-1"));
}
