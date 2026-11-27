//! notebookKernelQuickPickStrategy.ts `KernelPickerMRUStrategy` and notebookKernelHistoryServiceImpl.ts: the kernel
//! picker of the notebook toolbar (`_notebook.selectKernel`), the recent kernels per notebook type in workspace
//! storage, and the kernel sources picker with its extension recommendations when no kernel matches.

use gpui::{Context, Window};
use serde_json::{Value, json};

use super::Workbench;
use crate::commands::Outcome;
use crate::extension_enablement::EnablementState;
use crate::notebook::kernels::{Kernel, association_args};
use crate::quick_input::{Button, Entry, Item, Kind, QuickInput};

const KERNEL_HISTORY_KEY: &str = "notebook.kernelHistory";
const MAX_KERNELS_IN_HISTORY: usize = 5;
const JUPYTER_EXTENSION_ID: &str = "ms-toolsai.jupyter";
/// The title bar back button's handle (`quickInputService.backButton`).
const BACK: i64 = -1;

/// An item of the kernel sources picker.
#[derive(Clone)]
enum SourcePick {
    Kernel(String),
    /// Kernels of one extension, picked from a third picker titled by `source`.
    Group { source: String, kernels: Vec<String> },
    /// `installSuggested`: the recommended extensions.
    Install(Vec<String>),
    /// `install`: Browse marketplace for kernel extensions.
    Marketplace,
}

impl Workbench {
    pub(super) fn register_kernel_picker_commands(&mut self) {
        self.commands.register("notebook.clearNotebookKernelsMRUCache", |workbench, _, _, _| {
            workbench.workspace_storage.lock().unwrap().delete(KERNEL_HISTORY_KEY);
            Outcome::done()
        });
    }

    /// `getKernels(..).all`: the recent kernel ids of a notebook type, newest first.
    fn kernel_history(&self, view_type: &str) -> Vec<String> {
        let stored = self.workspace_storage.lock().unwrap().get(KERNEL_HISTORY_KEY).and_then(|raw| serde_json::from_str::<Value>(raw).ok());
        let entries = stored.map(|stored| stored[view_type]["entries"].clone()).unwrap_or_default();
        entries.as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect()
    }

    /// `addMostRecentKernel`: the kernel goes first, the list keeps five.
    pub(super) fn add_most_recent_kernel(&mut self, view_type: &str, kernel_id: &str) {
        let mut storage = self.workspace_storage.lock().unwrap();
        let mut stored = storage.get(KERNEL_HISTORY_KEY).and_then(|raw| serde_json::from_str::<Value>(raw).ok()).filter(Value::is_object).unwrap_or_else(|| json!({}));
        let mut entries: Vec<Value> = stored[view_type]["entries"].as_array().cloned().unwrap_or_default();
        entries.retain(|entry| entry != kernel_id);
        entries.insert(0, json!(kernel_id));
        entries.truncate(MAX_KERNELS_IN_HISTORY);
        stored[view_type] = json!({ "entries": entries });
        storage.set(KERNEL_HISTORY_KEY, stored.to_string());
    }

    /// `showQuickPick` without a wanted kernel: the selected kernel (the bound one, else the only one) and the recent
    /// ones, then Select Another Kernel..., which runs at once when it is the only item unless `skip_auto_run`.
    pub(super) fn show_kernel_picker(&mut self, skip_auto_run: bool, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let Some(key) = self.active_notebook_editor().map(|editor| editor.notebook.clone()) else { return Outcome::Ready(Ok(json!(false))) };
        let model = self.host.notebooks.model(&key).expect("an editor shows an open notebook");
        let (view_type, uri) = (model.view_type.clone(), model.uri.clone());
        let history = self.kernel_history(&view_type);
        let kernels = &self.host.notebooks.kernels;
        let (all, _) = kernels.matching(&view_type, &key);
        let selected = kernels.resolve(&view_type, &key);
        let mut picked: Vec<&Kernel> = selected.into_iter().collect();
        for id in &history {
            if let Some(kernel) = all.iter().find(|kernel| kernel.id == *id && Some(kernel.id.as_str()) != selected.map(|selected| selected.id.as_str())) {
                picked.push(kernel);
            }
        }
        if picked.is_empty() && !skip_auto_run {
            return self.show_kernel_source_picker(true, window, cx);
        }
        let mut entries: Vec<Entry> = picked.iter().enumerate().map(|(index, kernel)| Entry::Item(kernel_item(index as i64, kernel, selected))).collect();
        if !entries.is_empty() {
            entries.push(Entry::Separator(None));
        }
        let select_another = picked.len() as i64;
        entries.push(Entry::Item(Item { handle: select_another, label: "Select Another Kernel...".into(), pickable: true, ..Default::default() }));
        let ids: Vec<String> = picked.iter().map(|kernel| kernel.id.clone()).collect();
        let label = self.notebook_uri_label(&uri);
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some(match selected {
            Some(_) => format!("Change kernel for '{label}'"),
            None => format!("Select kernel for '{label}'"),
        });
        input.set_items(entries);
        let choice = self.choose(input, window, cx);
        let (sender, receiver) = futures::channel::oneshot::channel();
        cx.spawn_in(window, async move |workbench, cx| {
            let Ok(Ok(handle)) = choice.await else { return drop(sender.send(Ok(json!(false)))) };
            let Some(handle) = handle.as_i64() else { return drop(sender.send(Ok(json!(false)))) };
            let outcome = workbench.update_in(cx, |workbench, window, cx| match ids.get(handle as usize) {
                Some(id) => {
                    workbench.select_picked_kernel(&view_type, &uri, id, cx);
                    Outcome::Ready(Ok(json!(true)))
                }
                None => workbench.show_kernel_source_picker(false, window, cx),
            });
            let _ = sender.send(match outcome {
                Ok(Outcome::Ready(result)) => result,
                Ok(Outcome::Pending(pending)) => pending.await.unwrap_or(Ok(json!(false))),
                Err(_) => Ok(json!(false)),
            });
        })
        .detach();
        Outcome::Pending(receiver)
    }

