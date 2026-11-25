//! The text diff editor (diffEditorWidget.ts and textDiffEditor.ts): the original model read-only on the left, the
//! modified model in the group's code editor on the right (inline: the deleted lines as view zones of that editor),
//! line and character decorations (diffEditorDecorations.ts), the alignment view zones (diffEditorViewZones.ts) and
//! the two-lane overview ruler (overviewRulerFeature.ts). Opened by `_workbench.diff` (editorCommands.ts).

use std::path::Path;

use diff::LineRange;
use gpui::{Context, Div, PathBuilder, Rgba, ScrollWheelEvent, StyledText, Window, canvas, div, fill, point, prelude::*, px, size};
use serde_json::{Value, json};

use super::unchanged_regions_view::PLACEHOLDER_HEIGHT;
use super::{EditorMetrics, LineNumbers, Workbench};
use crate::workbench::hover::{HoverContent, HoverOptions};
use crate::commands::Outcome;
use crate::diff_editor::{DIFF_EDITOR_SCHEME, DiffInput, DiffZone, Side, ZoneContent, ZoneSide, diff_options, range_alignment, view_zones};
use crate::editor::{OpenEditor, Position, TextDocument, VerticalReveal, expand_tabs, visible_column};
use crate::workbench::host_requests::{range, uri_parts};

/// textDiffEditor.ts `lineDecorationsWidth: '2ch'`, in whole pixels as `EditorIntOption.clampedInt` rounds it.
pub(crate) fn diff_decorations_width(char_width: f32) -> f32 {
    (2. * char_width).floor()
}

/// overviewRulerFeature.ts `ENTIRE_DIFF_OVERVIEW_WIDTH`: two 15px lanes, the original's left of the modified's.
const DIFF_OVERVIEW_WIDTH: f32 = 30.;
const ONE_OVERVIEW_WIDTH: f32 = 15.;
/// diffEditorOptions.ts `renderSideBySideInlineBreakpoint`.
const INLINE_BREAKPOINT: f32 = 900.;
/// diffEditorSash.ts `MINIMUM_EDITOR_WIDTH`.
const MINIMUM_EDITOR_WIDTH: f32 = 100.;

