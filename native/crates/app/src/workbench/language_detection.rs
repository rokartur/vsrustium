//! languageDetectionWorkerServiceImpl.ts `LanguageDetectionService` with its worker in Bun, textEditorModel.ts
//! `autoDetectLanguage`, and languageDetection.contribution.ts: the status bar's "Accept Detected Language" light bulb
//! and `editor.detectLanguage` (Shift+Alt+D).
// shortcut: only edits of the active editor and the active model's change trigger the automatic detection; upstream
// listens to every untitled model's content.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::channel::oneshot;
use gpui::{AnyElement, Context, Rgba, Task, div, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use crate::commands::Outcome;
use crate::language_detection::{detected_languages, text_for_detection};

/// textEditorModel.ts `AUTO_DETECT_LANGUAGE_THROTTLE_DELAY`.
const AUTO_DETECT_DELAY: Duration = Duration::from_millis(600);
/// `LanguageDetectionStatusContribution._delayer`.
const STATUS_DELAY: Duration = Duration::from_millis(1000);
const DETECT_COMMAND: &str = "editor.detectLanguage";

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Vec<(String, f64)>>>>>;

#[derive(Default)]
pub(super) struct LanguageDetection {
    worker: Option<Worker>,
    /// `_loadFailed`: the worker could not start, so detection answers nothing from now on.
    failed: bool,
    auto_detect: Option<Task<()>>,
    status: Option<Task<()>>,
    /// The detected language the status bar offers for the active untitled model.
    offered: Option<String>,
}

pub(super) enum Detection {
    Language(String),
    Model(Vec<String>),
}

struct Worker {
    stdin: ChildStdin,
    pending: Pending,
    next_id: u64,
}

impl Workbench {
    pub(super) fn register_language_detection(&mut self) {
        self.commands.register(DETECT_COMMAND, |workbench, _, window, cx| {
            let Some(editor) = workbench.editors.active() else { return Outcome::done() };
            let (scheme, path) = (editor.document().scheme.clone(), editor.document().path.clone());
            let detection = workbench.detect_language(&scheme, &path);
            cx.spawn_in(window, async move |workbench, cx| {
                let detection = detection.await;
                let _ = workbench.update(cx, |workbench, cx| match workbench.detected_language(detection) {
                    Some(language) if workbench.editors.model(&scheme, &path).is_some() => workbench.set_model_language(&scheme, &path, &language, cx),
                    Some(_) => {}
                    None => workbench.notify_warning("Unable to detect editor language"),
                });
            })
            .detach();
            Outcome::done()
        });
    }

    /// `LanguageDetectionWorkerClient.detectLanguage`: the language of the file name, else the model's languages
    /// best first, for `detected_language`.
    pub(super) fn detect_language(&mut self, scheme: &str, path: &Path) -> impl std::future::Future<Output = Detection> + use<> {
        // `_guessLanguageIdByUri`: by the resource only, without the first line.
        let quick_guess = self.languages.guess_language(path, "").map(str::to_owned);
        let lines = self.editors.model(scheme, path).filter(|_| quick_guess.is_none()).map(|model| model.document().lines.clone());
        let request = lines.and_then(|lines| self.run_language_model(text_for_detection(&lines)));
        async move {
            match (quick_guess, request) {
                (Some(language), _) => Detection::Language(language),
                (None, Some(request)) => Detection::Model(request.await.map(|results| detected_languages(&results)).unwrap_or_default()),
                (None, None) => Detection::Model(Vec::new()),
            }
        }
    }

    /// `getLanguageId` of the first model language that maps to one: a registered id, else the language of `file.<id>`.
    pub(super) fn detected_language(&self, detection: Detection) -> Option<String> {
        let model_ids = match detection {
            Detection::Language(language) => return Some(language),
            Detection::Model(model_ids) => model_ids,
        };
        for model_id in model_ids {
            if self.languages.is_registered(&model_id) {
                return Some(model_id);
            }
            if let Some(guessed) = self.languages.guess_language(Path::new(&format!("file.{model_id}")), "") {
                return Some(guessed.to_owned());
            }
        }
        None
    }

    /// `ModelOperations.runModel` in the Bun worker, started on first use.
    fn run_language_model(&mut self, content: String) -> Option<oneshot::Receiver<Vec<(String, f64)>>> {
        if self.language_detection.failed {
            return None;
        }
        if self.language_detection.worker.is_none() {
            match start_worker(&self.app_root) {
                Ok(worker) => self.language_detection.worker = Some(worker),
                Err(error) => {
                    eprintln!("language detection: {error}");
                    self.language_detection.failed = true;
                    return None;
                }
            }
        }
        let worker = self.language_detection.worker.as_mut().expect("started above");
        let id = worker.next_id;
        worker.next_id += 1;
        let (sender, receiver) = oneshot::channel();
        worker.pending.lock().unwrap().insert(id, sender);
        let line = json!({ "id": id, "content": content }).to_string() + "\n";
        if worker.stdin.write_all(line.as_bytes()).is_err() {
            self.language_detection.worker = None;
            self.language_detection.failed = true;
            return None;
        }
        Some(receiver)
    }

    /// The active model changed or was edited: the throttled `autoDetectLanguage`, and the status bar's offer.
    pub(super) fn language_detection_model_changed(&mut self, cx: &mut Context<Self>) {
        self.language_detection.offered = None;
        self.schedule_auto_detect(cx);
        self.schedule_detection_status(cx);
    }

    /// An edit of the active model detects again and updates the offer, which stays until the answer.
    pub(super) fn language_detection_content_changed(&mut self, cx: &mut Context<Self>) {
        self.schedule_auto_detect(cx);
        self.schedule_detection_status(cx);
    }

    /// `doAutoDetectLanguage` for the active model: untitled models, and files without an extension in plain text,
    /// unless the user or an extension picked the language or `workbench.editor.languageDetection` is off for it.
    fn schedule_auto_detect(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        let document = editor.document();
        let (scheme, path, language) = (document.scheme.clone(), document.path.clone(), document.language_id.clone());
        drop(document);
        let eligible = match scheme.as_str() {
            "untitled" => true,
            "file" => language == crate::languages::PLAINTEXT && path.extension().is_none(),
            _ => false,
        };
        if !eligible || self.text_files.preferred_languages.contains(&(scheme.clone(), path.clone())) || !self.language_detection_enabled(&path, &language) {
            return;
        }
        self.language_detection.auto_detect = Some(cx.spawn(async move |workbench, cx| {
            cx.background_executor().timer(AUTO_DETECT_DELAY).await;
            let Ok(detection) = workbench.update(cx, |workbench, _| workbench.detect_language(&scheme, &path)) else { return };
            let detection = detection.await;
            let _ = workbench.update(cx, |workbench, cx| {
                let Some(language) = workbench.detected_language(detection) else { return };
                if workbench.editors.model(&scheme, &path).is_some() && !workbench.text_files.preferred_languages.contains(&(scheme.clone(), path.clone())) {
                    workbench.set_model_language(&scheme, &path, &language, cx);
                }
            });
        }));
    }

    /// `isEnabledForLanguage`: `workbench.editor.languageDetection` with the language's overrides.
    fn language_detection_enabled(&self, path: &PathBuf, language: &str) -> bool {
        let overrides = config::configuration::Overrides { resource: Some(path), override_identifier: Some(language) };
        self.settings.lock().unwrap().get_value(Some("workbench.editor.languageDetection"), &overrides) == Some(json!(true))
    }

    /// `LanguageDetectionStatusContribution._doUpdate`: with `workbench.editor.languageDetectionHints.untitledEditors`,
    /// an untitled model whose detected language differs from its own (jsonc for json aside) offers it.
    fn schedule_detection_status(&mut self, cx: &mut Context<Self>) {
        let hints = self.setting("workbench.editor.languageDetectionHints");
        let Some(editor) = self.editors.active().filter(|editor| editor.document().scheme == "untitled" && hints["untitledEditors"] == true) else {
            self.language_detection.status = None;
            return;
        };
        let (scheme, path) = (editor.document().scheme.clone(), editor.document().path.clone());
        self.language_detection.status = Some(cx.spawn(async move |workbench, cx| {
            cx.background_executor().timer(STATUS_DELAY).await;
            let Ok(detection) = workbench.update(cx, |workbench, _| workbench.detect_language(&scheme, &path)) else { return };
            let detection = detection.await;
            let _ = workbench.update(cx, |workbench, cx| {
                let language = workbench.detected_language(detection);
                let Some(existing) = workbench.editors.model(&scheme, &path).map(|model| model.document().language_id.clone()) else { return };
                workbench.language_detection.offered = language.filter(|language| *language != existing && !(existing == "jsonc" && language == "json"));
                cx.notify();
            });
        }));
    }

    /// The compact `$(lightbulb-autofix)` entry right of the language entry; a click runs `editor.detectLanguage`.
    pub(super) fn language_detection_status_item(&self, foreground: Rgba, cx: &mut Context<Self>) -> Option<AnyElement> {
        let language = self.language_detection.offered.as_ref()?;
        let tooltip = format!("Accept Detected Language: {}", self.languages.name(language));
        let tooltip = self.keybindings.resolver.append_keybinding(&tooltip, DETECT_COMMAND, &HashMap::new());
        let item = div()
            .id("status-language-detection")
            .relative()
            .h_full()
            .flex_none()
            .flex()
            .items_center()
            .px(px(3.))
            .cursor_pointer()
            .child(self.icon("lightbulb-autofix", 16., 16., foreground))
            .child(self.hover_target("language-detection-status", super::hover::HoverContent::Text(tooltip), super::hover::HoverOptions::status_bar()))
            .on_click(cx.listener(|workbench, _, window, cx| drop(workbench.execute_command(DETECT_COMMAND, Vec::new(), window, cx))));
        Some(item.into_any_element())
    }
}

/// The Bun worker over `@vscode/vscode-languagedetection` in the app's node_modules; a thread hands its answers to
/// the waiting requests.
fn start_worker(app_root: &Path) -> std::io::Result<Worker> {
    let root = app_root.join("node_modules/@vscode/vscode-languagedetection");
    let mut child = Command::new(exthost::Runtime::Bun.executable())
        .args(["--eval", include_str!("../language_detection/worker.mjs")])
        .env("VSCODE_LANGUAGE_DETECTION_ROOT", &root)
        .env("DO_NOT_TRACK", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let stdin = child.stdin.take().expect("piped");
    let stdout = child.stdout.take().expect("piped");
    let pending: Pending = Arc::default();
    let answers = pending.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let Ok(answer) = serde_json::from_str::<Value>(&line) else { continue };
            let Some(sender) = answer["id"].as_u64().and_then(|id| answers.lock().unwrap().remove(&id)) else { continue };
            let results = answer["results"].as_array().into_iter().flatten().filter_map(|result| Some((result["languageId"].as_str()?.to_owned(), result["confidence"].as_f64()?))).collect();
            let _ = sender.send(results);
        }
        let _ = child.wait();
    });
    Ok(Worker { stdin, pending, next_id: 0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_worker_runs_the_model_from_node_modules() {
        let repo = Path::new(env!("VSCODE_SRC")).to_path_buf();
        let mut worker = start_worker(&repo).unwrap();
        let (sender, receiver) = oneshot::channel();
        worker.pending.lock().unwrap().insert(7, sender);
        let content = "import os\ndef main():\n    print(os.getcwd())\n\nif __name__ == \"__main__\":\n    main()\n";
        worker.stdin.write_all((json!({ "id": 7, "content": content }).to_string() + "\n").as_bytes()).unwrap();
        let results = futures::executor::block_on(receiver).unwrap();
        assert_eq!(detected_languages(&results), ["py"]);
    }
}