    /// `KernelPickerMRUStrategy._selecteKernel`: the kernel bound before and the picked one become the most recent.
    pub(super) fn select_picked_kernel(&mut self, view_type: &str, uri: &Value, kernel_id: &str, cx: &mut Context<Self>) {
        let key = crate::notebook::uri_string(uri);
        if let Some(bound) = self.host.notebooks.kernels.matching(view_type, &key).1.map(|kernel| kernel.id.clone()) {
            self.add_most_recent_kernel(view_type, &bound);
        }
        for association in self.host.notebooks.kernels.select(Some(kernel_id), view_type, uri) {
            self.host.send("ExtHostNotebookKernels", "$acceptNotebookAssociation", association_args(&association));
        }
        self.add_most_recent_kernel(view_type, kernel_id);
        cx.notify();
    }

    /// `displaySelectAnotherQuickPick`: the matching kernels grouped by extension (Jupyter's left out), or with none
    /// the extension recommendations; the back button returns to the first picker.
    fn show_kernel_source_picker(&mut self, kernel_list_empty: bool, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let Some(key) = self.active_notebook_editor().map(|editor| editor.notebook.clone()) else { return Outcome::Ready(Ok(json!(false))) };
        let model = self.host.notebooks.model(&key).expect("an editor shows an open notebook");
        let (view_type, uri) = (model.view_type.clone(), model.uri.clone());
        let picks = self.kernel_sources(&key, &view_type);
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.title = Some(if kernel_list_empty { "Select Kernel" } else { "Select Another Kernel" }.into());
        input.placeholder = Some("Type to choose a kernel source".into());
        input.buttons = vec![Button { handle: BACK, codicon: Some("quick-input-back".into()), tooltip: Some("Back".into()), ..Button::default() }];
        // `updateItems(isInitial)`: an empty first list keeps the progress bar.
        input.busy = picks.is_empty();
        input.set_items(picks.iter().enumerate().map(|(index, (item, _))| Entry::Item(Item { handle: index as i64, ..item.clone() })).collect());
        let choice = self.choose(input, window, cx);
        let (sender, receiver) = futures::channel::oneshot::channel();
        cx.spawn_in(window, async move |workbench, cx| {
            let Ok(Ok(handle)) = choice.await else { return drop(sender.send(Ok(json!(false)))) };
            let outcome = workbench.update_in(cx, |workbench, window, cx| {
                if handle == json!("back") {
                    return workbench.show_kernel_picker(true, window, cx);
                }
                let Some((_, pick)) = handle.as_i64().and_then(|handle| picks.get(handle as usize)) else { return Outcome::Ready(Ok(json!(false))) };
                match pick.clone() {
                    SourcePick::Kernel(id) => {
                        workbench.select_picked_kernel(&view_type, &uri, &id, cx);
                        Outcome::Ready(Ok(json!(true)))
                    }
                    SourcePick::Group { source, kernels } => workbench.show_extension_kernel_picker(&source, &kernels, &view_type, &uri, window, cx),
                    SourcePick::Marketplace => workbench.show_kernel_extensions(&view_type, &[], window, cx),
                    SourcePick::Install(ids) => workbench.show_kernel_extensions(&view_type, &ids, window, cx),
                }
            });
            let _ = sender.send(match outcome {
                Ok(Outcome::Ready(result)) => result,
                Ok(Outcome::Pending(pending)) => pending.await.unwrap_or(Ok(json!(false))),
                Err(_) => Ok(json!(false)),
            });
        })
        .detach();
        Outcome::Pending(receiver)
    }

