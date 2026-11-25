//! The merge editor (view/mergeEditor.ts, view/editors/*.ts, viewZones.ts, conflictActions.ts, scrollSynchronizer.ts
//! and commands/commands.ts): input 1 and input 2 read-only beside each other, the result model in the group's code
//! editor, and base on request, in the mixed or the column layout; an action row above each modified base range,
//! accept checkboxes in the inputs' gutters and spacers that line the panes up.
// shortcut: the result shows no base-to-result diff backgrounds and base no "Comparing with" highlights: the group's
// code editor takes no merge decorations yet. Add them with a decoration provider hook in the text area.

use std::path::Path;

use diff::LineRange;
use gpui::{Context, Div, MouseDownEvent, Rgba, ScrollWheelEvent, StyledText, Window, div, prelude::*, px, relative};
use serde_json::{Value, json};

use super::diff_editor_view::{char_ranges, diagonal_fill};
use super::{EditorMetrics, Workbench};
use crate::commands::Outcome;
use crate::diff_editor::{Side, ZoneSide};
use crate::editor::{EditReason, OpenEditor, Position, TextDocument, VerticalReveal, expand_tabs};
use crate::menus::MenuAction;
use crate::merge_editor::{DocumentRangeMap, Input, InputData, LAYOUT_KEY, Layout, LineEdit, MERGE_EDITOR_SCHEME, MergeInput, MergeModel, SHOW_NON_CONFLICTING_KEY, State, ViewZones, Zone, ZoneKind, ZoneOptions, text_edit, text_of};
use crate::workbench::host_requests::uri_parts;
use crate::workbench::hover::{HoverContent, HoverOptions};

/// mergeEditor.css `.code-view > .title` height.
const TITLE_HEIGHT: f32 = 24.;
/// mergeEditor.css `.merge-accept-gutter-marker`: 28px wide with a 4px left margin.
const GUTTER_WIDTH: f32 = 32.;
/// viewZones.ts `Placeholder` height in base.
const PLACEHOLDER_HEIGHT: f32 = 16.;

/// One action of a range's row: its label, and what it does when clicked (`None`: a label only).
type Action = (String, Option<MergeAction>);

#[derive(Clone, Copy)]
enum MergeAction {
    SetState(usize, State),
    Ignore(usize, Input),
}

/// Which read-only pane: an input or base.
#[derive(Clone, Copy, PartialEq)]
enum Pane {
    Input(Input),
    Base,
}

impl Workbench {
    pub(in crate::workbench) fn register_merge_editor_commands(&mut self) {
        self.commands.register("_open.mergeEditor", |workbench, args, window, cx| workbench.open_merge_editor(args, window, cx));
        self.commands.register("merge.goToNextUnhandledConflict", |workbench, _, _, cx| {
            workbench.go_to_unhandled_conflict(true, cx);
            Outcome::done()
        });
        self.commands.register("merge.goToPreviousUnhandledConflict", |workbench, _, _, cx| {
            workbench.go_to_unhandled_conflict(false, cx);
            Outcome::done()
        });
        for (id, input) in [("merge.acceptAllInput1", Input::One), ("merge.acceptAllInput2", Input::Two)] {
            self.commands.register(id, move |workbench, _, _, cx| {
                workbench.accept_all(input, cx);
                Outcome::done()
            });
        }
        for (id, input) in [("merge.toggleActiveConflictInput1", Input::One), ("merge.toggleActiveConflictInput2", Input::Two)] {
            self.commands.register(id, move |workbench, _, _, cx| workbench.toggle_active_conflict(input, cx));
        }
        self.commands.register("mergeEditor.resetResultToBaseAndAutoMerge", |workbench, _, _, cx| {
            workbench.reset_merge_result(cx);
            Outcome::done()
        });
        self.commands.register("mergeEditor.acceptMerge", |workbench, _, window, cx| workbench.accept_merge(window, cx));
        // mergeEditor.ts `setLayoutKind`, `toggleBase`, `toggleShowBaseTop` and `toggleShowBaseCenter`.
        let layouts: [(&str, fn(Layout) -> Layout); 5] = [
            ("merge.mixedLayout", |layout| Layout { columns: false, ..layout }),
            ("merge.columnLayout", |layout| Layout { columns: true, ..layout }),
            ("merge.showBase", |layout| Layout { show_base: !layout.show_base, ..layout }),
            ("merge.showBaseTop", |layout| Layout { show_base: !(layout.show_base && layout.show_base_at_top), show_base_at_top: true, ..layout }),
            ("merge.showBaseCenter", |layout| Layout { show_base: !(layout.show_base && !layout.show_base_at_top), show_base_at_top: false, ..layout }),
        ];
        for (id, change) in layouts {
            self.commands.register(id, move |workbench, _, _, cx| {
                if workbench.editors.active_merge().is_some() {
                    let layout = change(workbench.merge_layout());
                    workbench.profile_storage.lock().unwrap().set(LAYOUT_KEY, layout.to_json());
                    cx.notify();
                }
                Outcome::done()
            });
        }
        self.commands.register("merge.showNonConflictingChanges", |workbench, _, _, cx| {
            if workbench.editors.active_merge().is_some() {
                let shown = workbench.merge_show_non_conflicting();
                workbench.profile_storage.lock().unwrap().set(SHOW_NON_CONFLICTING_KEY, json!(!shown).to_string());
                cx.notify();
            }
            Outcome::done()
        });
        // commands.ts `OpenResultResource` and `OpenBaseFile`: the result or base in an editor of its own.
        for (id, base) in [("merge.openResult", false), ("merge.openBaseEditor", true)] {
            self.commands.register(id, move |workbench, _, window, cx| {
                let Some(merge) = workbench.editors.active_merge() else { return Outcome::done() };
                let side = if base { &merge.base } else { &merge.result };
                let uri = crate::editor::uri_value(&side.scheme, &side.path);
                workbench.execute_command("vscode.open", vec![uri], window, cx)
            });
        }
        self.commands.register("mergeEditor.acceptAllCombination", |workbench, _, _, cx| {
            workbench.accept_all_combination(cx);
            Outcome::done()
        });
        for (id, input) in [("mergeEditor.compareInput1WithBase", Input::One), ("mergeEditor.compareInput2WithBase", Input::Two)] {
            self.commands.register(id, move |workbench, _, window, cx| workbench.compare_input_with_base(input, window, cx));
        }
        self.commands.register("mergeEditor.resetCloseWithConflictsChoice", |workbench, _, _, _| {
            workbench.profile_storage.lock().unwrap().delete(CLOSE_WITH_CONFLICTS_KEY);
            Outcome::done()
        });
        // shortcut: the inputs take no keyboard focus, the toggle only marks the input pane; give the panes focus handles to port it.
        self.commands.register("mergeEditor.toggleBetweenInputs", |workbench, _, _, cx| {
            if let Some(merge) = workbench.editors.active_merge_mut() {
                merge.focused_input = Some(if merge.focused_input == Some(Input::One) { Input::Two } else { Input::One });
                cx.notify();
            }
            Outcome::done()
        });
        self.register_merge_dev_commands();
    }

