//! The quick diff peek (quickDiffWidget.ts): `QuickDiffEditorController` opens a `QuickDiffWidget`, a peek view
//! (peekView.ts) in a view zone (zoneWidget.ts) below a change, with the change drawn as an inline diff.

use gpui::{AnyElement, ClickEvent, Context, Div, PathBuilder, ScrollWheelEvent, StyledText, Window, canvas, div, point, prelude::*, px};
use serde_json::{Value, json};

use super::quick_diff::{ChangeKind, QuickDiffChange};
use super::{EditorMetrics, Workbench};
use crate::commands::Outcome;
use crate::diff_editor::ZoneSide;
use crate::editor::{OpenEditor, Position, Selection, VERTICAL_SCROLLBAR_WIDTH, VerticalReveal, expand_tabs, visible_column};
use crate::menus::{MenuAction, MenuId};
use crate::workbench::hover::HoverOptions;

/// zoneWidget.ts: the arrow is `round(lineHeight / 3)` high, the frame `frameWidth` 1.
const ARROW_HEIGHT: f32 = 6.;
const FRAME_WIDTH: f32 = 1.;
/// peekView.ts `_doLayout`: `ceil(lineHeight * 1.2)`.
const HEAD_HEIGHT: f32 = 22.;

/// The shown widget: its zone in the editor of `model_id`, the change it shows and the embedded diff's scroll.
pub(in crate::workbench) struct PeekWidget {
    model_id: u32,
    zone_id: u32,
    index: usize,
    /// `providerId`: the quick diff of the shown change, an index into the entry's quick diffs.
    provider: usize,
    /// The original lines (`QuickDiffModel.originalTextModels`) and the inline diff the embedded editor computes.
    diff: diff::LinesDiff,
    scroll_top: f32,
}

/// A line of the embedded inline diff editor: an original line in a deleted zone, or a modified line.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Row {
    Original { line: usize, change: usize },
    Modified { line: usize, change: Option<usize> },
}

/// The inline diff's lines top to bottom: a change's deleted lines sit in a zone above its first modified line.
fn inline_rows(diff: &diff::LinesDiff, modified_lines: usize) -> Vec<Row> {
    let mut rows = Vec::new();
    let deleted_before = |rows: &mut Vec<Row>, line: usize| {
        for (index, change) in diff.changes.iter().enumerate().filter(|(_, change)| change.modified.start == line) {
            rows.extend((change.original.start..change.original.end).map(|line| Row::Original { line, change: index }));
        }
    };
    for line in 1..=modified_lines {
        deleted_before(&mut rows, line);
        let change = diff.changes.iter().position(|change| change.modified.contains(line));
        rows.push(Row::Modified { line, change });
    }
    deleted_before(&mut rows, modified_lines + 1);
    rows
}

/// quickDiff.ts `getChangeHeight`.
fn change_height(change: &QuickDiffChange) -> usize {
    let modified = change.end as isize - change.start as isize + 1;
    let original = change.original_end as isize - change.original_start as isize + 1;
    let height = match (change.original_end, change.end) {
        (0, _) => modified,
        (_, 0) => original,
        _ => modified + original,
    };
    height.max(0) as usize
}

/// quickDiff.ts `getModifiedEndLineNumber`.
fn modified_end_line(change: &QuickDiffChange) -> usize {
    match change.end {
        0 => change.start.max(1),
        end => end,
    }
}

/// quickDiff.ts `lineIntersectsChange`.
fn line_intersects(line: usize, change: &QuickDiffChange) -> bool {
    if line == 1 && change.start == 0 && change.end == 0 {
        return true;
    }
    line >= change.start && line <= if change.end == 0 { change.start } else { change.end }
}

/// quickDiffModel.ts `findNextClosestChange` over the changes `eligible` keeps (of the provider, of visible quick
/// diffs).
fn next_closest(changes: &[QuickDiffChange], line: usize, inclusive: bool, eligible: impl Fn(&QuickDiffChange) -> bool) -> usize {
    let found = changes.iter().position(|change| eligible(change) && if inclusive { modified_end_line(change) >= line } else { change.start > line });
    found.unwrap_or(0)
}

/// quickDiffModel.ts `findPreviousClosestChange`.
fn previous_closest(changes: &[QuickDiffChange], line: usize, inclusive: bool, eligible: impl Fn(&QuickDiffChange) -> bool) -> usize {
    let found = changes.iter().rposition(|change| eligible(change) && if inclusive { change.start <= line } else { modified_end_line(change) < line });
    found.unwrap_or(changes.len() - 1)
}

/// `rot` within `quickDiffChanges.get(providerId)`: the provider's change after or before `index`.
fn step_in_provider(changes: &[QuickDiffChange], index: usize, provider: usize, forward: bool) -> usize {
    let provider_changes: Vec<usize> = (0..changes.len()).filter(|&at| changes[at].provider == provider).collect();
    let position = provider_changes.iter().position(|&at| at == index).map_or(-1, |position| position as isize);
    let next = (position + if forward { 1 } else { -1 }).rem_euclid(provider_changes.len() as isize);
    provider_changes[next as usize]
}