    /// `_calculdateKernelSources` (source actions are not asked: the port has no kernel source action providers).
    fn kernel_sources(&self, key: &str, view_type: &str) -> Vec<(Item, SourcePick)> {
        let (all, _) = self.host.notebooks.kernels.matching(view_type, key);
        if all.is_empty() {
            return self.kernel_recommendations(key);
        }
        let mut groups: Vec<(String, Vec<&Kernel>)> = Vec::new();
        for kernel in all.into_iter().filter(|kernel| !kernel.extension.eq_ignore_ascii_case(JUPYTER_EXTENSION_ID)) {
            match groups.iter_mut().find(|(extension, _)| *extension == kernel.extension) {
                Some((_, kernels)) => kernels.push(kernel),
                None => groups.push((kernel.extension.clone(), vec![kernel])),
            }
        }
        let mut picks = Vec::new();
        for (extension, kernels) in groups {
            if let [kernel] = kernels[..] {
                picks.push((Item { label: kernel.label.clone(), pickable: true, ..Default::default() }, SourcePick::Kernel(kernel.id.clone())));
                continue;
            }
            let manifest = self.extensions.iter().find(|scanned| scanned.id.eq_ignore_ascii_case(&extension)).map(|scanned| &scanned.description);
            let source = manifest.and_then(|manifest| manifest["displayName"].as_str().or(manifest["description"].as_str())).unwrap_or(&extension).to_owned();
            let ids = kernels.iter().map(|kernel| kernel.id.clone()).collect();
            picks.push((Item { label: source.clone(), pickable: true, ..Default::default() }, SourcePick::Group { source, kernels: ids }));
        }
        picks
    }

    /// `_getKernelRecommendationsQuickPickItems`: Install/Enable suggested extensions for a Python notebook (none
    /// when they are installed and enabled: they may still be detecting kernels), then Browse marketplace.
    fn kernel_recommendations(&self, key: &str) -> Vec<(Item, SourcePick)> {
        let model = self.host.notebooks.model(key).expect("an open notebook");
        let mut picks = Vec::new();
        if let Some((extension_ids, display_name)) = suggested_language(model).and_then(|language| kernel_recommendation(&model.view_type, &language)) {
            let local = self.local_extensions();
            let enabled = extension_ids.iter().filter(|id| local.iter().any(|extension| extension.enablement.is_enabled() && extension.identifier().id.eq_ignore_ascii_case(id))).count();
            if enabled == extension_ids.len() {
                return Vec::new();
            }
            let item = Item { label: "$(lightbulb) Install/Enable suggested extensions".into(), description: Some(display_name.into()), pickable: true, ..Default::default() };
            picks.push((item, SourcePick::Install(extension_ids.iter().map(|id| (*id).to_owned()).collect())));
        }
        picks.push((Item { label: "Browse marketplace for kernel extensions".into(), pickable: true, ..Default::default() }, SourcePick::Marketplace));
        picks
    }

    /// `_selectOneKernel`: the kernels of one extension, titled "Select Kernel from <source>".
    fn show_extension_kernel_picker(&mut self, source: &str, kernel_ids: &[String], view_type: &str, uri: &Value, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let key = crate::notebook::uri_string(uri);
        let (all, _) = self.host.notebooks.kernels.matching(view_type, &key);
        let kernels: Vec<&Kernel> = kernel_ids.iter().filter_map(|id| all.iter().find(|kernel| kernel.id == *id).copied()).collect();
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.title = Some(format!("Select Kernel from {source}"));
        input.set_items(kernels.iter().enumerate().map(|(index, kernel)| Entry::Item(kernel_item(index as i64, kernel, None))).collect());
        let ids: Vec<String> = kernels.iter().map(|kernel| kernel.id.clone()).collect();
        let choice = self.choose(input, window, cx);
        let (view_type, uri) = (view_type.to_owned(), uri.clone());
        cx.spawn(async move |workbench, cx| {
            let Ok(Ok(handle)) = choice.await else { return };
            let Some(id) = handle.as_i64().and_then(|handle| ids.get(handle as usize)) else { return };
            let _ = workbench.update(cx, |workbench, cx| workbench.select_picked_kernel(&view_type, &uri, id, cx));
        })
        .detach();
        Outcome::Ready(Ok(json!(true)))
    }