    /// devCommands.ts of browser/ and electron-browser/.
    fn register_merge_dev_commands(&mut self) {
        self.commands.register("merge.dev.copyContentsJson", |workbench, _, _, cx| {
            let Some(contents) = workbench.merge_contents() else { return workbench.merge_info("No active merge editor") };
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(pretty_json(&contents)));
            workbench.merge_info("Successfully copied merge editor state")
        });
        self.commands.register("merge.dev.saveContentsToFolder", |workbench, _, window, cx| {
            let Some(contents) = workbench.merge_contents() else { return workbench.merge_info("No active merge editor") };
            let extension = workbench.editors.active().and_then(|result| result.document.path.extension()).map(|extension| format!(".{}", extension.to_string_lossy())).unwrap_or_default();
            let picked = cx.prompt_for_paths(gpui::PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Select folder to save to".into()) });
            cx.spawn_in(window, async move |workbench, cx| {
                let Ok(Ok(Some(paths))) = picked.await else { return };
                let Some(folder) = paths.first() else { return };
                let written: std::io::Result<()> = ["base", "input1", "input2", "result", "initialResult"].into_iter().try_for_each(|name| std::fs::write(folder.join(format!("{name}{extension}")), contents[name].as_str().unwrap_or_default()));
                drop(workbench.update(cx, |workbench, cx| {
                    match written {
                        Ok(()) => drop(workbench.merge_info("Successfully saved merge editor state to folder")),
                        Err(error) => workbench.notify_error(error.to_string()),
                    }
                    cx.notify();
                }));
            })
            .detach();
            Outcome::done()
        });
        self.commands.register("merge.dev.loadContentsFromFolder", |_, args, window, cx| {
            let options = args.first().cloned().unwrap_or(Value::Null);
            let folder = uri_parts(&options["folderUri"]).ok().map(|(_, path)| path);
            let picked = folder.is_none().then(|| cx.prompt_for_paths(gpui::PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Select folder to save to".into()) }));
            cx.spawn_in(window, async move |workbench, cx| {
                let folder = match (folder, picked) {
                    (Some(folder), _) => folder,
                    (None, Some(picked)) => match picked.await {
                        Ok(Ok(Some(paths))) if !paths.is_empty() => paths[0].clone(),
                        _ => return,
                    },
                    (None, None) => return,
                };
                let Ok(entries) = std::fs::read_dir(&folder) else { return };
                let names: Vec<String> = entries.flatten().map(|entry| entry.file_name().to_string_lossy().into_owned()).collect();
                let open_initial = open_initial_result(&workbench, cx, options["resultState"].as_str()).await;
                let find = |prefix: &str| names.iter().find(|name| name.starts_with(prefix)).map(|name| file_uri(&folder.join(name)));
                let (Some(base), Some(input1), Some(input2), Some(result)) = (find("base"), find("input1"), find("input2"), find(if open_initial { "initialResult" } else { "result" })) else { return };
                drop(workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("_open.mergeEditor", vec![merge_args(base, input1, input2, result, "(from file)")], window, cx))));
            })
            .detach();
            Outcome::done()
        });
        self.commands.register("merge.dev.openContentsJson", |workbench, args, window, cx| {
            let options = args.first().cloned().unwrap_or(Value::Null);
            let data = match &options["data"] {
                Value::Null => {
                    // shortcut: the JSON comes from the clipboard as is; upstream shows it in an input box first.
                    let text = cx.read_from_clipboard().and_then(|item| item.text()).unwrap_or_default();
                    match text.as_str() {
                        "" => json!({ "base": "", "input1": "", "input2": "", "result": "", "languageId": "plaintext" }),
                        text => match serde_json::from_str(text) {
                            Ok(data) => data,
                            Err(error) => return Outcome::error(error.to_string()),
                        },
                    }
                }
                data => data.clone(),
            };
            workbench.open_merge_contents(data, options["resultState"].as_str().map(str::to_string), window, cx);
            Outcome::done()
        });
        self.commands.register("merge.dev.openSelectionInTemporaryMergeEditor", |workbench, _, window, cx| {
            workbench.sync_merge();
            let Some(data) = workbench.merge_selection_contents() else { return Outcome::done() };
            workbench.open_merge_contents(data, None, window, cx);
            Outcome::done()
        });
    }

    /// devCommands.ts `MergeEditorContents` of the active merge editor.
    fn merge_contents(&self) -> Option<Value> {
        let merge = self.editors.active_merge()?;
        let text = |side: &Side| self.editors.model(&side.scheme, &side.path).map(|model| model.document.lines.join("\n"));
        Some(json!({
            "languageId": self.editors.active()?.document.language_id,
            "base": text(&merge.base)?,
            "input1": text(&merge.input1.side)?,
            "input2": text(&merge.input2.side)?,
            "result": text(&merge.result)?,
            "initialResult": merge.initial_result.join("\n"),
        }))
    }

    /// `OpenSelectionInTemporaryMergeEditor`: the result's selections in base, and the same ranges of each model.
    fn merge_selection_contents(&self) -> Option<Value> {
        let model = self.active_merge_model()?;
        let result = self.editors.active()?;
        let to_base = DocumentRangeMap::of(&model.result_diffs).reverse();
        let to_result = DocumentRangeMap::of(&model.result_diffs);
        let to_input = |input: Input| DocumentRangeMap::of(&model.ranges.iter().flat_map(|range| range.diffs(input).iter().cloned()).collect::<Vec<_>>());
        let (to_input1, to_input2) = (to_input(Input::One), to_input(Input::Two));
        let ranges: Vec<_> = result
            .selections
            .iter()
            .map(|selection| {
                let (start, end) = (selection.anchor.min(selection.head), selection.anchor.max(selection.head));
                to_base.project_range(((start.line + 1, start.column + 1), (end.line + 1, end.column + 1)))
            })
            .collect();
        if ranges.is_empty() {
            return None;
        }
        let join = |lines: &[String], map: Option<&DocumentRangeMap>| ranges.iter().map(|range| text_of(lines, map.map_or(*range, |map| map.project_range(*range)))).collect::<Vec<_>>().join("\n");
        Some(json!({
            "base": join(&model.base, None),
            "input1": join(&model.input1, Some(&to_input1)),
            "input2": join(&model.input2, Some(&to_input2)),
            "result": join(&result.document.lines, Some(&to_result)),
            "languageId": result.document.language_id,
        }))
    }