/// The `IChange` the git extension's change commands take.
fn change_json(change: &QuickDiffChange) -> Value {
    json!({
        "originalStartLineNumber": change.original_start,
        "originalEndLineNumber": change.original_end,
        "modifiedStartLineNumber": change.start,
        "modifiedEndLineNumber": change.end,
    })
}

impl Workbench {
    pub(in crate::workbench) fn register_quick_diff_commands(&mut self) {
        self.register_quick_diff_visibility_commands();
        for (id, forward) in [("editor.action.dirtydiff.next", true), ("editor.action.dirtydiff.previous", false)] {
            self.commands.register(id, move |workbench, _, window, cx| {
                if workbench.quick_diff_can_navigate() {
                    workbench.navigate_quick_diff(forward, None, window, cx);
                }
                Outcome::done()
            });
        }
        for (id, forward) in [("workbench.action.editor.nextChange", true), ("workbench.action.editor.previousChange", false)] {
            self.commands.register(id, move |workbench, _, window, cx| {
                workbench.go_to_change(forward, window, cx);
                Outcome::done()
            });
        }
        self.commands.register("closeQuickDiff", |workbench, _, window, cx| {
            workbench.close_quick_diff(cx);
            window.focus(&workbench.editor_focus, cx);
            Outcome::done()
        });
        // shortcut: the peek draws its diff read-only, so `toggleQuickDiffWidgetFocus` has no embedded editor to focus;
        // register it once the peek hosts an editor.
    }

    /// `dirtyDiffVisible`.
    pub(in crate::workbench) fn quick_diff_visible(&self) -> bool {
        self.quick_diffs.widget.is_some()
    }

    /// `quickDiffDecorationCount` of the active editor: the changes of visible quick diffs.
    pub(in crate::workbench) fn quick_diff_decoration_count(&self) -> usize {
        let Some(entry) = self.editors.active().and_then(|editor| self.quick_diff_entry(editor)) else { return 0 };
        entry.changes.iter().filter(|change| self.quick_diff_provider_visible(&entry.quick_diffs[change.provider].id)).count()
    }

    /// Whether a change's quick diff is visible.
    fn quick_diff_change_visible(&self, editor: &OpenEditor, change: &QuickDiffChange) -> bool {
        self.quick_diff_entry(editor).is_some_and(|entry| self.quick_diff_provider_visible(&entry.quick_diffs[change.provider].id))
    }

    /// `QuickDiffEditorController.canNavigate`.
    fn quick_diff_can_navigate(&self) -> bool {
        self.quick_diffs.widget.is_none() || self.editors.active().is_some_and(|editor| self.quick_diff_changes(editor).len() > 1)
    }

    /// `QuickDiffEditorController.next` and `previous`: from a line (a gutter click) or a new widget, the closest
    /// change; else the one after or before the shown one.
    fn navigate_quick_diff(&mut self, forward: bool, line: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        let changes = self.quick_diff_changes(editor).to_vec();
        if changes.is_empty() {
            return self.close_quick_diff(cx);
        }
        let shown = self.quick_diffs.widget.as_ref().filter(|widget| widget.model_id == editor.model_id).map(|widget| (widget.index, widget.provider));
        let index = match (shown, line) {
            (Some((index, provider)), None) => step_in_provider(&changes, index, provider, forward),
            (shown, line) => {
                let line = line.unwrap_or(editor.selection().head.line + 1);
                let provider = shown.map(|shown| shown.1);
                let eligible = |change: &QuickDiffChange| provider.is_none_or(|provider| change.provider == provider) && self.quick_diff_change_visible(editor, change);
                if forward { next_closest(&changes, line, true, eligible) } else { previous_closest(&changes, line, true, eligible) }
            }
        };
        self.show_quick_diff_change(index, true, cx);
        window.focus(&self.editor_focus, cx);
    }

    /// `GotoNextChangeAction`, `GotoPreviousChangeAction`: the cursor to the change's first line, centered.
    fn go_to_change(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let viewport = self.editors.viewport;
        let Some(editor) = self.editors.active() else { return };
        let changes = self.quick_diff_changes(editor).to_vec();
        if changes.is_empty() {
            return;
        }
        let line = editor.selection().head.line + 1;
        let eligible = |change: &QuickDiffChange| self.quick_diff_change_visible(editor, change);
        let index = if forward { next_closest(&changes, line, false, eligible) } else { previous_closest(&changes, line, false, eligible) };
        let editor = self.editors.active_mut().expect("checked above");
        let position = Position::new(changes[index].start.max(1) - 1, 0);
        editor.set_selections(vec![Selection::caret(position)]);
        editor.reveal_range(position, position, VerticalReveal::Center, viewport);
        self.host.send_selection(editor, "api");
        self.editor_scrolled(cx);
        window.focus(&self.editor_focus, cx);
        cx.notify();
    }

