//! Workspace recommendations actions (extensionsActions.ts, extensionsViews.ts, extensionRecommendationNotificationService.ts):
//! the startup prompt, Install Workspace Recommended Extensions and the two Configure Recommended Extensions commands.

use std::path::{Path, PathBuf};

use config::json::{ParseOptions, Segment, parse_tree};
use config::json_edit::{FormattingOptions, apply_edits, set_property};
use exthost::gallery::{ExtensionInfo, Gallery};
use gpui::{Context, Window};
use serde_json::{Value, json};

use super::Workbench;
use crate::commands::Outcome;
use crate::editor::{Position, Selection, VerticalReveal};
use crate::extension_recommendations as recommendations;
use crate::main_thread_quick_open::Owner;
use crate::notifications::{Action, Notification, Run, Severity};
use crate::quick_input::{Entry, Event, Item, Kind, QuickInput, path_label};

/// extensionsFileTemplate.ts `ExtensionsConfigurationInitialContent`.
const EXTENSIONS_CONFIGURATION_INITIAL_CONTENT: &str = "{\n\t// See https://go.microsoft.com/fwlink/?LinkId=827846 to learn about workspace recommendations.\n\t// Extension identifier format: ${publisher}.${name}. Example: vscode.csharp\n\n\t// List of extensions which should be recommended for users of this workspace.\n\t\"recommendations\": [\n\t\t\n\t],\n\t// List of extensions recommended by VS Code that should not be recommended for users of this workspace.\n\t\"unwantedRecommendations\": [\n\t\t\n\t]\n}";
/// extensionRecommendationsService.ts `promptWorkspaceRecommendations` waits 5 s after activation.
const PROMPT_DELAY: std::time::Duration = std::time::Duration::from_secs(5);

impl Workbench {
    pub(super) fn register_workspace_recommendations_commands(&mut self) {
        self.commands.register("workbench.extensions.action.installWorkspaceRecommendedExtensions", |workbench, _, _, cx| {
            workbench.install_workspace_recommendations(cx);
            Outcome::done()
        });
        self.commands.register("workbench.extensions.action.configureWorkspaceRecommendedExtensions", |workbench, _, window, cx| {
            let workspace = workbench.settings.lock().unwrap().workspace.clone();
            let Some(workspace) = workspace else { return Outcome::done() };
            match (&workspace.file, workspace.folders.first()) {
                (Some(file), _) => workbench.open_workspace_file_recommendations(file.clone(), window, cx),
                (None, Some(folder)) => workbench.open_extensions_file(&folder.path, window, cx),
                (None, None) => Outcome::done(),
            }
        });
        self.commands.register("workbench.extensions.action.configureWorkspaceFolderRecommendedExtensions", |workbench, _, window, cx| {
            let workspace = workbench.settings.lock().unwrap().workspace.clone();
            let Some(workspace) = workspace else { return Outcome::done() };
            match workspace.folders.as_slice() {
                [] => Outcome::done(),
                [folder] => workbench.open_extensions_file(&folder.path, window, cx),
                // `PICK_WORKSPACE_FOLDER_COMMAND_ID`.
                folders => {
                    let items = folders
                        .iter()
                        .enumerate()
                        .map(|(handle, folder)| {
                            let parent = folder.path.parent().map(|parent| path_label(parent, Some(&workspace))).unwrap_or_default();
                            let description = (parent != folder.name).then_some(parent);
                            Entry::Item(Item { handle: handle as i64, label: folder.name.clone(), description, pickable: true, ..Item::default() })
                        })
                        .collect();
                    let mut input = QuickInput::new(0, Kind::QuickPick);
                    input.placeholder = Some("Select workspace folder".into());
                    input.match_on_description = true;
                    input.set_items(items);
                    let id = workbench.quick_inputs.add(input, Owner::ExtensionsFolder(folders.iter().map(|folder| folder.path.clone()).collect()));
                    workbench.show_quick_input(id, window, cx);
                    workbench.flush_quick_input_events(window, cx);
                    Outcome::done()
                }
            }
        });
        self.commands.register("_extensions.workspaceRecommendations.dontShowAgain", |workbench, _, _, _| {
            workbench.workspace_storage.lock().unwrap().set(recommendations::DONOT_SHOW_WORKSPACE_RECOMMENDATIONS_KEY, "true".into());
            Outcome::done()
        });
        self.commands.register("_extensions.workspaceRecommendations.install", |workbench, args, window, cx| {
            let ids: Vec<String> = args.iter().filter_map(Value::as_str).map(str::to_owned).collect();
            workbench.search_extensions("@recommended", window, cx);
            for id in ids {
                drop(workbench.install_from_gallery(id, None, false, cx));
            }
            Outcome::done()
        });
    }