    /// electron-browser/devCommands.ts `MergeEditorOpenContentsFromJSON`: the contents written to a temporary folder
    /// and opened in a merge editor.
    // shortcut: the files get no extension of the language (the port's language registry lists no extensions by id).
    fn open_merge_contents(&mut self, data: Value, result_state: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_nanos());
        let folder = std::env::temp_dir().join(format!("merge-editor-{nanos:x}"));
        cx.spawn_in(window, async move |workbench, cx| {
            let open_initial = open_initial_result(&workbench, cx, result_state.as_deref()).await;
            let text = |key: &str| data[key].as_str().unwrap_or_default().to_string();
            let files = [("base", text("base")), ("input1", text("input1")), ("input2", text("input2")), ("result", if open_initial { text("initialResult") } else { text("result") }), ("initialResult", text("initialResult"))];
            let written = std::fs::create_dir_all(&folder).and_then(|()| files.iter().try_for_each(|(name, text)| std::fs::write(folder.join(name), text)));
            drop(workbench.update_in(cx, |workbench, window, cx| match written {
                Ok(()) => {
                    let uri = |name: &str| file_uri(&folder.join(name));
                    drop(workbench.execute_command("_open.mergeEditor", vec![merge_args(uri("base"), uri("input1"), uri("input2"), uri("result"), "(from JSON)")], window, cx));
                }
                Err(error) => workbench.notify_error(error.to_string()),
            }));
        })
        .detach();
    }

    fn merge_info(&mut self, message: &str) -> Outcome {
        self.notify(crate::notifications::Notification::new(crate::notifications::Severity::Info, message));
        Outcome::done()
    }

    /// commands.ts `AcceptAllCombination`: every range that takes neither input yet and can be combined takes both.
    fn accept_all_combination(&mut self, cx: &mut Context<Self>) {
        if !self.sync_merge() {
            return;
        }
        let count = self.active_merge_model().map_or(0, |model| model.ranges.len());
        for index in 0..count {
            let model = self.active_merge_model().expect("synced above");
            let state = model.accepted[index];
            if state == State::Unrecognized || state.includes(Input::One) || !model.can_be_combined(index) {
                continue;
            }
            self.set_merge_state(index, state.with_input(Input::One, true, false).with_input(Input::Two, true, true), cx);
        }
    }

    /// commands.ts `mergeEditorCompare`: the merge editor is pinned, then a diff editor of base and the input opens
    /// at the input line beside the result's cursor.
    fn compare_input_with_base(&mut self, input: Input, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        self.sync_merge();
        let (Some(merge), Some(model), Some(result)) = (self.editors.active_merge(), self.active_merge_model(), self.editors.active()) else { return Outcome::done() };
        let side = if input == Input::One { &merge.input1.side } else { &merge.input2.side };
        let line = model.input_line_for_result_line(input, result.selection().head.line + 1);
        let (base, input_uri) = (crate::editor::uri_value(&merge.base.scheme, &merge.base.path), crate::editor::uri_value(&side.scheme, &side.path));
        if let Some(index) = self.editors.active_index() {
            self.editors.pin(index);
        }
        let options = json!({ "selection": { "startLineNumber": line, "startColumn": 1, "endLineNumber": line, "endColumn": 1 }, "pinned": true });
        self.execute_command("_workbench.diff", vec![base, input_uri, Value::Null, json!([Value::Null, options])], window, cx)
    }

    fn merge_layout(&self) -> Layout {
        Layout::parse(self.profile_storage.lock().unwrap().get(LAYOUT_KEY))
    }

    /// mergeEditor.ts `showNonConflictingChangesStore`.
    fn merge_show_non_conflicting(&self) -> bool {
        self.profile_storage.lock().unwrap().get(SHOW_NON_CONFLICTING_KEY) == Some("true")
    }

    /// commands.ts `OpenMergeEditor`: `{ base, input1: { uri, title, description, detail }, input2, output }`. The
    /// models open first, a missing one from its content or file system provider.
    fn open_merge_editor(&mut self, args: Vec<Value>, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let arg = args.first().cloned().unwrap_or(Value::Null);
        let uris = [arg["base"].clone(), arg["input1"]["uri"].clone(), arg["input2"]["uri"].clone(), arg["output"].clone()];
        let mut sides = Vec::new();
        for uri in &uris {
            match uri_parts(uri) {
                Ok((scheme, path)) => sides.push(Side { scheme: scheme.to_string(), path }),
                Err(error) => return Outcome::Ready(Err(error)),
            }
        }
        let missing: Vec<Value> = uris.iter().zip(&sides).filter(|(_, side)| side.scheme != "file" && self.editors.model(&side.scheme, &side.path).is_none()).map(|(uri, _)| uri.clone()).collect();
        let contents: Vec<_> = missing.iter().map(|uri| (uri.clone(), self.host.call("workbench", "resolveTextContent", vec![uri.clone()]))).collect();
        let (sender, receiver) = futures::channel::oneshot::channel();
        cx.spawn_in(window, async move |workbench, cx| {
            let mut resolved = Vec::new();
            for (uri, content) in contents {
                resolved.push((uri, content.await.unwrap_or_else(|_| Err(exthost::main_thread::serialized_error("Canceled")))));
            }
            let result = workbench.update_in(cx, |workbench, window, cx| {
                for (uri, content) in resolved {
                    workbench.try_open_document(&uri, &Value::Null, &content?, false, cx)?;
                }
                for (uri, side) in uris.iter().zip(&sides) {
                    if side.scheme == "file" && workbench.editors.model(&side.scheme, &side.path).is_none() {
                        workbench.try_open_document(uri, &Value::Null, &Value::Null, false, cx)?;
                    }
                }
                let data = |input: &Value, side: &Side| {
                    let label = |key: &str| input[key].as_str().map(str::to_string);
                    InputData { side: side.clone(), title: label("title"), description: label("description"), detail: label("detail") }
                };
                let (input1, input2) = (data(&arg["input1"], &sides[1]), data(&arg["input2"], &sides[2]));
                workbench.show_merge_editor(sides[0].clone(), input1, input2, sides[3].clone(), window, cx);
                Ok(Value::Null)
            });
            drop(sender.send(result.unwrap_or_else(|_| Err(exthost::main_thread::serialized_error("Canceled")))));
        })
        .detach();
        Outcome::Pending(receiver)
    }

    /// `openEditor` of a `MergeEditorInput`: an open merge editor of the same result is activated, else a new one
    /// opens; a result with conflict markers is reset to the auto-merged text (mergeEditorInputModel.ts), and the
    /// first conflict becomes the active range (mergeEditor.ts `setInput`).
    fn show_merge_editor(&mut self, base: Side, input1: InputData, input2: InputData, result: Side, window: &mut Window, cx: &mut Context<Self>) {
        match self.editors.editors.iter().position(|editor| self.editors.merges.get(&editor.model_id).is_some_and(|merge| merge.result == result)) {
            Some(index) => self.editors.activate(index),
            None => {
                let initial_result = self.editors.model(&result.scheme, &result.path).map_or_else(Vec::new, |model| model.document.lines.clone());
                let id = self.editors.merges.len() + self.editors.editors.len() + 1;
                let input = MergeInput::new(base, input1, input2, result, initial_result);
                // The entry's file name is the tab label (`MergeEditorInput.getName`).
                let entry = TextDocument::from_text(MERGE_EDITOR_SCHEME, Path::new(&format!("/{id}/{}", input.name())), "", "utf8".into(), Some("plaintext"), &self.languages, &self.settings.lock().unwrap());
                self.editors.open_merge(entry, input, false);
                self.update_merge_editor(cx);
                let merge = self.editors.active_merge_mut().expect("opened above");
                merge.active_range = merge.model.as_ref().and_then(|model| model.ranges.iter().position(|range| range.is_conflicting()));
            }
        }
        self.marked_text = None;
        self.update_merge_editor(cx);
        window.focus(&self.editor_focus, cx);
        self.editors_changed(cx);
    }

    /// Brings the active merge editor's model up to its models, resets a result opened with conflict markers and
    /// places the result's view zones; other merge editors take theirs back.
    pub(in crate::workbench) fn update_merge_editor(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<u32> = self.editors.merges.keys().copied().collect();
        for id in ids {
            let merge = self.editors.merges.get_mut(&id).expect("listed above");
            let (placed, result) = (std::mem::take(&mut merge.placed_zones), merge.result.clone());
            if let Some(model) = self.editors.model_mut(&result.scheme, &result.path) {
                for (zone, _) in placed {
                    model.remove_view_zone(zone);
                }
            }
        }
        if !self.sync_merge() {
            return;
        }
        let merge = self.editors.active_merge_mut().expect("synced above");
        if std::mem::take(&mut merge.reset_pending) {
            self.reset_merge_result(cx);
        }
        let zones = self.merge_view_zones().result;
        let heights: Vec<f32> = zones.iter().map(|zone| self.merge_zone_height(zone.kind)).collect();
        let result = self.editors.active_mut().expect("the merge editor's result model");
        let placed = zones.into_iter().zip(heights).map(|(zone, height)| (result.add_view_zone(zone.after_line.checked_sub(1).map(|line| Position::new(line, 0)), height), zone)).collect();
        self.editors.active_merge_mut().expect("synced above").placed_zones = placed;
    }

    /// Recomputes the active merge editor's ranges when an input changed and reads its states back from a result
    /// that changed, after following its undo history (mergeEditorModel.ts autoruns, viewModel.ts
    /// `AttachedHistory`); false without an active merge editor or one of its models.
    fn sync_merge(&mut self) -> bool {
        let Some(id) = self.editors.active_tab().map(|tab| tab.model_id) else { return false };
        let Some(mut merge) = self.editors.merges.remove(&id) else { return false };
        let model = |side: &Side| self.editors.model(&side.scheme, &side.path);
        let synced = if let (Some(base), Some(input1), Some(input2), Some(result)) = (model(&merge.base), model(&merge.input1.side), model(&merge.input2.side), model(&merge.result)) {
            let versions = (base.version_id, input1.version_id, input2.version_id);
            if merge.model.as_ref().is_none_or(|model| model.versions != versions) {
                let mut new = MergeModel::new(versions, base.document.lines.clone(), input1.document.lines.clone(), input2.document.lines.clone());
                new.alternative_version_id = result.alternative_version_id();
                merge.model = Some(new);
            }
            let model = merge.model.as_mut().expect("set above");
            model.follow_history(result.alternative_version_id());
            model.sync_result(&result.document.lines, result.version_id, true);
            true
        } else {
            false
        };
        self.editors.merges.insert(id, merge);
        synced
    }

    /// Applies a line edit of the active merge editor's model to its result, then takes the result's new diff.
    fn apply_merge_edit(&mut self, edit: LineEdit, cx: &mut Context<Self>) {
        self.run_indenting(
            |editor, _| {
                let (start, end, text) = text_edit(&edit, &editor.document.lines, editor.document.eol);
                editor.execute_edits(vec![(start, end, text, false)], None, EditReason::Unknown("mergeEditor")).into_iter().collect()
            },
            cx,
        );
        let result = self.editors.active().expect("the merge editor's result model");
        let (lines, version, alternative) = (result.document.lines.clone(), result.version_id, result.alternative_version_id());
        if let Some(model) = self.editors.active_merge_mut().and_then(|merge| merge.model.as_mut()) {
            model.sync_result(&lines, version, false);
            model.alternative_version_id = alternative;
        }
        cx.notify();
    }

    /// `MergeEditorViewModel.setState`: the range becomes active, the result takes the state's lines and the edit
    /// carries the range's handled states for undo.
    fn set_merge_state(&mut self, index: usize, state: State, cx: &mut Context<Self>) {
        if !self.sync_merge() {
            return;
        }
        let merge = self.editors.active_merge_mut().expect("synced above");
        merge.active_range = Some(index);
        let model = merge.model.as_mut().expect("synced above");
        let handled_before = model.handled[index];
        let edit = model.set_state(index, state);
        self.apply_merge_edit(edit, cx);
        let alternative = self.editors.active().expect("the merge editor's result model").alternative_version_id();
        if let Some(model) = self.active_merge_model_mut() {
            model.attach(alternative, index, handled_before);
        }
    }

    // shortcut: Ignore is not undoable; upstream pushes it on the result's undo stack, which carries text edits only here.
    fn run_merge_action(&mut self, action: MergeAction, cx: &mut Context<Self>) {
        match action {
            MergeAction::SetState(index, state) => self.set_merge_state(index, state, cx),
            MergeAction::Ignore(index, input) => {
                if let Some(model) = self.active_merge_model_mut() {
                    model.set_input_handled(index, input);
                }
                cx.notify();
            }
        }
    }

    /// `acceptAll`: every range takes `input` too.
    fn accept_all(&mut self, input: Input, cx: &mut Context<Self>) {
        if !self.sync_merge() {
            return;
        }
        let count = self.active_merge_model().map_or(0, |model| model.ranges.len());
        for index in 0..count {
            let state = self.active_merge_model().expect("synced above").accepted[index].with_input(input, true, false);
            self.set_merge_state(index, state, cx);
        }
    }

    /// `MergeEditorModel.reset` (Reset Result).
    fn reset_merge_result(&mut self, cx: &mut Context<Self>) {
        if !self.sync_merge() {
            return;
        }
        let line_count = self.editors.active().map_or(0, |result| result.document.lines.len());
        let edit = self.active_merge_model_mut().expect("synced above").reset(line_count);
        self.apply_merge_edit(edit, cx);
    }

    fn active_merge_model(&self) -> Option<&MergeModel> {
        self.editors.active_merge()?.model.as_ref()
    }

    fn active_merge_model_mut(&mut self) -> Option<&mut MergeModel> {
        self.editors.active_merge_mut()?.model.as_mut()
    }

    /// viewModel.ts `activeModifiedBaseRange`: the range at the result's cursor, else the one last acted on.
    fn active_merge_range(&self) -> Option<usize> {
        let model = self.active_merge_model()?;
        let line = self.editors.active()?.selection().head.line + 1;
        let at_cursor = (0..model.ranges.len()).find(|&index| {
            let range = model.result_line_range(model.ranges[index].base);
            range.contains(line) || (range.is_empty() && range.start == line)
        });
        at_cursor.or(self.editors.active_merge()?.active_range)
    }

    /// `toggleActiveConflict`.
    fn toggle_active_conflict(&mut self, input: Input, cx: &mut Context<Self>) -> Outcome {
        self.sync_merge();
        let Some(index) = self.active_merge_range() else { return Outcome::error("There is currently no conflict focused that can be toggled.") };
        let state = self.active_merge_model().expect("found a range").accepted[index].toggle(input);
        self.set_merge_state(index, state, cx);
        Outcome::done()
    }

    /// `goToNextModifiedBaseRange` and `goToPreviousModifiedBaseRange` of the unhandled ranges, in the result: the
    /// cursor goes to the range's first line, revealed near the top; wraps around.
    fn go_to_unhandled_conflict(&mut self, next: bool, cx: &mut Context<Self>) {
        self.sync_merge();
        let Some(model) = self.active_merge_model() else { return };
        let Some(result) = self.editors.active() else { return };
        let line = result.selection().head.line + 1;
        let unhandled: Vec<(usize, LineRange)> = (0..model.ranges.len()).filter(|&index| !model.is_handled(index)).map(|index| (index, model.result_line_range(model.ranges[index].base))).collect();
        let target = match next {
            true => unhandled.iter().find(|(_, range)| range.start > line).or(unhandled.first()),
            false => unhandled.iter().rev().find(|(_, range)| range.end < line).or(unhandled.last()),
        };
        let Some(&(index, range)) = target else { return };
        let viewport = self.editors.viewport;
        let editor = self.editors.active_mut().expect("found above");
        let target_line = (range.start - 1).min(editor.document.lines.len() - 1);
        let column = editor.document.lines[target_line].chars().position(|c| c != ' ' && c != '\t').unwrap_or(0);
        editor.move_to(Position::new(target_line, column), false);
        editor.reveal_range(Position::new(target_line, 0), Position::new(target_line, 0), VerticalReveal::NearTop, viewport);
        if let Some(merge) = self.editors.active_merge_mut() {
            merge.active_range = Some(index);
        }
        cx.notify();
    }

    /// commands.ts `AcceptMerge` (Complete Merge): with unhandled conflicts after a confirmation, saves the result,
    /// then closes the merge editor.
    fn accept_merge(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        self.sync_merge();
        let Some(merge) = self.editors.active_merge() else { return Outcome::done() };
        let result = merge.result.clone();
        let unhandled = merge.model.as_ref().is_some_and(|model| model.unhandled_count() > 0);
        let (sender, receiver) = futures::channel::oneshot::channel();
        cx.spawn_in(window, async move |workbench, cx| {
            let message = format!("Do you want to complete the merge of {}?", result.name());
            let detail = Some("The file contains unhandled conflicts.".to_string());
            if unhandled && !crate::workbench::explorer_actions::confirm(&workbench, cx, gpui::PromptLevel::Info, message, detail, "Complete with Conflicts").await {
                drop(sender.send(Ok(json!(false))));
                return;
            }
            let saved = workbench.update(cx, |workbench, cx| workbench.save_model(result.scheme.clone(), result.path.clone(), crate::workbench::auto_save::SaveReason::Explicit, cx));
            let saved = match saved {
                Ok(saved) => saved.await.unwrap_or(false),
                Err(_) => false,
            };
            let closed = workbench.update_in(cx, |workbench, window, cx| {
                let index = workbench.editors.editors.iter().position(|editor| workbench.editors.merges.get(&editor.model_id).is_some_and(|merge| merge.result == result));
                if let (true, Some(index)) = (saved, index) {
                    drop(workbench.execute_command("workbench.action.closeActiveEditor", vec![json!({ "editorIndex": index })], window, cx));
                }
            });
            drop(sender.send(closed.map(|_| json!(saved)).map_err(|_| exthost::main_thread::serialized_error("Canceled"))));
        })
        .detach();
        Outcome::Pending(receiver)
    }

    /// The context keys of an active merge editor (mergeEditor.ts `_ctx*`, common/mergeEditor.ts).
    pub(in crate::workbench) fn insert_merge_editor_context(&self, context: &mut std::collections::HashMap<String, Value>) {
        let Some(merge) = self.editors.active_merge() else { return };
        let layout = self.merge_layout();
        context.insert("activeEditor".into(), json!("mergeEditor"));
        context.insert("isMergeEditor".into(), json!(true));
        context.insert("isMergeResultEditor".into(), json!(true));
        context.insert("mergeEditorLayout".into(), json!(if layout.columns { "columns" } else { "mixed" }));
        context.insert("mergeEditorShowBase".into(), json!(layout.show_base));
        context.insert("mergeEditorShowBaseAtTop".into(), json!(layout.show_base_at_top));
        context.insert("mergeEditorShowNonConflictingChanges".into(), json!(self.merge_show_non_conflicting()));
        context.insert("mergeEditorBaseUri".into(), json!(crate::editor::uri_string(&merge.base.scheme, &merge.base.path)));
        context.insert("mergeEditorResultUri".into(), json!(crate::editor::uri_string(&merge.result.scheme, &merge.result.path)));
    }

    /// The merge editor's `navigation` actions of the editor title: Open File, previous and next unhandled conflict.
    pub(in crate::workbench) fn merge_title_actions(&self, cx: &mut Context<Self>) -> Vec<(f64, gpui::AnyElement)> {
        if self.editors.active_merge().is_none() {
            return Vec::new();
        }
        let actions = [
            (1., "merge.openResult", "go-to-file", "Open File"),
            (2., "merge.goToPreviousUnhandledConflict", "arrow-up", "Go to Previous Unhandled Conflict"),
            (3., "merge.goToNextUnhandledConflict", "arrow-down", "Go to Next Unhandled Conflict"),
        ];
        actions
            .into_iter()
            .map(|(order, command, icon, title)| (order, self.merge_toolbar_button(command, icon, title, cx).into_any_element()))
            .collect()
    }

    /// An action bar item: the command's icon with its hover.
    fn merge_toolbar_button(&self, command: &'static str, icon: &str, title: &'static str, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        self.icon(icon, 22., 16., self.color("icon.foreground"))
            .id(command)
            .rounded(px(5.))
            .cursor_pointer()
            .hover(|icon| icon.bg(self.color("toolbar.hoverBackground")))
            .on_click(cx.listener(move |workbench, _, window, cx| drop(workbench.execute_command(command, Vec::new(), window, cx))))
            .child(self.hover_target(gpui::SharedString::from(format!("merge-action-{command}")), HoverContent::Action { title, command }, HoverOptions::mouse()))
    }

    /// The editor title's "More Actions..." of a merge editor: its `1_merge`, `2_merge` and `3_merge` items.
    pub(in crate::workbench) fn show_merge_more_actions(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let layout = self.merge_layout();
        let item = |id: &str, title: &str, checked: bool| MenuAction::Command { id: id.into(), title: title.into(), enabled: true, checked, alt: None };
        let mut actions = vec![item("merge.mixedLayout", "Mixed Layout", !layout.columns), item("merge.columnLayout", "Column Layout", layout.columns), MenuAction::Separator];
        match layout.columns {
            true => actions.push(item("merge.showBase", "Show Base", layout.show_base)),
            false => {
                actions.push(item("merge.showBaseTop", "Show Base Top", layout.show_base && layout.show_base_at_top));
                actions.push(item("merge.showBaseCenter", "Show Base Center", layout.show_base && !layout.show_base_at_top));
            }
        }
        actions.push(MenuAction::Separator);
        actions.push(item("merge.showNonConflictingChanges", "Show Non-Conflicting Changes", self.merge_show_non_conflicting()));
        let context = self.key_context(window);
        self.show_context_menu_actions(actions, &context, gpui::Bounds::new(event.position, Default::default()), Vec::new(), window, cx);
    }

    /// viewZones.ts `computeViewZones` for the layout: the result lines up with the inputs in columns, base in the
    /// center of the mixed layout.
    fn merge_view_zones(&self) -> ViewZones {
        let (Some(merge), Some(model)) = (self.editors.active_merge(), self.active_merge_model()) else { return ViewZones::default() };
        let layout = self.merge_layout();
        let show_non_conflicting = self.merge_show_non_conflicting();
        let options = ZoneOptions { align_result: layout.columns, align_base: layout.show_base && !layout.columns && !layout.show_base_at_top, show_non_conflicting };
        model.view_zones(options, |index| input_actions(merge, model, Input::One, index, show_non_conflicting).is_empty() && input_actions(merge, model, Input::Two, index, show_non_conflicting).is_empty())
    }

    /// conflictActions.ts `codeLensHeight + 2` for actions, lines for spacers, the placeholder's 16px.
    fn merge_zone_height(&self, kind: ZoneKind) -> f32 {
        let line_height = self.editors.viewport.line_height;
        match kind {
            ZoneKind::Actions(_) => {
                let font_size = self.merge_code_lens_font_size();
                (font_size * (line_height / (font_size / 0.9)).max(1.3)).floor() + 2.
            }
            ZoneKind::Spacer(lines) => lines as f32 * line_height,
            ZoneKind::Placeholder => PLACEHOLDER_HEIGHT,
        }
    }

    /// conflictActions.ts `_getLayoutInfo`: 90% of the editor font size.
    fn merge_code_lens_font_size(&self) -> f32 {
        let font_size = self.setting("editor.fontSize").as_f64().unwrap_or(12.) as f32 * self.editor_zoom.multiplier();
        (font_size * 0.9).floor()
    }

    /// mergeEditor.ts `applyLayout`: mixed puts the inputs over the result (38:62) with base on top or between them;
    /// columns puts input 1, the result and input 2 beside each other with base above (40:60).
    pub(super) fn merge_editor_body(&self, merge: &MergeInput, result: &OpenEditor, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let layout = self.merge_layout();
        let border = self.color("surface.border");
        let zones = self.merge_view_zones();
        let scroll = self.merge_pane_scroll(merge, result, &zones, layout);
        let pane = |workbench: &Self, pane: Pane, window: &mut Window, cx: &mut Context<Self>| {
            let zones = match pane {
                Pane::Input(Input::One) => &zones.input1,
                Pane::Input(Input::Two) => &zones.input2,
                Pane::Base => &zones.base,
            };
            let scroll_top = match pane {
                Pane::Input(_) => scroll.0,
                Pane::Base => scroll.1,
            };
            workbench.merge_pane(merge, pane, zones, scroll_top, window, cx).flex_1().min_w_0().min_h_0()
        };
        let result_pane = div().flex().flex_col().min_h_0().min_w_0().child(self.merge_result_title(merge, result, cx)).child(self.merge_result_area(merge, result, window, cx));
        let base_on_top = |body: Div, window: &mut Window, cx: &mut Context<Self>, size: f32| match layout.show_base {
            true => body.child(pane(self, Pane::Base, window, cx).flex_none().h(relative(size)).border_b_1().border_color(border)),
            false => body,
        };
        let body = div().flex_1().min_h_0().flex().flex_col();
        if layout.columns {
            let base_size = 40. / 100.;
            let row = div()
                .flex_1()
                .min_h_0()
                .flex()
                .child(pane(self, Pane::Input(Input::One), window, cx))
                .child(result_pane.flex_1().border_l_1().border_color(border))
                .child(pane(self, Pane::Input(Input::Two), window, cx).border_l_1().border_color(border));
            return base_on_top(body, window, cx, base_size).child(row);
        }
        let with_base_on_top = layout.show_base && layout.show_base_at_top;
        let total = if with_base_on_top { 38. + 38. + 62. } else { 38. + 62. };
        let mut inputs = div().flex_none().h(relative(38. / total)).min_h_0().flex().child(pane(self, Pane::Input(Input::One), window, cx));
        if layout.show_base && !layout.show_base_at_top {
            inputs = inputs.child(pane(self, Pane::Base, window, cx).border_l_1().border_color(border));
        }
        inputs = inputs.child(pane(self, Pane::Input(Input::Two), window, cx).border_l_1().border_color(border));
        let body = if with_base_on_top { base_on_top(body, window, cx, 38. / total) } else { body };
        body.child(inputs).child(result_pane.flex_1().border_t_1().border_color(border))
    }

    /// scrollSynchronizer.ts from the result: in columns every pane at the result's scroll top; in the mixed layout
    /// the inputs at input 1's line beside the result's first line, base with them in the center, else at its own
    /// mapped line. Returns the inputs' and base's scroll tops.
    fn merge_pane_scroll(&self, merge: &MergeInput, result: &OpenEditor, zones: &ViewZones, layout: Layout) -> (f32, f32) {
        let viewport = self.editors.viewport;
        let locked = result.scroll.top - viewport.top_whitespace;
        let Some(model) = merge.model.as_ref().filter(|_| !layout.columns) else { return (locked, locked) };
        let first_view_line = result.view_line_at(result.scroll.top, viewport);
        let first_line = result.model_line(first_view_line) + 1;
        let offset = result.scroll.top - result.line_top(first_view_line, viewport);
        let input1_lines = self.editors.model(&merge.input1.side.scheme, &merge.input1.side.path).map_or(1, |model| model.document.lines.len());
        let input_line = model.input_line_for_result_line(Input::One, first_line).min(input1_lines);
        let inputs = self.merge_line_top(&zones.input1, input_line) + offset;
        if layout.show_base && !layout.show_base_at_top {
            return (inputs, inputs);
        }
        let base_line = model.base_line_for_result_line(first_line).min(model.base.len());
        (inputs, self.merge_line_top(&zones.base, base_line) + offset)
    }

    /// The top of 1-based `line` in a read-only pane, below the zones above it.
    fn merge_line_top(&self, zones: &[Zone], line: usize) -> f32 {
        let above: f32 = zones.iter().filter(|zone| zone.after_line < line).map(|zone| self.merge_zone_height(zone.kind)).sum();
        (line.max(1) - 1) as f32 * self.editors.viewport.line_height + above
    }

    /// The tops of a pane's zones, each below its line and the zones before it on that line.
    fn merge_zone_tops(&self, zones: &[Zone]) -> Vec<(f32, f32, ZoneKind)> {
        let line_height = self.editors.viewport.line_height;
        let mut sorted: Vec<&Zone> = zones.iter().collect();
        sorted.sort_by_key(|zone| zone.after_line);
        let mut above = 0.;
        sorted
            .into_iter()
            .map(|zone| {
                let height = self.merge_zone_height(zone.kind);
                let top = zone.after_line as f32 * line_height + above;
                above += height;
                (top, height, zone.kind)
            })
            .collect()
    }

    /// `.code-view > .title`: the label, its description and detail, with their `$(icon)`s as codicons.
    fn merge_title(&self, title: &str, description: Option<&str>, detail: Option<&str>) -> Div {
        let (foreground, dim) = (self.color("foreground"), self.color("descriptionForeground"));
        let label = |text: &str, color: Rgba| self.highlighted_label(text, &[], color, 14., color).text_color(color);
        div()
            .h(px(TITLE_HEIGHT))
            .flex_none()
            .px(px(10.))
            .flex()
            .items_center()
            .gap(px(6.))
            .overflow_hidden()
            .whitespace_nowrap()
            .text_size(px(13.))
            .child(label(title, foreground))
            .children(description.map(|description| label(description, dim)))
            .children(detail.map(|detail| label(detail, dim)))
    }

    /// resultCodeEditorView.ts title: "Result", the result's path, the remaining conflicts counter, and Complete
    /// Merge (the `MergeToolbar` button).
    fn merge_result_title(&self, merge: &MergeInput, result: &OpenEditor, cx: &mut Context<Self>) -> Div {
        let count = merge.model.as_ref().map_or(0, MergeModel::unhandled_count);
        let counter = if count == 1 { "1 Conflict Remaining".to_string() } else { format!("{count} Conflicts Remaining") };
        let path = crate::quick_input::path_label(&result.document.path, None);
        self.merge_title("Result", Some(&path), None)
            .child(
                div()
                    .id("merge-counter")
                    .ml_auto()
                    .text_color(self.color("descriptionForeground"))
                    .when(count > 0, |counter| counter.cursor_pointer().on_click(cx.listener(|workbench, _, _, cx| workbench.go_to_unhandled_conflict(true, cx))))
                    .child(counter),
            )
            // `MergeInputResultToolbar`: Reset.
            .child(self.merge_toolbar_button("mergeEditor.resetResultToBaseAndAutoMerge", "discard", "Reset Result", cx))
            .child(
                div()
                    .id("merge-complete")
                    .cursor_pointer()
                    .px(px(8.))
                    .rounded(px(2.))
                    .bg(self.color("button.background"))
                    .text_color(self.color("button.foreground"))
                    .child("Complete Merge")
                    .on_click(cx.listener(|workbench, _, window, cx| drop(workbench.execute_command("mergeEditor.acceptMerge", Vec::new(), window, cx)))),
            )
    }

    /// The result's code editor with each range's action row and spacer fill in its view zones and its block border.
    fn merge_result_area(&self, merge: &MergeInput, result: &OpenEditor, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let mut area = div().flex_1().min_h_0().relative().flex().flex_col().child(self.text_area(result, window, cx));
        let Some(model) = merge.model.as_ref() else { return area };
        let viewport = self.editors.viewport;
        let line_height = viewport.line_height;
        let line_numbers = self.setting("editor.lineNumbers") != json!("off");
        let content_left = EditorMetrics::measure(window, result.document.lines.len(), line_numbers, self.editor_zoom.multiplier()).content_left();
        let top = |index: usize| result.line_top(result.view_line(index), viewport) - result.scroll.top;
        let show_non_conflicting = self.merge_show_non_conflicting();
        let fill = self.theme.color("diffEditor.diagonalFill");
        for (position, (_, zone)) in merge.placed_zones.iter().enumerate() {
            let below: f32 = merge.placed_zones[position..].iter().filter(|(_, other)| other.after_line == zone.after_line).map(|(_, other)| self.merge_zone_height(other.kind)).sum();
            let zone_top = top(zone.after_line) - below;
            let height = self.merge_zone_height(zone.kind);
            let element = match zone.kind {
                ZoneKind::Actions(index) => self.merge_action_row(("merge-result-action", index), result_actions(merge, model, index), cx),
                ZoneKind::Spacer(_) | ZoneKind::Placeholder => div().relative().children(fill.map(|fill| diagonal_fill(fill).absolute().inset_0())),
            };
            area = area.child(element.absolute().left(px(content_left)).right_0().top(px(zone_top)).h(px(height)));
        }
        let active = self.active_merge_range();
        for (index, range) in model.ranges.iter().enumerate() {
            let lines = model.result_line_range(range.base);
            if !model.shows_actions(index, show_non_conflicting) || lines.start > result.document.lines.len() + 1 {
                continue;
            }
            let block_top = top(lines.start - 1);
            let height = if lines.is_empty() { 2. } else { top(lines.end - 2) + line_height - block_top };
            area = area.child(self.merge_block(model.is_handled(index), active == Some(index)).left(px(content_left)).right_0().top(px(block_top)).h(px(height)));
        }
        area
    }

    /// mergeEditor.css `.merge-editor-block`: 1px, focused and unhandled 2px.
    fn merge_block(&self, handled: bool, focused: bool) -> Div {
        let key = match (handled, focused) {
            (false, false) => "mergeEditor.conflict.unhandledUnfocused.border",
            (false, true) => "mergeEditor.conflict.unhandledFocused.border",
            (true, false) => "mergeEditor.conflict.handledUnfocused.border",
            (true, true) => "mergeEditor.conflict.handledFocused.border",
        };
        let block = div().absolute().border_color(self.color(key));
        if focused && !handled { block.border_2() } else { block.border_1() }
    }

    /// conflictActions.ts actions as a codelens-like row: links joined with " | ".
    fn merge_action_row(&self, id: (&'static str, usize), actions: Vec<Action>, cx: &mut Context<Self>) -> Div {
        let foreground = self.color("descriptionForeground");
        let link = self.color("textLink.foreground");
        let mut row = div().flex().items_end().whitespace_nowrap().text_size(px(self.merge_code_lens_font_size())).text_color(foreground);
        for (position, (label, action)) in actions.into_iter().enumerate() {
            if position > 0 {
                row = row.child("\u{a0}|\u{a0}");
            }
            row = match action {
                Some(action) => row.child(
                    div()
                        .id((id.0, id.1 * 16 + position))
                        .cursor_pointer()
                        .hover(move |item| item.text_color(link))
                        .on_click(cx.listener(move |workbench, _, _, cx| workbench.run_merge_action(action, cx)))
                        .child(label),
                ),
                None => row.child(label),
            };
        }
        row
    }

    /// inputCodeEditorView.ts and baseCodeEditorView.ts: the pane's title bar over its read-only lines, gutter
    /// checkboxes in an input, its zones and range blocks.
    fn merge_pane(&self, merge: &MergeInput, pane: Pane, zones: &[Zone], scroll_top: f32, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let (side, title) = match pane {
            Pane::Input(input) => {
                let data = if input == Input::One { &merge.input1 } else { &merge.input2 };
                let (accept_all, compare) = match input {
                    Input::One => ("merge.acceptAllInput1", "mergeEditor.compareInput1WithBase"),
                    Input::Two => ("merge.acceptAllInput2", "mergeEditor.compareInput2WithBase"),
                };
                let title = self.merge_title(&input_title(merge, input), data.description.as_deref(), data.detail.as_deref());
                let accept_title = if input == Input::One { "Accept All Incoming Changes from Left" } else { "Accept All Current Changes from Right" };
                // `MergeInput1Toolbar` and `MergeInput2Toolbar`: Accept All and Compare With Base.
                (&data.side, title.child(div().ml_auto().flex().child(self.merge_toolbar_button(accept_all, "check-all", accept_title, cx)).child(self.merge_toolbar_button(compare, "git-compare", "Compare With Base", cx))))
            }
            Pane::Base => (&merge.base, self.merge_title("Base", None, None)),
        };
        let focused = matches!(pane, Pane::Input(input) if merge.focused_input == Some(input));
        let container = div().flex().flex_col().child(title).when(focused, |pane| pane.border_1().border_color(self.color("focusBorder")));
        let (Some(editor), Some(model)) = (self.editors.model(&side.scheme, &side.path), merge.model.as_ref()) else { return container };
        container.child(self.merge_pane_lines(merge, model, pane, editor, zones, scroll_top, window, cx))
    }

    #[allow(clippy::too_many_arguments)]
    fn merge_pane_lines(&self, merge: &MergeInput, model: &MergeModel, pane: Pane, editor: &OpenEditor, zones: &[Zone], scroll_top: f32, window: &mut Window, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let metrics = EditorMetrics::measure(window, editor.document.lines.len(), true, self.editor_zoom.multiplier());
        let line_height = metrics.line_height;
        let gutter = if matches!(pane, Pane::Input(_)) { GUTTER_WIDTH } else { 0. };
        let content_left = gutter + metrics.glyph_margin_width() + metrics.line_numbers_width + metrics.decorations_width;
        let height = self.editors.viewport.height;
        let show_non_conflicting = self.merge_show_non_conflicting();
        let active = self.active_merge_range();
        let mut rows = Vec::new();
        let line_count = editor.document.lines.len();
        let mut line = 1;
        while line < line_count && self.merge_line_top(zones, line + 1) - scroll_top < 0. {
            line += 1;
        }
        while line <= line_count {
            let top = self.merge_line_top(zones, line) - scroll_top;
            if top > height {
                break;
            }
            rows.push(self.merge_pane_line(model, pane, editor, line - 1, metrics, gutter).absolute().top(px(top)).left_0().right_0());
            line += 1;
        }
        let fill = self.theme.color("diffEditor.diagonalFill");
        for (top, zone_height, kind) in self.merge_zone_tops(zones) {
            let top = top - scroll_top;
            if top + zone_height < 0. || top > height {
                continue;
            }
            let element = match (kind, pane) {
                (ZoneKind::Actions(index), Pane::Input(input)) => {
                    let id = if input == Input::One { "merge-input1-action" } else { "merge-input2-action" };
                    self.merge_action_row((id, index), input_actions(merge, model, input, index, show_non_conflicting), cx)
                }
                (ZoneKind::Spacer(_), _) => div().relative().children(fill.map(|fill| diagonal_fill(fill).absolute().inset_0())),
                _ => div(),
            };
            rows.push(element.absolute().left(px(content_left)).right_0().top(px(top)).h(px(zone_height)));
        }
        for (index, range) in model.ranges.iter().enumerate() {
            let (lines, handled) = match pane {
                Pane::Input(input) => {
                    let handled = if input == Input::One { model.handled[index].0 } else { model.handled[index].1 };
                    (range.range(input), handled)
                }
                Pane::Base => (range.base, model.is_handled(index)),
            };
            if !model.shows_actions(index, show_non_conflicting) {
                continue;
            }
            let block_top = self.merge_line_top(zones, lines.start) - scroll_top;
            let block_height = if lines.is_empty() { 2. } else { self.merge_line_top(zones, lines.end - 1) + line_height - scroll_top - block_top };
            if block_top + block_height < 0. || block_top > height {
                continue;
            }
            rows.push(self.merge_block(handled, active == Some(index)).left(px(content_left)).right_0().top(px(block_top)).h(px(block_height)));
            if let Pane::Input(input) = pane
                && !range.diffs(input).is_empty()
            {
                rows.push(self.merge_checkbox(model, index, input, lines, handled, active == Some(index), block_height, cx).absolute().left_0().top(px(block_top)));
            }
        }
        let id = match pane {
            Pane::Input(Input::One) => "merge-input1",
            Pane::Input(Input::Two) => "merge-input2",
            Pane::Base => "merge-base",
        };
        div()
            .id(id)
            .flex_1()
            .min_h_0()
            .relative()
            .overflow_hidden()
            .font_family(super::EDITOR_FONT)
            .text_size(px(metrics.font_size))
            .line_height(px(line_height))
            .bg(self.color("editor.background"))
            .text_color(self.color("editor.foreground"))
            .on_scroll_wheel(cx.listener(|workbench, event: &ScrollWheelEvent, window, cx| workbench.editor_wheel(event, window, cx)))
            .children(rows)
    }

    /// inputCodeEditorView.ts `MergeConflictGutterItemView`: a checkbox at the range's first line, a bracket along a
    /// longer range; checking it adds the input to the range's state, unchecking removes it.
    #[allow(clippy::too_many_arguments)]
    fn merge_checkbox(&self, model: &MergeModel, index: usize, input: Input, lines: LineRange, handled: bool, focused: bool, block_height: f32, cx: &mut Context<Self>) -> Div {
        let state = model.accepted[index];
        let checked = state.includes(input);
        let second = matches!(state, State::Both { first, .. } if first != input) && model.is_order_relevant(index);
        let (icon, title) = match (checked, second) {
            (false, _) => (None, "Accept"),
            (true, false) => (Some("check"), "Undo accept"),
            (true, true) => (Some("check-all"), "Undo accept (currently second)"),
        };
        let size = 18.;
        let border = if focused || !handled { self.merge_block(handled, focused) } else { div().absolute().border_color(self.color("checkbox.border")).border_2() };
        let multi_line = lines.len() > 1;
        let target_state = state.with_input(input, !checked, false);
        let id = if input == Input::One { ("merge-checkbox1", index) } else { ("merge-checkbox2", index) };
        div()
            .w(px(GUTTER_WIDTH))
            .h(px(block_height.max(size)))
            .when(multi_line, |marker| marker.child(border.border_r_0().rounded_l(px(3.)).left(px(12.)).w(px(10.)).top_0().bottom_0()))
            .child(
                div()
                    .id(id)
                    .absolute()
                    .top_0()
                    .left(px(7.))
                    .size(px(size))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(3.))
                    .border_1()
                    .border_color(self.color("checkbox.border"))
                    .bg(self.color("checkbox.background"))
                    .cursor_pointer()
                    .children(icon.map(|icon| self.icon(icon, size - 2., 14., self.color("checkbox.foreground"))))
                    .on_click(cx.listener(move |workbench, _, _, cx| workbench.set_merge_state(index, target_state, cx)))
                    .child(self.hover_target(gpui::SharedString::from(format!("{}-{}", id.0, id.1)), HoverContent::Text(title.into()), HoverOptions::mouse())),
            )
    }

    /// One read-only line: number, and in an input that shows its changes `mergeEditor.change.background` on changed
    /// lines and `mergeEditor.change.word.background` on their changed characters, then the text.
    fn merge_pane_line(&self, model: &MergeModel, pane: Pane, editor: &OpenEditor, index: usize, metrics: EditorMetrics, gutter: f32) -> Div {
        let line_number = index + 1;
        let text = &editor.document.lines[index];
        let tab_size = editor.document.indentation.tab_size;
        let mut content = div().absolute().top_0().bottom_0().right_0().left(px(gutter + metrics.glyph_margin_width() + metrics.line_numbers_width + metrics.decorations_width));
        if let Pane::Input(input) = pane {
            let shown = model.ranges.iter().enumerate().filter(|(index, range)| range.is_conflicting() || !model.is_handled(*index));
            let change = shown.flat_map(|(_, range)| range.diffs(input)).find(|diff| diff.modified.contains(line_number));
            if let Some(change) = change {
                content = content.child(div().absolute().inset_0().bg(self.color("mergeEditor.change.background")));
                let word = self.color("mergeEditor.change.word.background");
                for (left, width) in char_ranges(change, ZoneSide::Modified, line_number, text, tab_size, metrics.char_width) {
                    content = content.child(div().absolute().top_0().h_full().left(px(left)).w(px(width)).bg(word));
                }
            }
        }
        let highlights = editor.document.tokens.highlights(&self.theme, &self.languages, &editor.document.lines, index, tab_size);
        content = content.child(div().absolute().top_0().left_0().whitespace_nowrap().child(StyledText::new(expand_tabs(text, tab_size)).with_highlights(highlights)));
        div()
            .h(px(metrics.line_height))
            .flex()
            .child(div().w(px(gutter + metrics.glyph_margin_width())).flex_none())
            .child(div().w(px(metrics.line_numbers_width)).flex_none().text_right().text_color(self.color("editorLineNumber.foreground")).child(line_number.to_string()))
            .child(content)
    }
}