    /// `QuickDiffWidget.showChange`: the zone below the change's last line, `min(change + 8, a third of the editor)`
    /// lines high, the embedded diff centered on the change; with `use_position` the cursor moves there.
    fn show_quick_diff_change(&mut self, index: usize, use_position: bool, cx: &mut Context<Self>) {
        let viewport = self.editors.viewport;
        let Some(editor) = self.editors.active() else { return };
        let key = (editor.document.scheme.clone(), editor.document.path.clone());
        let Some(entry) = self.quick_diffs.entries.get(&key) else { return };
        let Some(change) = entry.changes.get(index).copied() else { return };
        let original = &entry.quick_diffs[change.provider].original;
        let Some(original) = self.editors.model(&original.scheme, &original.path) else { return };
        // The embedded diff editor's options: `ignoreTrimWhitespace` false, the advanced algorithm.
        let options = diff::DiffOptions { ignore_trim_whitespace: false, max_computation_time_ms: 5000, compute_moves: false, extend_to_subwords: false };
        let diff = diff::compute_diff(&original.document.lines, &editor.document.lines, options);
        let model_id = editor.model_id;
        let editor_lines = (viewport.height / viewport.line_height).floor() as usize;
        let height_in_lines = (change_height(&change) + 2 + 6).min(editor_lines / 3);
        let height = height_in_lines as f32 * viewport.line_height + 1.;
        let position = Position::new(modified_end_line(&change) - 1, 0);
        let old = self.quick_diffs.widget.take().filter(|widget| widget.model_id == model_id);
        let editor = self.editors.active_mut().expect("checked above");
        let zone_id = match old.as_ref().filter(|_| !use_position) {
            Some(widget) => widget.zone_id,
            None => {
                if let Some(widget) = &old {
                    editor.remove_view_zone(widget.zone_id);
                }
                editor.add_view_zone(Some(position), height)
            }
        };
        let body_height = height - 2. * ARROW_HEIGHT - 2. * FRAME_WIDTH - HEAD_HEIGHT;
        let rows = inline_rows(&diff, editor.document.lines.len());
        let scroll_top = reveal_change_scroll(&rows, &change, body_height, viewport.line_height);
        // The picker's items: one command per quick diff of the model (`QuickDiffPickerBaseAction`).
        let quick_diff_count = self.quick_diffs.entries.get(&key).map_or(0, |entry| entry.quick_diffs.len());
        for provider in 0..quick_diff_count {
            self.commands.register(&format!("_quickDiff.switchProvider.{provider}"), move |workbench, _, _, cx| {
                workbench.switch_quick_diff(provider, cx);
                Outcome::done()
            });
        }
        self.quick_diffs.widget = Some(PeekWidget { model_id, zone_id, index, provider: change.provider, diff, scroll_top });
        if use_position {
            let editor = self.editors.active_mut().expect("checked above");
            editor.set_selections(vec![Selection::caret(position)]);
            let below = Position::new((position.line + 1).min(editor.document.lines.len() - 1), 0);
            editor.reveal_range(below, below, VerticalReveal::CenterIfOutsideViewport, viewport);
            self.host.send_selection(editor, "api");
            self.editor_scrolled(cx);
        }
        cx.notify();
    }

    /// `QuickDiffEditorController.close`.
    pub(in crate::workbench) fn close_quick_diff(&mut self, cx: &mut Context<Self>) {
        let Some(widget) = self.quick_diffs.widget.take() else { return };
        if let Some(editor) = self.editors.editors.iter_mut().find(|editor| editor.model_id == widget.model_id) {
            editor.remove_view_zone(widget.zone_id);
        }
        cx.notify();
    }

    /// The controller's listeners, run when the quick diff updates: another model closes the widget
    /// (`onDidChangeModel`); new changes move it on when they start at or before the shown one, else redraw it
    /// (`onDidModelChange`).
    pub(in crate::workbench) fn follow_quick_diff_widget(&mut self, old_changes: Option<Vec<QuickDiffChange>>, cx: &mut Context<Self>) {
        let Some(widget) = &self.quick_diffs.widget else { return };
        let Some(editor) = self.editors.active().filter(|editor| editor.model_id == widget.model_id) else { return self.close_quick_diff(cx) };
        let Some(old_changes) = old_changes else { return };
        let changes = self.quick_diff_changes(editor).to_vec();
        if changes.is_empty() {
            return self.close_quick_diff(cx);
        }
        let (index, provider) = (widget.index, widget.provider);
        let first_difference = (0..old_changes.len().max(changes.len())).find(|&at| old_changes.get(at) != changes.get(at));
        match first_difference {
            None => {}
            Some(start) if start <= index => {
                let provider = if changes.iter().any(|change| change.provider == provider) { provider } else { changes[0].provider };
                self.show_quick_diff_change(step_in_provider(&changes, index.min(changes.len() - 1), provider, true), true, cx)
            }
            Some(_) => self.show_quick_diff_change(index, false, cx),
        }
    }