    /// `_showKernelExtension`: disabled ones are enabled and missing ones installed, then `onNotebook:<type>` activates
    /// and the sources picker shows again; with nothing to do, the Extensions view searches `@tag:notebookKernel<Type>`.
    fn show_kernel_extensions(&mut self, view_type: &str, extension_ids: &[String], window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let local = self.local_extensions();
        let mut pending = Vec::new();
        for id in extension_ids {
            match local.iter().find(|extension| extension.identifier().id.eq_ignore_ascii_case(id)) {
                Some(extension) => {
                    let enabled = match extension.enablement {
                        EnablementState::DisabledWorkspace => EnablementState::EnabledWorkspace,
                        EnablementState::DisabledGlobally => EnablementState::EnabledGlobally,
                        EnablementState::DisabledByEnvironment => EnablementState::EnabledByEnvironment,
                        _ => continue,
                    };
                    pending.push(self.set_extension_enablement(std::slice::from_ref(id), enabled, cx));
                }
                None => pending.push(self.install_from_gallery(id.clone(), None, false, cx)),
            }
        }
        if pending.is_empty() {
            let pascal_cased: String = view_type.split(|c: char| !c.is_ascii_alphanumeric()).map(uppercase_first_letter).collect();
            return self.execute_command("workbench.extensions.search", vec![json!(format!("@tag:notebookKernel{pascal_cased}"))], window, cx);
        }
        let view_type = view_type.to_owned();
        let (sender, receiver) = futures::channel::oneshot::channel();
        cx.spawn_in(window, async move |workbench, cx| {
            for outcome in pending {
                if let Outcome::Pending(outcome) = outcome {
                    let _ = outcome.await;
                }
            }
            let Ok(activation) = workbench.update(cx, |workbench, _| workbench.host.activate_by_event(&format!("onNotebook:{view_type}"))) else { return };
            let _ = activation.await;
            let outcome = workbench.update_in(cx, |workbench, window, cx| workbench.show_kernel_source_picker(false, window, cx));
            let _ = sender.send(match outcome {
                Ok(Outcome::Pending(pending)) => pending.await.unwrap_or(Ok(json!(false))),
                Ok(Outcome::Ready(result)) => result,
                Err(_) => Ok(json!(false)),
            });
        })
        .detach();
        Outcome::Pending(receiver)
    }

    /// `labelService.getUriLabel(uri, { relative: true })`: a file inside a workspace folder relative to it, else
    /// its path.
    fn notebook_uri_label(&self, uri: &Value) -> String {
        let path = uri["path"].as_str().unwrap_or_default();
        if uri["scheme"] == "file" {
            for folder in self.workspace_folders() {
                if let Ok(relative) = std::path::Path::new(path).strip_prefix(&folder) {
                    return relative.to_string_lossy().into_owned();
                }
            }
        }
        path.to_owned()
    }
}

/// `toKernelQuickPick`: the selected kernel described as such.
fn kernel_item(handle: i64, kernel: &Kernel, selected: Option<&Kernel>) -> Item {
    let current = selected.is_some_and(|selected| selected.id == kernel.id);
    let description = match (current, &kernel.description) {
        (false, description) => description.clone(),
        (true, None) => Some("Currently Selected".into()),
        (true, Some(description)) => Some(format!("{description} - Currently Selected")),
    };
    Item { handle, label: kernel.label.clone(), description, detail: kernel.detail.clone(), picked: current, pickable: true, ..Default::default() }
}

/// `getSuggestedLanguage`: the metadata's `language_info.name`, else the language of every non-markdown cell when
/// there are more than one and they agree.
fn suggested_language(model: &crate::notebook::NotebookModel) -> Option<String> {
    if let Some(name) = model.metadata["metadata"]["language_info"]["name"].as_str() {
        return Some(name.to_owned());
    }
    let languages: Vec<&str> = model.cells.iter().map(|cell| cell.language.as_str()).filter(|language| *language != "markdown").collect();
    match languages[..] {
        [first, _, ..] if languages.iter().all(|language| *language == first) => Some(first.to_owned()),
        _ => None,
    }
}

/// notebookBrowser.ts `KERNEL_RECOMMENDATIONS`.
fn kernel_recommendation(view_type: &str, language: &str) -> Option<(&'static [&'static str], &'static str)> {
    match (view_type, language) {
        ("jupyter-notebook", "python") => Some((&["ms-python.python", JUPYTER_EXTENSION_ID], "Python + Jupyter")),
        _ => None,
    }
}

fn uppercase_first_letter(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}