/// common/mergeEditor.ts `StorageCloseWithConflicts`.
const CLOSE_WITH_CONFLICTS_KEY: &str = "mergeEditorCloseWithConflicts";

/// `JSON.stringify(contents, undefined, 4)`.
fn pretty_json(value: &Value) -> String {
    use serde::Serialize;
    let mut buffer = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut buffer, serde_json::ser::PrettyFormatter::with_indent(b"    "));
    value.serialize(&mut serializer).expect("a JSON value serializes");
    String::from_utf8(buffer).expect("serde_json writes UTF-8")
}

fn file_uri(path: &Path) -> Value {
    crate::editor::uri_value("file", path)
}

/// The `_open.mergeEditor` argument of the dev commands' files: inputs labeled "Input 1" and "Input 2".
fn merge_args(base: Value, input1: Value, input2: Value, result: Value, detail: &str) -> Value {
    let input = |uri: Value, label: &str| json!({ "uri": uri, "title": label, "description": label, "detail": detail });
    json!({ "base": base, "input1": input(input1, "Input 1"), "input2": input(input2, "Input 2"), "output": result })
}

/// devCommands.ts `promptOpenInitial`: the `resultState` argument, else the user's pick of the current or the initial
/// result.
// shortcut: a dialog with both choices stands in for upstream's quick pick; the port has no awaitable quick pick.
async fn open_initial_result(workbench: &gpui::WeakEntity<Workbench>, cx: &mut gpui::AsyncWindowContext, result_state: Option<&str>) -> bool {
    if let Some(state) = result_state {
        return state == "initial";
    }
    let answer = workbench.update_in(cx, |workbench, window, cx| match workbench.skip_dialogs {
        true => None,
        false => Some(window.prompt(gpui::PromptLevel::Info, "Open the result or the initial result?", None, &["result", "initial result"], cx)),
    });
    match answer {
        Ok(Some(answer)) => answer.await == Ok(1),
        _ => false,
    }
}