    /// `QuickDiffWidget.switchQuickDiff`: the other provider's change closest to the shown one, by last modified line.
    fn switch_quick_diff(&mut self, provider: usize, cx: &mut Context<Self>) {
        let Some(widget) = &self.quick_diffs.widget else { return };
        let Some(editor) = self.editors.active().filter(|editor| editor.model_id == widget.model_id) else { return };
        let changes = self.quick_diff_changes(editor);
        let index = widget.index;
        if provider == changes[index].provider {
            return;
        }
        let count = changes.len();
        let mut greater = if index < count - 1 { index + 1 } else { 0 };
        let mut at = greater;
        while at != index {
            if changes[at].provider == provider {
                greater = at;
                break;
            }
            at = if at < count - 1 { at + 1 } else { 0 };
        }
        let mut lesser = if index > 0 { index - 1 } else { count - 1 };
        let mut at = lesser;
        while at != index {
            if changes[at].provider == provider {
                lesser = at;
                break;
            }
            at = if at > 0 { at - 1 } else { count - 1 };
        }
        let distance = |at: usize| changes[at].end.abs_diff(changes[index].end);
        let closest = if distance(greater) < distance(lesser) { greater } else { lesser };
        self.show_quick_diff_change(closest, false, cx);
    }

    /// `onEditorMouseUp` on a change's glyph with `scm.diffDecorationsGutterAction` `diff`: the shown change closes
    /// the widget, another one shows.
    pub(super) fn quick_diff_glyph_clicked(&mut self, line: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        let Some(index) = self.quick_diff_changes(editor).iter().position(|change| line_intersects(line, change)) else { return };
        let shown = self.quick_diffs.widget.as_ref().filter(|widget| widget.model_id == editor.model_id).map(|widget| widget.index);
        match shown == Some(index) {
            true => self.close_quick_diff(cx),
            false => self.navigate_quick_diff(true, Some(line), window, cx),
        }
    }