impl Workbench {
    pub(in crate::workbench) fn register_diff_editor_commands(&mut self) {
        self.commands.register("_workbench.diff", |workbench, args, window, cx| workbench.open_diff_editor(args, window, cx));
        self.commands.register("workbench.action.compareEditor.nextChange", |workbench, _, _, cx| {
            workbench.navigate_diff(true, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.compareEditor.previousChange", |workbench, _, _, cx| {
            workbench.navigate_diff(false, cx);
            Outcome::done()
        });
        self.commands.register("toggle.diff.renderSideBySide", |workbench, _, _, cx| {
            let side_by_side = workbench.setting("diffEditor.renderSideBySide") != json!(false);
            workbench.update_setting("diffEditor.renderSideBySide", json!(!side_by_side));
            cx.notify();
            Outcome::done()
        });
        for (id, side_by_side, automatic) in [("diffEditor.setViewMode.inline", false, None), ("diffEditor.setViewMode.sideBySide", true, Some(false)), ("diffEditor.setViewMode.automatic", true, Some(true))] {
            self.commands.register(id, move |workbench, _, _, cx| {
                workbench.update_setting("diffEditor.renderSideBySide", json!(side_by_side));
                if let Some(automatic) = automatic {
                    workbench.update_setting("diffEditor.useInlineViewWhenSpaceIsLimited", json!(automatic));
                }
                cx.notify();
                Outcome::done()
            });
        }
        self.commands.register("workbench.action.compareEditor.swapSides", |workbench, _, _, cx| {
            workbench.swap_diff_sides();
            cx.notify();
            Outcome::done()
        });
        // shortcut: the original pane has no cursor (one code editor per group), so the modified side opens and
        // `diffEditor.switchSide`, `compareEditor.focus*Side` are missing; add them when diff panes get editors.
        self.commands.register("workbench.action.compareEditor.openSide", |workbench, _, window, cx| {
            let Some(modified) = workbench.editors.active_diff().map(|diff| crate::editor::uri_value(&diff.modified.scheme, &diff.modified.path)) else { return Outcome::done() };
            workbench.execute_command("vscode.open", vec![modified], window, cx)
        });
        self.commands.register("diffEditor.toggleCollapseUnchangedRegions", |workbench, _, _, cx| {
            let enabled = workbench.setting("diffEditor.hideUnchangedRegions.enabled") == json!(true);
            workbench.update_setting("diffEditor.hideUnchangedRegions.enabled", json!(!enabled));
            cx.notify();
            Outcome::done()
        });
        self.commands.register("toggle.diff.ignoreTrimWhitespace", |workbench, _, _, cx| {
            let ignore = workbench.setting("diffEditor.ignoreTrimWhitespace") != json!(false);
            workbench.update_setting("diffEditor.ignoreTrimWhitespace", json!(!ignore));
            cx.notify();
            Outcome::done()
        });
    }

    /// editorCommands.ts `_workbench.diff`: `(original, modified, labelOrDescription?, [column?, options?])`. Both
    /// models open first, a missing one from its content or file system provider.
    fn open_diff_editor(&mut self, args: Vec<Value>, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let original = args.first().cloned().unwrap_or(Value::Null);
        let modified = args.get(1).cloned().unwrap_or(Value::Null);
        let sides = match (uri_parts(&original), uri_parts(&modified)) {
            (Ok((original_scheme, original_path)), Ok((modified_scheme, modified_path))) => (Side { scheme: original_scheme.to_string(), path: original_path }, Side { scheme: modified_scheme.to_string(), path: modified_path }),
            (Err(error), _) | (_, Err(error)) => return Outcome::Ready(Err(error)),
        };
        let label = match args.get(2) {
            Some(Value::String(label)) => Some(label.clone()),
            Some(Value::Object(labels)) => labels.get("label").and_then(Value::as_str).map(str::to_string),
            _ => None,
        };
        // shortcut: models key on scheme and path, so two `git:` refs of one file (index, HEAD) share a model; until text models keep the URI query.
        let column_and_options = args.get(3).cloned().unwrap_or_default();
        let mut options = column_and_options[1].clone();
        if !column_and_options[0].is_null() {
            options["position"] = column_and_options[0].clone();
        }
        let missing: Vec<Value> = [(&original, &sides.0), (&modified, &sides.1)]
            .into_iter()
            .filter(|(_, side)| side.scheme != "file" && self.editors.model(&side.scheme, &side.path).is_none())
            .map(|(uri, _)| uri.clone())
            .collect();
        let contents: Vec<_> = missing.iter().map(|uri| (uri.clone(), self.host.call("workbench", "resolveTextContent", vec![uri.clone()]))).collect();
        let (sender, receiver) = futures::channel::oneshot::channel();
        cx.spawn_in(window, async move |workbench, cx| {
            let mut resolved = Vec::new();
            for (uri, content) in contents {
                resolved.push((uri, content.await.unwrap_or_else(|_| Err(exthost::main_thread::serialized_error("Canceled")))));
            }
            let result = workbench.update_in(cx, |workbench, window, cx| {
                for (uri, content) in resolved {
                    // textModelResolverService.ts shares one resolution per uri: a model a concurrent call opened
                    // stands, even once the provider it came from is gone.
                    let (scheme, path) = uri_parts(&uri)?;
                    if workbench.editors.model(scheme, &path).is_none() {
                        workbench.try_open_document(&uri, &Value::Null, &content?, false, cx)?;
                    }
                }
                for (uri, side) in [(&original, &sides.0), (&modified, &sides.1)] {
                    if side.scheme == "file" && workbench.editors.model(&side.scheme, &side.path).is_none() {
                        workbench.try_open_document(uri, &Value::Null, &Value::Null, false, cx)?;
                    }
                }
                workbench.show_diff_editor(sides.0, sides.1, label, &options, window, cx);
                Ok(Value::Null)
            });
            drop(sender.send(result.unwrap_or_else(|_| Err(exthost::main_thread::serialized_error("Canceled")))));
        })
        .detach();
        Outcome::Pending(receiver)
    }

    /// `openEditor` of a `DiffEditorInput`: an open diff editor of the same sides is activated, else a new one
    /// opens (a preview unless `pinned` or `preview: false`).
    pub(in crate::workbench) fn show_diff_editor(&mut self, original: Side, modified: Side, label: Option<String>, options: &Value, window: &mut Window, cx: &mut Context<Self>) {
        self.activate_column(&options["position"]);
        let existing = self.editors.editors.iter().position(|editor| self.editors.diffs.get(&editor.model_id).is_some_and(|diff| diff.original == original && diff.modified == modified));
        let pinned = options["pinned"] == json!(true) || options["preview"] == json!(false);
        match existing {
            Some(index) => {
                self.editors.activate(index);
                if pinned {
                    self.editors.pin(index);
                }
            }
            None => {
                let id = self.editors.diffs.len() + self.editors.editors.len() + 1;
                let entry = TextDocument::from_text(DIFF_EDITOR_SCHEME, Path::new(&format!("/{id}")), "", "utf8".into(), Some("plaintext"), &self.languages, &self.settings.lock().unwrap());
                self.editors.open_diff(entry, DiffInput::new(original, modified, label), !pinned);
            }
        }
        self.marked_text = None;
        self.update_diff_editor();
        if !options["selection"].is_null() {
            let viewport = self.editors.viewport;
            let editor = self.editors.active_mut().expect("the diff editor's modified model");
            let (start, end) = range(editor, &options["selection"]);
            editor.move_to(start, false);
            editor.move_to(end, true);
            editor.reveal_range(start, end, VerticalReveal::Center, viewport);
        } else if let Some(first) = self.editors.active_diff().and_then(|diff| diff.diff.as_ref()).and_then(|diff| diff.diff.changes.first().map(|change| change.modified.start)) {
            // textDiffEditor.ts `revealFirstDiff`: an editor without view state shows its first change.
            let viewport = self.editors.viewport;
            let editor = self.editors.active_mut().expect("the diff editor's modified model");
            let line = Position::new(first.saturating_sub(1).min(editor.document().lines.len() - 1), 0);
            match viewport.height == 0. {
                true => self.editors.reveal_after_layout = Some(line),
                false => editor.reveal_range(line, line, VerticalReveal::CenterIfOutsideViewport, viewport),
            }
        }
        if options["preserveFocus"] != json!(true) {
            window.focus(&self.editor_focus, cx);
        }
        self.editors_changed(cx);
    }

    /// Recomputes the active diff and places its view zones in the modified editor; other diff editors take theirs
    /// back, so a text editor of the same model shows none.
    pub(in crate::workbench) fn update_diff_editor(&mut self) {
        let options = diff_options(|key| self.setting(key));
        let side_by_side = self.diff_side_by_side();
        let active = self.editors.active_tab().map(|tab| tab.model_id);
        let ids: Vec<u32> = self.editors.diffs.keys().copied().collect();
        for id in ids {
            let mut input = self.editors.diffs.remove(&id).expect("listed above");
            let placed = std::mem::take(&mut input.placed_zones);
            let placed_unchanged = std::mem::take(&mut input.placed_unchanged);
            if let Some(modified) = self.editors.model_mut(&input.modified.scheme, &input.modified.path) {
                for zone in &placed {
                    modified.remove_view_zone(zone.0);
                }
                for zone in &placed_unchanged {
                    modified.remove_view_zone(zone.0);
                }
                modified.folding.set_hidden_areas(Vec::new());
            }
            if Some(id) == active
                && let (Some(original), Some(modified)) = (self.editors.model(&input.original.scheme, &input.original.path), self.editors.model(&input.modified.scheme, &input.modified.path))
            {
                input.update(original, modified, options);
                input.update_unchanged(original.document().lines.len(), modified.document().lines.len(), self.hide_unchanged_regions());
                input.inline_now = !side_by_side;
                let computed = input.diff.as_ref().expect("just computed");
                let alignments = range_alignment(&computed.diff, &original.document().lines, side_by_side);
                input.zones = view_zones(&computed.diff, &alignments, side_by_side);
                let line_height = self.editors.viewport.line_height;
                let modified = self.editors.model_mut(&input.modified.scheme, &input.modified.path).expect("found above");
                for zone in input.zones.iter().filter(|zone| zone.side == ZoneSide::Modified) {
                    let anchor = zone.after_line.checked_sub(1).map(|line| Position::new(line, 0));
                    let id = modified.add_view_zone(anchor, zone.lines as f32 * line_height);
                    input.placed_zones.push((id, zone.clone()));
                }
                self.place_unchanged_regions(&mut input);
            }
            self.editors.diffs.insert(id, input);
        }
    }

    /// diffEditorOptions.ts `renderSideBySide` with `useInlineViewWhenSpaceIsLimited` below the 900px breakpoint.
    pub(super) fn diff_side_by_side(&self) -> bool {
        let configured = self.setting("diffEditor.renderSideBySide") != json!(false);
        let limited = self.setting("diffEditor.useInlineViewWhenSpaceIsLimited") != json!(false);
        configured && !(limited && self.diff_part_width.get() > 0. && self.diff_part_width.get() < INLINE_BREAKPOINT)
    }

    /// mainThreadDocumentDiff.ts `$computeDocumentDiff`: the advanced diff of two open models, null when either is
    /// not open (editorWorker.ts `computeDiff` finds no model).
    pub(in crate::workbench) fn compute_document_diff(&self, original: &Value, modified: &Value, ignore_trim_whitespace: bool, max_computation_time_ms: u64, compute_moves: bool) -> Value {
        let model = |uri: &Value| uri_parts(uri).ok().and_then(|(scheme, path)| self.editors.model(scheme, &path));
        let (Some(original), Some(modified)) = (model(original), model(modified)) else { return Value::Null };
        let (original, modified) = (&original.document().lines, &modified.document().lines);
        let options = diff::DiffOptions { ignore_trim_whitespace, max_computation_time_ms, compute_moves, extend_to_subwords: false };
        let result = diff::compute_diff(original, modified, options);
        let line_range = |range: LineRange| json!({ "startLineNumber": range.start, "startColumn": 1, "endLineNumber": range.end, "endColumn": 1 });
        let text_range = |range: diff::Range, lines: &[String]| {
            let column = |position: diff::Position| lines.get(position.line - 1).map_or(position.column, |line| line.chars().take(position.column - 1).map(char::len_utf16).sum::<usize>() + 1);
            json!({ "startLineNumber": range.start.line, "startColumn": column(range.start), "endLineNumber": range.end.line, "endColumn": column(range.end) })
        };
        let change = |change: &diff::LineRangeMapping| {
            json!({
                "originalRange": line_range(change.original),
                "modifiedRange": line_range(change.modified),
                "innerChanges": change.inner_changes.as_ref().map(|inner| inner.iter().map(|i| json!({ "originalRange": text_range(i.original, original), "modifiedRange": text_range(i.modified, modified) })).collect::<Vec<_>>()),
            })
        };
        json!({
            "identical": result.changes.is_empty() && original == modified,
            "quitEarly": result.hit_timeout,
            "changes": result.changes.iter().map(change).collect::<Vec<_>>(),
            "moves": result.moves.iter().map(|m| json!({
                "originalRange": line_range(m.line_range_mapping.original),
                "modifiedRange": line_range(m.line_range_mapping.modified),
                "changes": m.changes.iter().map(change).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        })
    }

    /// editorGroupView.ts `activeCompareEditorCanSwap`: the original is writable and both sides are files or untitled.
    fn diff_can_swap(&self, input: &DiffInput) -> bool {
        let provided = |side: &Side| matches!(side.scheme.as_str(), "file" | "untitled");
        provided(&input.original) && provided(&input.modified) && self.readonly(&input.original.scheme, &input.original.path).is_none()
    }

    /// diffEditorCommandsService.ts `swapDiffSides`: the diff editor shows the modified side as the original.
    fn swap_diff_sides(&mut self) {
        let Some(id) = self.editors.active_tab().map(|tab| tab.model_id) else { return };
        let Some(input) = self.editors.diffs.get(&id).filter(|input| self.diff_can_swap(input)) else { return };
        let modified = input.modified.clone();
        let mut input = self.editors.diffs.remove(&id).expect("found above");
        if let Some(model) = self.editors.model_mut(&modified.scheme, &modified.path) {
            for (zone, _) in std::mem::take(&mut input.placed_zones) {
                model.remove_view_zone(zone);
            }
        }
        let swapped = DiffInput::new(input.modified, input.original, input.label);
        self.editors.diffs.insert(id, swapped);
        self.update_diff_editor();
    }

    /// The diff editor's `navigation` actions of the editor title with their menu orders: previous and next change,
    /// swap sides, whitespace and collapse unchanged regions.
    pub(in crate::workbench) fn diff_title_actions(&self, cx: &mut Context<Self>) -> Vec<(f64, gpui::AnyElement)> {
        let Some(input) = self.editors.active_diff() else { return Vec::new() };
        let has_changes = input.diff.as_ref().is_some_and(|diff| !diff.diff.changes.is_empty());
        let mut actions = vec![
            (10., "workbench.action.compareEditor.previousChange", "arrow-up", "Previous Change", has_changes),
            (11., "workbench.action.compareEditor.nextChange", "arrow-down", "Next Change", has_changes),
        ];
        if self.diff_can_swap(input) {
            actions.push((15., "workbench.action.compareEditor.swapSides", "arrow-swap", "Swap Left and Right Side", true));
        }
        actions.push((20., "toggle.diff.ignoreTrimWhitespace", "whitespace", "Show Leading/Trailing Whitespace Differences", true));
        actions.push((22., "diffEditor.toggleCollapseUnchangedRegions", "map", "Toggle Collapse Unchanged Regions", true));
        let icon_color = self.color("icon.foreground");
        let disabled_color = self.color("disabledForeground");
        actions
            .into_iter()
            .map(|(order, command, icon, title, enabled)| {
                let color = if enabled { icon_color } else { disabled_color };
                let element = self
                    .icon(icon, 22., 16., color)
                    .id(command)
                    .rounded(px(5.))
                    .when(enabled, |icon| {
                        icon.cursor_pointer().hover(|icon| icon.bg(self.color("toolbar.hoverBackground"))).on_click(cx.listener(move |workbench, _, window, cx| {
                            drop(workbench.execute_command(command, Vec::new(), window, cx));
                        }))
                    })
                    .child(self.hover_target(gpui::SharedString::from(format!("diff-title-{command}")), HoverContent::Action { title, command }, HoverOptions::mouse()))
                    .into_any_element();
                (order, element)
            })
            .collect()
    }

    /// The context keys of an active diff editor (editor.ts `TextCompareEditorActiveContext` and the diff
    /// editor's own `isInDiffEditor`).
    pub(in crate::workbench) fn insert_diff_editor_context(&self, context: &mut std::collections::HashMap<String, Value>) {
        let Some(input) = self.editors.active_diff() else { return };
        context.insert("activeEditor".into(), json!("workbench.editors.textDiffEditor"));
        context.insert("textCompareEditorActive".into(), json!(true));
        context.insert("textCompareEditorVisible".into(), json!(true));
        context.insert("isInDiffEditor".into(), json!(true));
        context.insert("diffEditorRenderSideBySideInlineBreakpointReached".into(), json!(input.inline_now && self.setting("diffEditor.renderSideBySide") != json!(false)));
        context.insert("diffEditorOriginalUri".into(), json!(crate::editor::uri_string(&input.original.scheme, &input.original.path)));
        context.insert("diffEditorModifiedUri".into(), json!(crate::editor::uri_string(&input.modified.scheme, &input.modified.path)));
        context.insert("activeCompareEditorCanSwap".into(), json!(self.diff_can_swap(input)));
        context.insert("editorHasChanges".into(), json!(input.diff.as_ref().is_some_and(|diff| !diff.diff.changes.is_empty())));
    }

    /// The tab label of a diff editor (`DiffEditorInput.getName`) or a multi diff editor (`MultiDiffEditorInput.getName`).
    pub(in crate::workbench) fn diff_name(&self, editor: &OpenEditor) -> Option<String> {
        self.editors.diffs.get(&editor.model_id).map(DiffInput::name).or_else(|| self.editors.multi_diffs.get(&editor.model_id).map(crate::multi_diff_editor::MultiDiffInput::name))
    }

    /// `navigateInDiffEditor`: the cursor goes to the next change's first line, wrapping around (diffNavigator).
    fn navigate_diff(&mut self, next: bool, cx: &mut Context<Self>) {
        let Some(changes) = self.editors.active_diff().and_then(|diff| diff.diff.as_ref()).map(|diff| diff.diff.changes.iter().map(|change| change.modified.start - 1).collect::<Vec<_>>()) else { return };
        if changes.is_empty() {
            return;
        }
        let viewport = self.editors.viewport;
        let editor = self.editors.active_mut().expect("the diff editor's modified model");
        let line = editor.selection().head.line;
        let target = match next {
            true => changes.iter().copied().find(|&start| start > line).unwrap_or(changes[0]),
            false => changes.iter().rev().copied().find(|&start| start < line).unwrap_or(*changes.last().expect("not empty")),
        };
        let target = target.min(editor.document().lines.len() - 1);
        editor.move_to(Position::new(target, 0), false);
        editor.reveal_range(Position::new(target, 0), Position::new(target, 0), VerticalReveal::CenterIfOutsideViewport, viewport);
        cx.notify();
    }

    /// The diff editor below the breadcrumbs: side by side the original pane, its sash and the modified code editor,
    /// inline the modified code editor alone, then the diff overview ruler.
    pub(super) fn diff_editor_body(&self, input: &DiffInput, modified: &OpenEditor, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let part_width = self.diff_part_width.clone();
        let measure = canvas(move |bounds, _, _| part_width.set(f32::from(bounds.size.width)), |_, _, _, _| {}).absolute().inset_0();
        let mut body = div().flex_1().min_h_0().relative().flex().child(measure);
        let original = self.editors.model(&input.original.scheme, &input.original.path);
        if !input.inline_now
            && let Some(original) = original
        {
            let width = self.diff_part_width.get();
            let sash_left = (width / 2.).floor().clamp(MINIMUM_EDITOR_WIDTH.min(width / 2.), (width - MINIMUM_EDITOR_WIDTH).max(width / 2.));
            // diffEditorWidget.ts: the gutter sits left of the sash, taken from the original editor.
            let gutter_width = self.diff_gutter_width(input);
            body = body.child(self.original_pane(input, original, modified, true, window, cx).w(px(sash_left - gutter_width)).flex_none());
            if gutter_width > 0. {
                body = body.child(self.diff_gutter(input, modified, cx));
            }
        } else {
            if self.diff_gutter_width(input) > 0. {
                body = body.child(self.diff_gutter(input, modified, cx));
            }
            if let Some(original) = original
                && self.setting("diffEditor.hideOriginalLineNumbers") != json!(true)
            {
                // diffEditorWidget.ts: inline, the original editor shows up to its decorations, its line numbers.
                let metrics = EditorMetrics::measure(window, original.document().lines.len(), LineNumbers::Off, self.editor_zoom.multiplier());
                let width = (metrics.glyph_margin_width() + metrics.line_numbers_width).max(5.);
                body = body.child(self.original_pane(input, original, modified, false, window, cx).w(px(width)).flex_none());
            }
        }
        let shadow = self.theme.color("scrollbar.shadow");
        let border = self.theme.color("diffEditor.border");
        // style.css `.side-by-side .editor.modified`: the shadow and border the modified editor casts on the original.
        let modified_pane = div()
            .flex_1()
            .min_w_0()
            .relative()
            .flex()
            .flex_col()
            .when(!input.inline_now, |pane| pane.when_some(border, |pane, border| pane.border_l_1().border_color(border)))
            .child(self.text_area(modified, window, cx))
            .when(!input.inline_now, |pane| pane.when_some(shadow, |pane, shadow| pane.child(div().absolute().top_0().bottom_0().left(px(-6.)).w(px(6.)).child(side_shadow(shadow)))));
        body.child(modified_pane).child(self.diff_overview(input, modified, cx).w(px(DIFF_OVERVIEW_WIDTH)).flex_none().h_full())
    }

    /// The original editor, read-only and scrolled with the modified one: its lines, the deleted line and character
    /// decorations, the `-` signs and the diagonal fill under the side with fewer lines.
    fn original_pane(&self, input: &DiffInput, original: &OpenEditor, modified: &OpenEditor, side_by_side: bool, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let metrics = EditorMetrics::measure(window, original.document().lines.len(), if side_by_side { LineNumbers::On } else { LineNumbers::Off }, self.editor_zoom.multiplier());
        let line_height = metrics.line_height;
        let decorations_width = diff_decorations_width(metrics.char_width);
        // `_adjustOptionsForLeftHandSide`: the glyph margin shows side by side.
        let content_left = metrics.glyph_margin_width() + metrics.line_numbers_width + decorations_width;
        let zones: Vec<&DiffZone> = input.zones.iter().filter(|zone| zone.side == ZoneSide::Original).collect();
        let scroll_top = modified.scroll.top;
        let height = self.editors.viewport.height;
        let mut rows = Vec::new();
        let mut top = self.editors.viewport.top_whitespace;
        for zone in zones.iter().filter(|zone| zone.after_line == 0) {
            rows.push(self.diff_zone_view(zone, line_height, content_left).absolute().top(px(top - scroll_top)).left_0().right_0());
            top += zone.lines as f32 * line_height;
        }
        // hideUnchangedRegionsFeature.ts: the original editor's hidden areas, each with its placeholder above.
        let hidden: Vec<(LineRange, usize)> = input.unchanged.iter().enumerate().filter(|(_, region)| !region.is_fully_revealed()).map(|(index, region)| (region.hidden_original(), index)).collect();
        let mut index = 0;
        while index < original.document().lines.len() {
            if top > scroll_top + height {
                break;
            }
            if let Some((range, region)) = hidden.iter().find(|(range, _)| range.start == index + 1) {
                if top + PLACEHOLDER_HEIGHT >= scroll_top {
                    rows.push(self.unchanged_placeholder(input, *region, ZoneSide::Original, content_left, !side_by_side, cx).absolute().top(px(top - scroll_top)).left_0().right_0());
                }
                top += PLACEHOLDER_HEIGHT;
                index = range.end - 1;
                continue;
            }
            let zones_below: f32 = zones.iter().filter(|zone| zone.after_line == index + 1).map(|zone| zone.lines as f32 * line_height).sum();
            if top + line_height + zones_below >= scroll_top {
                rows.push(self.original_line(input, original, index, metrics, decorations_width).absolute().top(px(top - scroll_top)).left_0().right_0());
                let mut zone_top = top + line_height;
                for zone in zones.iter().filter(|zone| zone.after_line == index + 1) {
                    rows.push(self.diff_zone_view(zone, line_height, content_left).absolute().top(px(zone_top - scroll_top)).left_0().right_0());
                    zone_top += zone.lines as f32 * line_height;
                }
            }
            top += line_height + zones_below;
            index += 1;
        }
        div()
            .h_full()
            .relative()
            .overflow_hidden()
            .font_family(super::EDITOR_FONT)
            .text_size(px(metrics.font_size))
            .line_height(px(line_height))
            .bg(self.color("editor.background"))
            .text_color(self.color("editor.foreground"))
            .children(rows)
    }

    /// One line of the original editor: margin background and sign, number, line and character backgrounds, text.
    fn original_line(&self, input: &DiffInput, original: &OpenEditor, index: usize, metrics: EditorMetrics, decorations_width: f32) -> Div {
        let line_number = index + 1;
        let changes = input.diff.as_ref().map_or(&[][..], |diff| &diff.diff.changes[..]);
        let change = changes.iter().find(|change| change.original.contains(line_number));
        let text = &original.document().lines[index];
        let tab_size = original.document().indentation.tab_size;
        let char_width = metrics.char_width;
        let mut content = div().absolute().top_0().bottom_0().right_0().left(px(metrics.glyph_margin_width() + metrics.line_numbers_width + decorations_width));
        content = content.children(self.unchanged_line_background(input, ZoneSide::Original, line_number));
        if let Some(change) = change {
            content = content.children(self.line_background(ZoneSide::Original));
            if !change.modified.is_empty() {
                content = content.children(char_ranges(change, ZoneSide::Original, line_number, text, tab_size, char_width).into_iter().map(|(left, width)| self.char_background(ZoneSide::Original, left, width)));
            }
        }
        for level in 0..original.text_model().guides.get(index).copied().unwrap_or(0) {
            let x = (level * original.document().indentation.indent_size()) as f32 * char_width;
            content = content.child(div().absolute().top_0().left(px(x)).w(px(1.)).h_full().bg(self.color("editorIndentGuide.background1")));
        }
        let highlights = original.document().tokens.highlights(&self.theme, &self.languages, &original.document().lines, index, tab_size);
        content = content.child(div().absolute().top_0().left_0().whitespace_nowrap().child(StyledText::new(expand_tabs(text, tab_size)).with_highlights(highlights)));
        div()
            .h(px(metrics.line_height))
            .flex()
            .when(change.is_some(), |row| row.children(self.margin_background(ZoneSide::Original, metrics.glyph_margin_width() + metrics.line_numbers_width + decorations_width)))
            .child(div().w(px(metrics.glyph_margin_width())).flex_none())
            .child(div().w(px(metrics.line_numbers_width)).flex_none().flex().justify_end().whitespace_nowrap().text_color(self.color("editorLineNumber.foreground")).child(line_number.to_string()))
            .child(div().w(px(decorations_width)).flex_none().when(change.is_some() && self.diff_render_indicators(), |lane| lane.child(self.diff_sign(ZoneSide::Original, metrics))))
            .child(content)
    }

    /// A zone's view: the diagonal fill, or the inline view's deleted lines.
    fn diff_zone_view(&self, zone: &DiffZone, line_height: f32, content_left: f32) -> Div {
        let height = zone.lines as f32 * line_height;
        match &zone.content {
            ZoneContent::Fill => {
                let color = self.theme.color("diffEditor.diagonalFill");
                div().h(px(height)).relative().children(color.map(|color| diagonal_fill(color).absolute().top_0().bottom_0().right_0().left(px(content_left))))
            }
            ZoneContent::Deleted { original, change } => self.deleted_lines(*original, *change, line_height, content_left),
        }
    }

    /// A view zone of the modified code editor: a diff zone's view, an unchanged region's placeholder, else empty
    /// whitespace.
    pub(super) fn view_zone_body(&self, editor: &OpenEditor, id: u32, height: f32, content_left: f32, cx: &mut Context<Self>) -> Div {
        let Some(input) = self.editors.active_diff().filter(|diff| editor.document().is(&diff.modified.scheme, &diff.modified.path)) else {
            return div().flex_none().h(px(height));
        };
        if let Some((_, region)) = input.placed_unchanged.iter().find(|placed| placed.0 == id) {
            return self.unchanged_placeholder(input, *region, ZoneSide::Modified, content_left, false, cx);
        }
        match input.placed_zones.iter().find(|placed| placed.0 == id) {
            Some((_, zone)) => self.diff_zone_view(zone, self.editors.viewport.line_height, content_left).flex_none(),
            None => div().flex_none().h(px(height)),
        }
    }

    /// inlineDiffDeletedCodeMargin.ts and renderLines.ts: the original's deleted lines with their margin and signs.
    fn deleted_lines(&self, lines: LineRange, change: usize, line_height: f32, content_left: f32) -> Div {
        let Some(input) = self.editors.active_diff() else { return div() };
        let (Some(original), Some(computed)) = (self.editors.model(&input.original.scheme, &input.original.path), input.diff.as_ref()) else { return div() };
        let change = &computed.diff.changes[change];
        let tab_size = original.document().indentation.tab_size;
        let char_width = self.editors.viewport.char_width;
        let mut block = div().h(px(lines.len() as f32 * line_height)).relative();
        for (row, line_number) in (lines.start..lines.end).enumerate() {
            let document = original.document();
            let Some(text) = document.lines.get(line_number - 1) else { break };
            let mut content = div().absolute().top_0().bottom_0().right_0().left(px(content_left)).children(self.line_background(ZoneSide::Original));
            if !change.modified.is_empty() {
                content = content.children(char_ranges(change, ZoneSide::Original, line_number, text, tab_size, char_width).into_iter().map(|(left, width)| self.char_background(ZoneSide::Original, left, width)));
            }
            let highlights = original.document().tokens.highlights(&self.theme, &self.languages, &original.document().lines, line_number - 1, tab_size);
            content = content.child(div().absolute().top_0().left_0().whitespace_nowrap().child(StyledText::new(expand_tabs(text, tab_size)).with_highlights(highlights)));
            let metrics = EditorMetrics { font_size: 0., line_height, char_width, line_numbers: LineNumbers::Off, line_numbers_width: 0., decorations_width: 0. };
            let sign_left = content_left - diff_decorations_width(char_width);
            block = block.child(
                div()
                    .absolute()
                    .top(px(row as f32 * line_height))
                    .left_0()
                    .right_0()
                    .h(px(line_height))
                    .children(self.margin_background(ZoneSide::Original, content_left))
                    .when(self.diff_render_indicators(), |line| line.child(div().absolute().top_0().left(px(sign_left)).child(self.diff_sign(ZoneSide::Original, metrics))))
                    .child(content),
            );
        }
        block
    }

    /// The modified editor's line and character decorations of the active diff (`line-insert`, `char-insert`).
    pub(super) fn diff_line_decorations(&self, editor: &OpenEditor, index: usize, char_width: f32, scroll_left: f32) -> Vec<Div> {
        let Some(change) = self.modified_change(editor, index) else {
            let input = self.editors.active_diff().filter(|diff| editor.document().is(&diff.modified.scheme, &diff.modified.path));
            return input.and_then(|input| self.unchanged_line_background(input, ZoneSide::Modified, index + 1)).map(|line| line.left(px(scroll_left))).into_iter().collect();
        };
        let text = &editor.document().lines[index];
        let mut decorations: Vec<Div> = self.line_background(ZoneSide::Modified).into_iter().map(|line| line.left(px(scroll_left))).collect();
        if !change.original.is_empty() {
            for (left, width) in char_ranges(change, ZoneSide::Modified, index + 1, text, editor.document().indentation.tab_size, char_width) {
                decorations.push(self.char_background(ZoneSide::Modified, left, width));
            }
        }
        decorations
    }

    /// The modified editor's margin of a changed line: `gutter-insert` and the `+` sign.
    pub(super) fn diff_margin(&self, editor: &OpenEditor, index: usize, metrics: EditorMetrics) -> Vec<Div> {
        if self.modified_change(editor, index).is_none() {
            return Vec::new();
        }
        let mut margin: Vec<Div> = self.margin_background(ZoneSide::Modified, metrics.content_left()).into_iter().collect();
        if self.diff_render_indicators() {
            margin.push(div().absolute().top_0().left(px(metrics.glyph_margin_width() + metrics.line_numbers_width)).child(self.diff_sign(ZoneSide::Modified, metrics)));
        }
        margin
    }

    /// The change a line of the active diff's modified model is in.
    fn modified_change(&self, editor: &OpenEditor, index: usize) -> Option<&diff::LineRangeMapping> {
        let input = self.editors.active_diff().filter(|diff| editor.document().is(&diff.modified.scheme, &diff.modified.path))?;
        input.diff.as_ref()?.diff.changes.iter().find(|change| change.modified.contains(index + 1))
    }

    pub(super) fn diff_render_indicators(&self) -> bool {
        self.setting("diffEditor.renderIndicators") != json!(false)
    }

    /// style.css `.line-insert`, `.line-delete`: the whole line, with the text border in high contrast (dashed).
    pub(super) fn line_background(&self, side: ZoneSide) -> Option<Div> {
        let (line, text, border) = match side {
            ZoneSide::Modified => ("diffEditor.insertedLineBackground", "diffEditor.insertedTextBackground", "diffEditor.insertedTextBorder"),
            ZoneSide::Original => ("diffEditor.removedLineBackground", "diffEditor.removedTextBackground", "diffEditor.removedTextBorder"),
        };
        let background = self.theme.color(line).or_else(|| self.theme.color(text));
        let border = self.theme.color(border);
        if background.is_none() && border.is_none() {
            return None;
        }
        let mut decoration = div().absolute().inset_0();
        if let Some(background) = background {
            decoration = decoration.bg(background);
        }
        if let Some(border) = border {
            decoration = decoration.border_1().border_dashed().border_color(border);
        }
        Some(decoration)
    }

    /// style.css `.char-insert`, `.char-delete`.
    pub(super) fn char_background(&self, side: ZoneSide, left: f32, width: f32) -> Div {
        let (text, border) = match side {
            ZoneSide::Modified => ("diffEditor.insertedTextBackground", "diffEditor.insertedTextBorder"),
            ZoneSide::Original => ("diffEditor.removedTextBackground", "diffEditor.removedTextBorder"),
        };
        let mut decoration = div().absolute().top_0().h_full().left(px(left)).w(px(width));
        if let Some(background) = self.theme.color(text) {
            decoration = decoration.bg(background);
        }
        if let Some(border) = self.theme.color(border) {
            decoration = decoration.border_1().border_dashed().border_color(border);
        }
        decoration
    }

    /// style.css `.gutter-insert`, `.gutter-delete`: the margin of a changed line.
    pub(super) fn margin_background(&self, side: ZoneSide, width: f32) -> Option<Div> {
        // style.css `var(--gutter, var(--line), var(--text))`: the fallback "line, text" is no color, so only the
        // gutter key paints.
        let key = match side {
            ZoneSide::Modified => "diffEditorGutter.insertedLineBackground",
            ZoneSide::Original => "diffEditorGutter.removedLineBackground",
        };
        let color = self.theme.color(key)?;
        Some(div().absolute().top_0().left_0().h_full().w(px(width)).bg(color))
    }

    /// `insert-sign codicon-diff-insert`, `delete-sign codicon-diff-remove`: 11px at 0.7 opacity (1 in high contrast).
    pub(super) fn diff_sign(&self, side: ZoneSide, metrics: EditorMetrics) -> Div {
        let name = if side == ZoneSide::Modified { "add" } else { "remove" };
        let opacity = if self.theme.kind.is_high_contrast() { 1. } else { 0.7 };
        self.icon(name, metrics.line_height, 11., self.color("editor.foreground")).opacity(opacity)
    }

    /// overviewRulerFeature.ts: the original's removed lines in the left lane, the modified's inserted ones in the
    /// right, over the modified editor's scroll height, and the viewport as a slider (`.diffViewport`).
    fn diff_overview(&self, input: &DiffInput, modified: &OpenEditor, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let viewport = self.editors.viewport;
        let scrollbar = modified.vertical_scrollbar(viewport);
        let scroll_height = modified.scroll_height(viewport).max(1.);
        let mut zones = Vec::new();
        let insert = self.theme.color("diffEditorOverview.insertedForeground").or_else(|| self.theme.color("diffEditor.insertedTextBackground").map(twice_as_opaque));
        let remove = self.theme.color("diffEditorOverview.removedForeground").or_else(|| self.theme.color("diffEditor.removedTextBackground").map(twice_as_opaque));
        let line_tops = self.aligned_line_tops(input, modified);
        for change in input.diff.as_ref().map_or(&[][..], |diff| &diff.diff.changes[..]) {
            if let (Some(color), false) = (remove, change.original.is_empty()) {
                zones.push((0., line_tops.original(change.original.start), line_tops.original(change.original.end), color));
            }
            if let (Some(color), false) = (insert, change.modified.is_empty()) {
                zones.push((ONE_OVERVIEW_WIDTH, line_tops.modified(modified, change.modified.start), line_tops.modified(modified, change.modified.end), color));
            }
        }
        // style.css: `.vs .diffOverview` 3% black, `.vs-dark` 1% white; high contrast none.
        let background = match self.theme.kind {
            crate::theme::ThemeKind::Light => Some(gpui::rgba(0x00000008)),
            crate::theme::ThemeKind::Dark => Some(gpui::rgba(0xffffff03)),
            _ => None,
        };
        let slider = self.theme.color("scrollbarSlider.background");
        div()
            .id("diff-overview")
            .relative()
            .when_some(background, |ruler, background| ruler.bg(background))
            .on_scroll_wheel(cx.listener(|workbench, event: &ScrollWheelEvent, window, cx| workbench.editor_wheel(event, window, cx)))
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        let height = f32::from(bounds.size.height);
                        for (left, top, bottom, color) in &zones {
                            let (top, bottom) = (top / scroll_height * height, (bottom / scroll_height * height).max(top / scroll_height * height + 2.));
                            window.paint_quad(fill(gpui::Bounds::new(bounds.origin + point(px(*left), px(top)), size(px(ONE_OVERVIEW_WIDTH), px(bottom - top))), *color));
                        }
                        if let Some(slider) = slider.filter(|_| scrollbar.needed) {
                            let slider_bounds = gpui::Bounds::new(bounds.origin + point(px(0.), px(scrollbar.slider_position)), size(bounds.size.width, px(scrollbar.slider_size)));
                            window.paint_quad(fill(slider_bounds, slider));
                        }
                    },
                )
                .absolute()
                .inset_0(),
            )
    }

    /// Where lines start in the aligned layout both editors share.
    fn aligned_line_tops(&self, input: &DiffInput, modified: &OpenEditor) -> LineTops {
        let line_height = self.editors.viewport.line_height;
        let original_zones = input.zones.iter().filter(|zone| zone.side == ZoneSide::Original).map(|zone| (zone.after_line, zone.lines as f32 * line_height)).collect();
        LineTops { line_height, original_zones, viewport: self.editors.viewport, modified_lines: modified.document().lines.len() }
    }
}

struct LineTops {
    line_height: f32,
    original_zones: Vec<(usize, f32)>,
    viewport: crate::editor::Viewport,
    modified_lines: usize,
}

impl LineTops {
    /// The top of 1-based original line `line` (one past the end for the end of the model).
    fn original(&self, line: usize) -> f32 {
        (line - 1) as f32 * self.line_height + self.original_zones.iter().filter(|zone| zone.0 < line).map(|zone| zone.1).sum::<f32>()
    }