    pub fn extensions_folder_pick_event(&mut self, id: i64, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            Event::Accept(_) => {
                let Some(handle) = self.quick_inputs.get(id).and_then(|input| input.selected_handles().first().copied()) else { return };
                let Some(Owner::ExtensionsFolder(folders)) = self.quick_inputs.owner(id) else { return };
                let folder = folders[handle as usize].clone();
                self.hide_quick_input_by_id(id, window, cx);
                let outcome = self.open_extensions_file(&folder, window, cx);
                super::quick_access::show_failure("Configure Recommended Extensions (Workspace Folder)".into(), outcome, window, cx);
            }
            Event::Hide => self.quick_inputs.remove(id),
            _ => {}
        }
    }

    /// `promptWorkspaceRecommendations`, 5 s after startup: unless the repository or the settings opted out, the
    /// recommendations not installed yet that Open VSX knows.
    pub(super) fn schedule_workspace_recommendations_prompt(&mut self, cx: &mut Context<Self>) {
        if self.settings.lock().unwrap().workspace.is_none() {
            return;
        }
        cx.spawn(async move |workbench, cx| {
            cx.background_executor().timer(PROMPT_DELAY).await;
            let _ = workbench.update(cx, |workbench, cx| workbench.prompt_workspace_recommendations(cx));
        })
        .detach();
    }

    fn prompt_workspace_recommendations(&mut self, cx: &mut Context<Self>) {
        if self.workspace_storage.lock().unwrap().get(recommendations::DONOT_SHOW_WORKSPACE_RECOMMENDATIONS_KEY) == Some("true") || self.ignores_recommendation_notifications() {
            return;
        }
        let ids = self.uninstalled_workspace_recommendations();
        if ids.is_empty() {
            return;
        }
        let fetch = cx.background_executor().spawn(async move {
            let infos: Vec<ExtensionInfo> = ids.iter().map(|id| ExtensionInfo { id: id.clone(), uuid: None, version: None, pre_release: false }).collect();
            Gallery::open_vsx(crate::VERSION).get_extensions(&infos, true)
        });
        cx.spawn(async move |workbench, cx| {
            let found = match fetch.await {
                Ok(found) => found,
                Err(error) => return eprintln!("[extensions] workspace recommendations: {error:#}"),
            };
            if found.is_empty() {
                return;
            }
            let _ = workbench.update(cx, |workbench, cx| {
                let named: Vec<(String, String)> = found.iter().map(|gallery| (gallery.display_name.clone(), gallery.publisher_display_name.clone())).collect();
                let ids: Vec<Value> = found.iter().map(|gallery| json!(gallery.id)).collect();
                let mut notification = Notification::new(Severity::Info, recommendations::prompt_message(&named));
                notification.sticky = true;
                notification.primary = vec![
                    Action { id: "workbench.notification.choice.0".into(), label: "Install".into(), run: Run::Command("_extensions.workspaceRecommendations.install".into(), ids) },
                    Action { id: "workbench.notification.choice.1".into(), label: "Show Recommendations".into(), run: Run::Command("workbench.extensions.action.showRecommendedExtensions".into(), Vec::new()) },
                ];
                notification.secondary = vec![Action {
                    id: "workbench.notification.choice.2".into(),
                    label: "Don't Show Again for this Repository".into(),
                    run: Run::Command("_extensions.workspaceRecommendations.dontShowAgain".into(), Vec::new()),
                }];
                workbench.notify(notification);
                cx.notify();
            });
        })
        .detach();
    }

    /// `hasToIgnoreRecommendationNotifications`.
    fn ignores_recommendation_notifications(&self) -> bool {
        self.setting("extensions.ignoreRecommendations") == true || self.setting("extensions.showRecommendationsOnlyOnDemand") == true
    }

    fn uninstalled_workspace_recommendations(&self) -> Vec<String> {
        let local = self.local_extensions();
        self.workspace_recommendations().into_iter().filter(|id| !local.iter().any(|extension| extension.identifier().id.eq_ignore_ascii_case(id))).collect()
    }

    /// `installWorkspaceRecommendations`.
    fn install_workspace_recommendations(&mut self, cx: &mut Context<Self>) {
        let ids = self.uninstalled_workspace_recommendations();
        if ids.is_empty() {
            self.notify(Notification::new(Severity::Info, "There are no extensions to install."));
            cx.notify();
            return;
        }
        for id in ids {
            drop(self.install_from_gallery(id, None, false, cx));
        }
    }

    /// `openExtensionsFile`: the folder's extensions.json, created from the template when missing, with the cursor
    /// after the last recommendation.
    fn open_extensions_file(&mut self, folder: &Path, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let path = config::service::folder_config_dir(folder).join("extensions.json");
        let created = !path.exists();
        if created {
            let written = std::fs::create_dir_all(path.parent().expect("a config dir")).and_then(|()| std::fs::write(&path, EXTENSIONS_CONFIGURATION_INITIAL_CONTENT));
            if let Err(error) = written {
                return Outcome::error(format!("Unable to create 'extensions.json' file inside the '.vscode' folder ({error})."));
            }
        }
        self.open_at_recommendations(path, &[Segment::Key("recommendations".into())], created, window, cx);
        Outcome::done()
    }

    /// `openWorkspaceConfigurationFile`: adds `extensions.recommendations` to the workspace file when missing.
    fn open_workspace_file_recommendations(&mut self, file: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let text = match std::fs::read_to_string(&file) {
            Ok(text) => text,
            Err(error) => return Outcome::error(error.to_string()),
        };
        let has_recommendations = recommendations::read_workspace_file_config(&file).is_some_and(|config| config.recommendations.is_some());
        if !has_recommendations {
            let options = FormattingOptions { tab_size: 4, insert_spaces: false, eol: "\n".into() };
            let edits = set_property(&text, &[Segment::Key("extensions".into())], Some(&json!({ "recommendations": [] })), &options);
            if let Err(error) = std::fs::write(&file, apply_edits(&text, &edits)) {
                return Outcome::error(error.to_string());
            }
        }
        self.open_at_recommendations(file, &[Segment::Key("extensions".into()), Segment::Key("recommendations".into())], false, window, cx);
        Outcome::done()
    }

    /// `getSelectionPosition`: after the last entry of the array at `path`, else just inside its bracket.
    fn open_at_recommendations(&mut self, path: PathBuf, json_path: &[Segment], pinned: bool, window: &mut Window, cx: &mut Context<Self>) {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        self.open_file(path, pinned, window, cx);
        let tree = parse_tree(&text, &mut Vec::new(), ParseOptions { allow_trailing_comma: true, ..ParseOptions::default() });
        let Some(array) = tree.find(tree.root, json_path) else { return };
        let array = tree.node(array);
        let offset = match array.children.last() {
            Some(&last) => tree.node(last).offset + tree.node(last).length,
            None => array.offset + 1,
        };
        let before = &text[..offset];
        let line_start = before.rfind('\n').map_or(0, |index| index + 1);
        let position = Position::new(before.matches('\n').count(), before[line_start..].chars().count());
        let viewport = self.editors.viewport;
        let Some(editor) = self.editors.active_mut() else { return };
        editor.set_selections(vec![Selection::caret(position)]);
        editor.reveal_range(position, position, VerticalReveal::CenterIfOutsideViewport, viewport);
        self.host.send_selection(self.editors.active().expect("just opened"), "api");
        cx.notify();
    }
}