    /// The peek over its zone's whitespace, as wide as the editor without the vertical scrollbar, framed in the
    /// change's gutter color with the arrow at column 2 (zoneWidget.ts moves column 1 there).
    pub(super) fn quick_diff_widget(&self, editor: &OpenEditor, metrics: EditorMetrics, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let widget = self.quick_diffs.widget.as_ref().filter(|widget| widget.model_id == editor.model_id)?;
        let viewport = self.editors.viewport;
        let zone = editor.view_zone(widget.zone_id)?;
        let top = editor.view_zone_top(widget.zone_id, viewport)? - editor.scroll.top;
        let changes = self.quick_diff_changes(editor);
        let change = changes.get(widget.index)?;
        let key = (editor.document.scheme.clone(), editor.document.path.clone());
        let entry = self.quick_diffs.entries.get(&key)?;
        let frame = self.color(match change.kind {
            ChangeKind::Add => "editorGutter.addedBackground",
            ChangeKind::Delete => "editorGutter.deletedBackground",
            ChangeKind::Modify => "editorGutter.modifiedBackground",
        });
        let anchor = zone.anchor?;
        let line = &editor.document.lines[anchor.line];
        let arrow_column = anchor.column.max(1).min(line.chars().count().max(1));
        let arrow_x = metrics.content_left() + visible_column(line, arrow_column, editor.document.indentation.tab_size) as f32 * metrics.char_width - editor.scroll.left;
        let arrow = canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let (x, y) = (bounds.origin.x, bounds.origin.y);
                let mut path = PathBuilder::fill();
                path.move_to(point(x, y + px(ARROW_HEIGHT)));
                path.line_to(point(x + px(2. * ARROW_HEIGHT), y + px(ARROW_HEIGHT)));
                path.line_to(point(x + px(ARROW_HEIGHT), y));
                path.close();
                if let Ok(path) = path.build() {
                    window.paint_path(path, frame);
                }
            },
        )
        .absolute()
        .top_0()
        .left(px(arrow_x - ARROW_HEIGHT))
        .w(px(2. * ARROW_HEIGHT))
        .h(px(ARROW_HEIGHT));
        let container_height = zone.height - 2. * ARROW_HEIGHT - 2. * FRAME_WIDTH;
        // `renderTitle`: the position among the provider's changes; the label when no picker shows it.
        let provider_changes: Vec<usize> = (0..changes.len()).filter(|&at| changes[at].provider == widget.provider).collect();
        let provider_index = provider_changes.iter().position(|&at| at == widget.index).unwrap_or(0) + 1;
        let noun = if changes.len() > 1 { "changes" } else { "change" };
        let pickable = self.quick_diffs_containing(editor, change);
        let detail = match pickable.len() > 1 {
            false => format!("{} - {provider_index} of {} {noun}", entry.quick_diffs[widget.provider].label, provider_changes.len()),
            true => format!("{provider_index} of {} {noun}", provider_changes.len()),
        };
        let picker = (pickable.len() > 1).then(|| self.quick_diff_picker(&entry.quick_diffs[widget.provider].label, pickable, widget.provider, cx));
        let title = div()
            .ml(px(20.))
            .min_w_0()
            .overflow_hidden()
            .flex()
            .items_baseline()
            .text_size(px(13.))
            .children(picker)
            .child(div().whitespace_nowrap().text_color(self.color("peekViewTitleLabel.foreground")).child(editor.document.name()))
            .child(div().whitespace_nowrap().ml(px(0.5 * 0.9 * 13.)).text_size(px(0.9 * 13.)).text_color(self.color("peekViewTitleDescription.foreground")).child(detail));
        let head = div()
            .h(px(HEAD_HEIGHT))
            .line_height(px(HEAD_HEIGHT))
            .flex()
            .justify_between()
            .flex_nowrap()
            .font_family(".SystemUIFont")
            .when_some(self.theme.color("peekViewTitle.background"), |head, color| head.bg(color))
            .child(title)
            .child(self.quick_diff_actions(editor, entry, widget, window, cx));
        let body = self.quick_diff_body(editor, widget, container_height - HEAD_HEIGHT, metrics, cx);
        let container = div()
            .absolute()
            .top(px(ARROW_HEIGHT))
            .left_0()
            .right_0()
            .h(px(container_height + 2. * FRAME_WIDTH))
            .overflow_hidden()
            .border_t(px(FRAME_WIDTH))
            .border_b(px(FRAME_WIDTH))
            .border_color(frame)
            .child(head)
            .child(body);
        let widget = div()
            .id("quick-diff-widget")
            .absolute()
            .left_0()
            .top(px(top))
            .w(px(metrics.content_left() + viewport.width - VERTICAL_SCROLLBAR_WIDTH))
            .h(px(zone.height))
            .cursor_default()
            .occlude()
            .child(container)
            .child(arrow);
        Some(widget.into_any_element())
    }

    /// `getQuickDiffsContainingChange`: the visible quick diffs with a change touching the shown one's lines, in the
    /// model's order, as indices.
    fn quick_diffs_containing(&self, editor: &OpenEditor, change: &QuickDiffChange) -> Vec<usize> {
        let Some(entry) = self.quick_diff_entry(editor) else { return Vec::new() };
        let touching: Vec<usize> = entry.changes.iter().filter(|other| change.modified.start <= other.modified.end && other.modified.start <= change.modified.end).map(|other| other.provider).collect();
        (0..entry.quick_diffs.len()).filter(|index| touching.contains(index) && self.quick_diff_provider_visible(&entry.quick_diffs[*index].id)).collect()
    }

    /// `QuickDiffPickerViewItem`: a select box before the title naming the shown change's quick diff; picking another
    /// shows that one's closest change.
    fn quick_diff_picker(&self, label: &str, providers: Vec<usize>, shown: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let bounds = std::rc::Rc::new(std::cell::Cell::new(gpui::Bounds::default()));
        let foreground = self.foreground_or("select.foreground");
        let key = self.editors.active().map(|editor| (editor.document.scheme.clone(), editor.document.path.clone()));
        let tracked = bounds.clone();
        div()
            .id("quick-diff-picker")
            .relative()
            .self_center()
            .mr(px(8.))
            .h(px(20.))
            .flex()
            .items_center()
            .pl(px(6.))
            .pr(px(22.))
            .border_1()
            .rounded(px(4.))
            .cursor_pointer()
            .text_size(px(11.))
            .whitespace_nowrap()
            .bg(self.color("select.background"))
            .border_color(self.color("select.border"))
            .text_color(foreground)
            .child(label.to_string())
            .child(self.icon("chevron-down", 16., 16., foreground).absolute().right(px(3.)))
            .child(canvas(move |bounds, _, _| tracked.set(bounds), |_, _, _, _| ()).absolute().inset_0())
            .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |workbench, _: &gpui::MouseDownEvent, window, cx| {
                cx.stop_propagation();
                let Some(entry) = key.as_ref().and_then(|key| workbench.quick_diffs.entries.get(key)) else { return };
                let actions = providers.iter().map(|&provider| MenuAction::Command { id: format!("_quickDiff.switchProvider.{provider}"), title: entry.quick_diffs[provider].label.clone(), enabled: true, checked: provider == shown, alt: None }).collect();
                let context = workbench.key_context(window);
                workbench.show_context_menu_actions(actions, &context, bounds.get(), Vec::new(), window, cx);
            }))
    }

    /// `updateActions`: the `scm/change/title` menu reversed, then next, previous and close. A menu action runs with
    /// `[uri, changes, index]` and closes the widget (`QuickDiffWidgetActionRunner`).
    fn quick_diff_actions(&self, editor: &OpenEditor, entry: &super::quick_diff::Entry, widget: &PeekWidget, window: &Window, cx: &mut Context<Self>) -> Div {
        let foreground = self.foreground_or("icon.foreground");
        let hover = self.color("toolbar.hoverBackground");
        let mut context = self.key_context(window);
        let original = &entry.quick_diffs[widget.provider].original;
        context.insert("originalResource".into(), json!(crate::editor::uri_string(&original.scheme, &original.path)));
        context.insert("originalResourceScheme".into(), json!(original.scheme));
        // The context is the provider's changes and the shown one's index among them.
        let provider_changes: Vec<&QuickDiffChange> = entry.changes.iter().filter(|change| change.provider == widget.provider).collect();
        let context_index = entry.changes[..widget.index].iter().filter(|change| change.provider == widget.provider).count();
        let args = vec![editor.document.uri(), Value::Array(provider_changes.into_iter().map(change_json).collect()), json!(context_index)];
        let mut bar = div().flex_1().flex().justify_end().items_center().pr(px(2.));
        let mut commands: Vec<(String, String)> = Vec::new();
        for action in crate::menus::menu_actions(&self.menus, MenuId::ScmChangeContext, &context) {
            if let MenuAction::Command { id, title, enabled: true, .. } = action {
                commands.push((id, title));
            }
        }
        for (id, title) in commands.into_iter().rev() {
            let icon = match self.command_icons.get(&id) {
                Some(crate::menus::CommandIcon::Codicon(name)) => self.icon(name, 22., 16., foreground),
                _ => div().px(px(4.)).child(title.clone()),
            };
            let args = args.clone();
            let command = id.clone();
            bar = bar.child(
                icon.id(gpui::SharedString::from(format!("quick-diff-{id}")))
                    .rounded(px(5.))
                    .cursor_pointer()
                    .hover(move |action| action.bg(hover))
                    .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                        cx.stop_propagation();
                        drop(workbench.execute_command(&command, args.clone(), window, cx));
                        workbench.close_quick_diff(cx);
                    }))
                    .child(self.hover_target(gpui::SharedString::from(format!("quick-diff-hover-{id}")), super::super::hover::HoverContent::Text(title), HoverOptions::mouse())),
            );
        }
        bar.child(self.peek_action("arrow-down", "Show Next Change", "editor.action.dirtydiff.next", cx))
            .child(self.peek_action("arrow-up", "Show Previous Change", "editor.action.dirtydiff.previous", cx))
            .child(self.peek_action("close", "Close", "closeQuickDiff", cx))
    }

    /// `_fillBody`: the embedded inline diff editor (no indicators, no overview ruler, no minimap), drawn read-only
    /// and scrolled by the wheel without scrolling beyond the last line.
    // shortcut: the port has one code editor per group, so the embedded diff editor is drawn, not editable; make it an
    // editor when editors can be embedded.
    fn quick_diff_body(&self, editor: &OpenEditor, widget: &PeekWidget, height: f32, metrics: EditorMetrics, cx: &mut Context<Self>) -> impl IntoElement {
        let key = (editor.document.scheme.clone(), editor.document.path.clone());
        let original = self.quick_diffs.entries.get(&key).and_then(|entry| entry.quick_diffs.get(widget.provider)).and_then(|quick_diff| self.editors.model(&quick_diff.original.scheme, &quick_diff.original.path));
        let line_height = metrics.line_height;
        let rows = inline_rows(&widget.diff, editor.document.lines.len());
        let first = (widget.scroll_top / line_height).floor() as usize;
        let visible = (height / line_height).ceil() as usize + 1;
        let content_left = metrics.content_left();
        let mut lines = div().absolute().left_0().right_0().top(px(first as f32 * line_height - widget.scroll_top));
        for row in rows.iter().skip(first).take(visible) {
            let line = match *row {
                Row::Modified { line, change } => self.peek_line(editor, line, change.map(|change| (ZoneSide::Modified, change)), widget, metrics),
                Row::Original { line, change } => match original {
                    Some(original) => self.peek_line(original, line, Some((ZoneSide::Original, change)), widget, metrics),
                    None => div().h(px(line_height)),
                },
            };
            lines = lines.child(line);
        }
        let max_scroll = (rows.len() as f32 * line_height - height).max(0.);
        div()
            .id("quick-diff-body")
            .relative()
            .h(px(height))
            .overflow_hidden()
            .border_t(px(1.))
            .border_color(self.color("peekView.border"))
            .bg(self.color("editor.background"))
            .text_color(self.color("editor.foreground"))
            .font_family(super::EDITOR_FONT)
            .text_size(px(metrics.font_size))
            .line_height(px(line_height))
            .child(div().absolute().top_0().bottom_0().left(px(content_left)).right_0().cursor_text())
            .child(lines)
            .on_scroll_wheel(cx.listener(move |workbench, event: &ScrollWheelEvent, _, cx| {
                cx.stop_propagation();
                let Some(widget) = workbench.quick_diffs.widget.as_mut() else { return };
                let delta = f32::from(event.delta.pixel_delta(px(line_height)).y);
                widget.scroll_top = (widget.scroll_top - delta).clamp(0., max_scroll);
                cx.notify();
            }))
    }

    /// One line of the embedded diff: the modified line number, the change's line and character backgrounds, the
    /// highlighted text. Deleted lines have no number in the inline view.
    fn peek_line(&self, model: &OpenEditor, line: usize, change: Option<(ZoneSide, usize)>, widget: &PeekWidget, metrics: EditorMetrics) -> Div {
        let text = &model.document.lines[line - 1];
        let tab_size = model.document.indentation.tab_size;
        let content_left = metrics.content_left();
        let mut content = div().absolute().top_0().bottom_0().right_0().left(px(content_left));
        let mut row = div().h(px(metrics.line_height)).relative().flex();
        if let Some((side, index)) = change {
            let mapping = &widget.diff.changes[index];
            content = content.children(self.line_background(side));
            let other_empty = if side == ZoneSide::Modified { mapping.original.is_empty() } else { mapping.modified.is_empty() };
            if !other_empty {
                let ranges = super::diff_editor_view::char_ranges(mapping, side, line, text, tab_size, metrics.char_width);
                content = content.children(ranges.into_iter().map(|(left, width)| self.char_background(side, left, width)));
            }
            row = row.children(self.margin_background(side, content_left));
        }
        let highlights = model.document.tokens.highlights(&self.theme, &self.languages, &model.document.lines, line - 1, tab_size);
        content = content.child(div().absolute().top_0().left_0().whitespace_nowrap().child(StyledText::new(expand_tabs(text, tab_size)).with_highlights(highlights)));
        let number = match change {
            Some((ZoneSide::Original, _)) => String::new(),
            _ => line.to_string(),
        };
        row.child(div().w(px(metrics.glyph_margin_width())).flex_none())
            .child(div().w(px(metrics.line_numbers_width)).flex_none().text_right().text_color(self.color("editorLineNumber.foreground")).child(number))
            .child(content)
    }
}