/// `MergeEditorInputData.title`, else "Input 1" or "Input 2".
fn input_title(merge: &MergeInput, input: Input) -> String {
    let (data, fallback) = if input == Input::One { (&merge.input1, "Input 1") } else { (&merge.input2, "Input 2") };
    data.title.clone().unwrap_or_else(|| fallback.into())
}

/// conflictActions.ts `getItemsInput`: Accept (or Append), Accept Combination, Ignore.
fn input_actions(merge: &MergeInput, model: &MergeModel, input: Input, index: usize, show_non_conflicting: bool) -> Vec<Action> {
    let state = model.accepted[index];
    let mut actions = Vec::new();
    let handled = if input == Input::One { model.handled[index].0 } else { model.handled[index].1 };
    if (!model.ranges[index].is_conflicting() && model.is_handled(index) && !show_non_conflicting) || state == State::Unrecognized || state.includes(input) {
        return actions;
    }
    let title = input_title(merge, input);
    // `shouldUseAppendInsteadOfAccept` is off by default: the other input's acceptance shows Accept, not Append.
    actions.push((format!("Accept {title}"), Some(MergeAction::SetState(index, state.with_input(input, true, false)))));
    if model.can_be_combined(index) {
        let label = if model.is_order_relevant(index) { format!("Accept Combination ({title} First)") } else { "Accept Combination".into() };
        actions.push((label, Some(MergeAction::SetState(index, State::Base.with_input(input, true, false).with_input(input.other(), true, true)))));
    }
    if !handled {
        actions.push(("Ignore".into(), Some(MergeAction::Ignore(index, input))));
    }
    actions
}

/// conflictActions.ts `resultItems`: what the range takes, Remove per included input, Reset to base.
fn result_actions(merge: &MergeInput, model: &MergeModel, index: usize) -> Vec<Action> {
    let state = model.accepted[index];
    let mut actions: Vec<Action> = Vec::new();
    let mut inputs = vec![Input::One, Input::Two];
    if let State::Both { first: Input::Two, .. } = state {
        inputs.reverse();
    }
    match state {
        State::Unrecognized => actions.push(("Manual Resolution".into(), None)),
        State::Base => actions.push(("No Changes Accepted".into(), None)),
        _ => actions.push((inputs.iter().filter(|input| state.includes(**input)).map(|input| input_title(merge, *input)).collect::<Vec<_>>().join(" + "), None)),
    }
    for input in inputs {
        if state.includes(input) {
            actions.push((format!("Remove {}", input_title(merge, input)), Some(MergeAction::SetState(index, state.with_input(input, false, false)))));
        }
    }
    if state == State::Unrecognized {
        actions.push(("Reset to base".into(), Some(MergeAction::SetState(index, State::Base))));
    }
    actions
}

#[cfg(test)]
mod tests;