    fn modified(&self, editor: &OpenEditor, line: usize) -> f32 {
        let view_line = editor.view_line((line - 1).min(self.modified_lines.saturating_sub(1)));
        editor.line_top(view_line, self.viewport) + if line - 1 >= self.modified_lines { self.line_height } else { 0. }
    }
}

/// `Color.transparent(2)`: twice the alpha.
fn twice_as_opaque(color: Rgba) -> Rgba {
    Rgba { a: (color.a * 2.).min(1.), ..color }
}

/// The character ranges of `change` on 1-based `line_number` of one side, as (left, width) in pixels; a range
/// reaching past the line's end fills to the line break (`shouldFillLineOnLineBreak`), an empty one is 3px wide
/// (`diff-range-empty`).
pub(super) fn char_ranges(change: &diff::LineRangeMapping, side: ZoneSide, line_number: usize, text: &str, tab_size: usize, char_width: f32) -> Vec<(f32, f32)> {
    let line_length = text.chars().count();
    let x = |column: usize| visible_column(text, column.min(line_length), tab_size) as f32 * char_width;
    let mut ranges = Vec::new();
    for inner in change.inner_changes.iter().flatten() {
        let range = if side == ZoneSide::Modified { inner.modified } else { inner.original };
        if range.start.line > line_number || range.end.line < line_number {
            continue;
        }
        let start = if range.start.line == line_number { range.start.column - 1 } else { 0 };
        let (end, fill_line_break) = if range.end.line == line_number { (range.end.column - 1, false) } else { (line_length, true) };
        let left = x(start);
        let width = x(end) - left + if fill_line_break { char_width } else { 0. };
        if range.is_empty() {
            ranges.push((left, 3.));
        } else if width > 0. {
            ranges.push((left, width));
        }
    }
    ranges
}

/// style.css `.diagonal-fill`: a -45deg stripe pattern in 8px tiles, 2px wide stripes every 4px.
pub(super) fn diagonal_fill(color: Rgba) -> gpui::Canvas<()> {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let (left, top) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
            let (width, height) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
            window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
                let mut x = -height - (left.rem_euclid(4.));
                while x < width {
                    let mut path = PathBuilder::fill();
                    path.move_to(point(px(left + x), px(top + height)));
                    path.line_to(point(px(left + x + 2.), px(top + height)));
                    path.line_to(point(px(left + x + 2. + height), px(top)));
                    path.line_to(point(px(left + x + height), px(top)));
                    path.close();
                    if let Ok(path) = path.build() {
                        window.paint_path(path, color);
                    }
                    x += 4.;
                }
            });
        },
    )
}

/// The 6px box-shadow `-6px 0 5px -5px` of the modified editor over the original's right edge.
fn side_shadow(color: Rgba) -> gpui::Canvas<()> {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            for step in 0..6 {
                let alpha = color.a * (1. - step as f32 / 6.) * 0.5;
                let x = bounds.origin.x + bounds.size.width - px(step as f32 + 1.);
                window.paint_quad(fill(gpui::Bounds::new(point(x, bounds.origin.y), size(px(1.), bounds.size.height)), Rgba { a: alpha, ..color }));
            }
        },
    )
}

#[cfg(test)]
mod tests;