/// `revealChange`: `revealLinesInCenter` of the lines around the change in the embedded diff, as a scroll offset that
/// stays inside the content (`scrollBeyondLastLine` false).
fn reveal_change_scroll(rows: &[Row], change: &QuickDiffChange, height: f32, line_height: f32) -> f32 {
    let (start, end) = match (change.end, change.original_end) {
        (0, _) => (change.start, change.start + 1),
        (_, 0) => (change.start, change.end),
        _ => (change.start.saturating_sub(1), change.end + 1),
    };
    let row_of = |line: usize| rows.iter().position(|row| matches!(row, Row::Modified { line: at, .. } if *at == line));
    let last = rows.iter().filter(|row| matches!(row, Row::Modified { .. })).count().max(1);
    let top = row_of(start.clamp(1, last)).unwrap_or(0) as f32 * line_height;
    let bottom = (row_of(end.clamp(1, last)).unwrap_or(0) + 1) as f32 * line_height;
    let max_scroll = (rows.len() as f32 * line_height - height).max(0.);
    ((top + bottom) / 2. - height / 2.).clamp(0., max_scroll)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.split('\n').map(str::to_string).collect()
    }

    fn diff_of(original: &str, modified: &str) -> diff::LinesDiff {
        let options = diff::DiffOptions { ignore_trim_whitespace: false, max_computation_time_ms: 0, compute_moves: false, extend_to_subwords: false };
        diff::compute_diff(&lines(original), &lines(modified), options)
    }

    #[test]
    fn quick_diff_peek_rows_put_deleted_lines_above_their_change() {
        let diff = diff_of("a\nb\nc\nd", "a\nB\nc");
        let rows = inline_rows(&diff, 3);
        assert_eq!(
            rows,
            vec![
                Row::Modified { line: 1, change: None },
                Row::Original { line: 2, change: 0 },
                Row::Modified { line: 2, change: Some(0) },
                Row::Modified { line: 3, change: None },
                Row::Original { line: 4, change: 1 },
            ]
        );
    }

    #[test]
    fn quick_diff_peek_finds_closest_changes_as_quick_diff_model() {
        let change = |start, end, provider| QuickDiffChange { kind: ChangeKind::Modify, start, end, original_start: start, original_end: end, provider, modified: diff::LineRange { start, end: end + 1 } };
        let changes = [change(3, 4, 0), change(6, 6, 1), change(10, 10, 0)];
        let any = |_: &QuickDiffChange| true;
        let first_provider = |change: &QuickDiffChange| change.provider == 0;
        assert_eq!(next_closest(&changes, 4, true, any), 0);
        assert_eq!(next_closest(&changes, 4, false, any), 1);
        assert_eq!(next_closest(&changes, 4, false, first_provider), 2);
        assert_eq!(next_closest(&changes, 11, true, any), 0);
        assert_eq!(previous_closest(&changes, 3, true, any), 0);
        assert_eq!(previous_closest(&changes, 3, false, any), 2);
        assert_eq!(step_in_provider(&changes, 2, 0, true), 0);
        assert_eq!(step_in_provider(&changes, 0, 0, false), 2);
        assert!(line_intersects(4, &changes[0]));
        assert!(!line_intersects(5, &changes[0]));
        let deletion = QuickDiffChange { kind: ChangeKind::Delete, start: 0, end: 0, original_start: 1, original_end: 2, provider: 0, modified: diff::LineRange { start: 1, end: 1 } };
        assert!(line_intersects(1, &deletion));
        assert_eq!((change_height(&deletion), modified_end_line(&deletion)), (2, 1));
        assert_eq!(change_height(&changes[0]), 4);
    }

    #[gpui::test]
    fn quick_diff_peek_shows_next_changes_and_closes(cx: &mut gpui::TestAppContext) {
        use super::super::quick_diff::{QuickDiff, QuickDiffKind};
        use std::path::Path;
        let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
        let themes = exthost::scan::scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let theme = crate::theme::load_theme(&[themes], "Dark Modern").unwrap();
        let dir = std::env::temp_dir().join(format!("code-gpui-quick-diff-peek-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.canonicalize().unwrap().join("a.txt");
        std::fs::write(&file, "one\nTWO\nthree\nfour\nfive\nsix\nseven\nEIGHT").unwrap();
        let (sender, _receiver) = std::sync::mpsc::channel();
        let settings = std::sync::Arc::new(std::sync::Mutex::new(config::service::Settings::load(config::registry::Registry::with_extensions(&[]), dir.join("User"), None)));
        let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), None, crate::languages::Languages::new(&[]), settings, crate::host_sync::HostSync::new(sender), cx));
        workbench.update_in(cx, |workbench, window, cx| {
            workbench.open_file(file.clone(), true, window, cx);
            // The index (primary) has the staged `TWO`; HEAD (secondary) still has `two`. Both have `eight`.
            let mut quick_diffs = Vec::new();
            for (reference, text, kind, id) in [("", "one\nTWO\nthree\nfour\nfive\nsix\nseven\neight", QuickDiffKind::Primary, "git.quickDiffProvider"), ("HEAD", "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight", QuickDiffKind::Secondary, "git.secondaryQuickDiffProvider")] {
                let original = json!({ "scheme": "git", "path": file.to_string_lossy(), "query": format!("{{\"ref\":\"{reference}\"}}") });
                workbench.try_open_document(&original, &Value::Null, &json!(text), false, cx).unwrap();
                let (scheme, path) = crate::workbench::host_requests::uri_parts(&original).map(|(scheme, path)| (scheme.to_string(), path)).unwrap();
                quick_diffs.push(QuickDiff { id: id.into(), label: "Git".into(), kind, original: crate::diff_editor::Side { scheme, path } });
            }
            let generation = Some(workbench.quick_diffs.generation);
            let entry = super::super::quick_diff::Entry { quick_diffs, generation, ..Default::default() };
            workbench.quick_diffs.entries.insert(("file".into(), file.clone()), entry);
            workbench.update_quick_diff(cx);
        });
        let run = |command: &'static str, cx: &mut gpui::VisualTestContext| {
            workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command(command, Vec::new(), window, cx)));
            workbench.read_with(cx, |workbench, _| {
                let widget = workbench.quick_diffs.widget.as_ref()?;
                let editor = workbench.editors.active().unwrap();
                Some((widget.index, editor.view_zone(widget.zone_id).unwrap().anchor.unwrap().line, editor.selection().head.line))
            })
        };
        // HEAD's line 8 change repeats the index's, so the changes are HEAD's line 2 and the index's line 8.
        let changes = workbench.read_with(cx, |workbench, _| workbench.quick_diff_changes(workbench.editors.active().unwrap()).iter().map(|change| (change.start, change.provider)).collect::<Vec<_>>());
        assert_eq!(changes, [(2, 1), (8, 0)]);
        assert_eq!(workbench.read_with(cx, |workbench, _| workbench.quick_diff_decoration_count()), 2);
        // From the cursor on line 1 the closest change is line 2; the zone sits below it and the cursor moves there.
        assert_eq!(run("editor.action.dirtydiff.next", cx), Some((0, 1, 1)));
        // Next stays among the shown quick diff's changes; the picker switches to the other one.
        assert_eq!(run("editor.action.dirtydiff.next", cx), Some((0, 1, 1)));
        assert_eq!(run("_quickDiff.switchProvider.0", cx), Some((1, 1, 1)));
        assert_eq!(run("closeQuickDiff", cx), None);
        assert_eq!(run("editor.action.dirtydiff.previous", cx), Some((0, 1, 1)));
        assert_eq!(run("closeQuickDiff", cx), None);
        workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("_quickDiff.toggleProviderVisibility", vec![json!("git.secondaryQuickDiffProvider")], window, cx)));
        assert_eq!(workbench.read_with(cx, |workbench, _| workbench.quick_diff_decoration_count()), 1);
        assert_eq!(run("editor.action.dirtydiff.next", cx), Some((1, 7, 7)));
        assert_eq!(run("closeQuickDiff", cx), None);
        assert!(workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().view_zones.is_empty()));
        // From line 8 the only visible earlier change is hidden, so it falls back to the last change, as upstream.
        assert_eq!(run("workbench.action.editor.previousChange", cx), None);
        assert_eq!(workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().selection().head.line), 7);
    }
}
