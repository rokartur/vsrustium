//! The diff editor's gutter (gutterFeature.ts): a 35px column with a toolbar per change, the built-in Revert Block
//! and the `diffEditor/gutter/hunk` actions of extensions (git's Stage Block), shown for the change at the cursor
//! and for every change while the pointer is over the column.

use gpui::{Bounds, Context, Div, MouseButton, MouseDownEvent, Pixels, div, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use crate::diff_editor::DiffInput;
use crate::editor::{EditReason, OpenEditor, Position};
use crate::menus::{MenuAction, MenuId, menu_actions};
use crate::workbench::hover::{HoverContent, HoverOptions};

/// gutterFeature.ts `width`.
pub(super) const GUTTER_WIDTH: f32 = 35.;
/// actionbar.css: a 16px codicon with `padding: 1px 2px`.
const BUTTON_HEIGHT: f32 = 18.;
const REVERT: &str = "diffEditor.revert";

/// One toolbar action: the command, its icon, its title.
#[derive(Clone)]
struct HunkAction {
    command: String,
    icon: String,
    title: String,
}

impl Workbench {
    pub(in crate::workbench) fn register_diff_gutter_commands(&mut self) {
        self.commands.register(REVERT, |workbench, args, _, cx| {
            let Some((original, modified)) = args.first().and_then(|context| Some((line_range(&context["mapping"]["original"])?, line_range(&context["mapping"]["modified"])?))) else {
                return crate::commands::Outcome::done();
            };
            workbench.revert_block(original, modified, cx)
        });
    }

    /// gutterFeature.ts `width`: 35px while the hunk toolbar has actions and `diffEditor.renderGutterMenu` is on.
    pub(super) fn diff_gutter_width(&self, input: &DiffInput) -> f32 {
        let shown = self.setting("diffEditor.renderGutterMenu") != json!(false) && !self.hunk_actions(input).is_empty();
        if shown { GUTTER_WIDTH } else { 0. }
    }

    /// `MenuId.DiffEditorHunkToolbar`: Revert Block where the modified side is writable (diffEditor.contribution.ts,
    /// an arrow side by side and a discard icon inline), then the extensions' `primary` items.
    fn hunk_actions(&self, input: &DiffInput) -> Vec<HunkAction> {
        let mut actions = Vec::new();
        if self.readonly(&input.modified.scheme, &input.modified.path).is_none() {
            let icon = if input.inline_now { "discard" } else { "arrow-right" };
            actions.push(HunkAction { command: REVERT.into(), icon: icon.into(), title: "Revert Block".into() });
        }
        let mut context = std::collections::HashMap::new();
        self.insert_diff_editor_context(&mut context);
        for action in menu_actions(&self.menus, MenuId::DiffEditorHunkToolbar, &context) {
            let MenuAction::Command { id, title, enabled: true, .. } = action else { continue };
            let icon = match self.command_icons.get(&id) {
                Some(crate::menus::CommandIcon::Codicon(name)) => name.clone(),
                _ => "symbol-misc".into(),
            };
            actions.push(HunkAction { command: id, icon, title });
        }
        actions
    }

    /// The gutter column over the modified editor's rows: per change a 2px line over its lines and the zones above
    /// them, and its toolbar (`DiffToolBar.layout`) centered on it, kept inside the change and the view.
    pub(super) fn diff_gutter(&self, input: &DiffInput, modified: &OpenEditor, cx: &mut Context<Self>) -> Div {
        let mut gutter = div().w(px(GUTTER_WIDTH)).flex_none().h_full().relative().overflow_hidden().group("diff-gutter");
        let Some(computed) = &input.diff else { return gutter };
        let actions = self.hunk_actions(input);
        let viewport = self.editors.viewport;
        let view_height = viewport.height;
        let cursor = modified.selection().head.line + 1;
        let line_count = modified.document().lines.len();
        let top_of = |line: usize| match line {
            line if line < line_count => modified.line_top(modified.view_line(line), viewport),
            _ => modified.line_top(modified.view_line(line_count - 1), viewport) + viewport.line_height,
        };
        // getTopForLineNumber(includeViewZones): the zones above the line belong to the change.
        let zones_above = |line: usize| match line {
            0 => top_of(0) - viewport.line_top(0),
            line if line < line_count => top_of(line) - top_of(line - 1) - viewport.line_height,
            _ => 0.,
        };
        let separator = self.color("menu.separatorBackground");
        for (index, change) in computed.diff.changes.iter().enumerate() {
            let start = change.modified.start - 1;
            let top = top_of(start) - zones_above(start) - modified.scroll.top;
            let bottom = top_of(change.modified.end - 1).max(top_of(start)) - modified.scroll.top;
            let height = bottom - top;
            if bottom < 0. || top > view_height {
                continue;
            }
            let small = change.original.start == 1 && height < 30.;
            let max_items = if small { 1 } else { 3 };
            let (primary, overflow) = if actions.len() >= max_items { actions.split_at(max_items - 1) } else { (actions.as_slice(), &[][..]) };
            let items = primary.len() + usize::from(!overflow.is_empty());
            let buttons_top = toolbar_top(top, height, items as f32 * BUTTON_HEIGHT, view_height) - top;
            let show_always = change.modified.contains(cursor);
            let context = hunk_context(self, input, change);
            let mut toolbar = div().absolute().top(px(buttons_top)).w_full().flex().justify_center();
            let mut buttons = div().flex().flex_col().rounded(px(4.)).bg(self.color("editorGutter.itemBackground"));
            for action in primary {
                buttons = buttons.child(self.hunk_button(index, &action.icon, &action.title, &action.command, context.clone(), cx));
            }
            if !overflow.is_empty() {
                let overflow = overflow.to_vec();
                let context = context.clone();
                buttons = buttons.child(self.hunk_button(index, "ellipsis", "More Actions...", "", Value::Null, cx).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |workbench, event: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        let actions = overflow.iter().map(|action| MenuAction::Command { id: action.command.clone(), title: action.title.clone(), enabled: true, checked: false, alt: None }).collect();
                        let anchor: Bounds<Pixels> = Bounds::new(event.position, Default::default());
                        workbench.show_context_menu_actions(actions, &Default::default(), anchor, vec![context.clone()], window, cx);
                    }),
                ));
            }
            toolbar = toolbar.child(buttons);
            let item = div()
                .absolute()
                .top(px(top))
                .h(px(height))
                .left_0()
                .w(px(GUTTER_WIDTH - 1.))
                .when(!show_always, |item| item.opacity(0.).group_hover("diff-gutter", |item| item.opacity(1.)))
                .child(div().absolute().top_0().bottom_0().left(px(((GUTTER_WIDTH - 1.) / 2.).floor())).w(px(2.)).bg(separator))
                .child(toolbar);
            gutter = gutter.child(item);
        }
        gutter
    }

    fn hunk_button(&self, index: usize, icon: &str, title: &str, command: &str, context: Value, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let id = gpui::SharedString::from(format!("diff-hunk-{index}-{command}-{icon}"));
        let mut button = div()
            .id(id.clone())
            .px(px(2.))
            .py(px(1.))
            .rounded(px(4.))
            .cursor_pointer()
            .hover(|button| button.bg(self.color("toolbar.hoverBackground")))
            .child(self.icon(icon, 16., 16., self.color("editorGutter.itemGlyphForeground")))
            .child(self.hover_target(gpui::SharedString::from(format!("{id}-hover")), HoverContent::Text(title.to_string()), HoverOptions::mouse()));
        if !command.is_empty() {
            let command = command.to_string();
            button = button.on_mouse_down(MouseButton::Left, cx.listener(move |workbench, _: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                drop(workbench.execute_command(&command, vec![context.clone()], window, cx));
            }));
        }
        button
    }

    /// commands.ts `RevertHunkOrSelection` of a whole block: the modified lines get the original's.
    fn revert_block(&mut self, original: diff::LineRange, modified: diff::LineRange, cx: &mut Context<Self>) -> crate::commands::Outcome {
        let Some(input) = self.editors.active_diff() else { return crate::commands::Outcome::done() };
        let Some(original_model) = self.editors.model(&input.original.scheme, &input.original.path) else { return crate::commands::Outcome::done() };
        let original_lines: Vec<String> = original_model.document().lines[original.start - 1..original.end - 1].to_vec();
        self.run_indenting(
            |editor, _| {
                let eol = editor.document().eol;
                let (start, end, text) = block_edit(&editor.document().lines, modified, &original_lines, eol);
                editor.execute_edits(vec![(start, end, text, false)], None, EditReason::Unknown("revert")).into_iter().collect()
            },
            cx,
        )
    }
}

/// The edit replacing the modified `lines` with `replacement`: whole lines, an empty range inserting before its line
/// (or after the last), an empty replacement deleting the lines with one line break.
fn block_edit(lines: &[String], range: diff::LineRange, replacement: &[String], eol: &str) -> (Position, Position, String) {
    let (start, end) = (range.start - 1, range.end - 1);
    let joined = replacement.join(eol);
    if end < lines.len() {
        let text = if replacement.is_empty() { String::new() } else { format!("{joined}{eol}") };
        return (Position::new(start, 0), Position::new(end, 0), text);
    }
    // The block reaches the end of the model: the break before it goes or comes with the text.
    let last = lines.len() - 1;
    let end_position = Position::new(last, lines[last].chars().count());
    match (start, replacement.is_empty()) {
        (0, _) => (Position::new(0, 0), end_position, joined),
        (start, true) => (Position::new(start - 1, lines[start - 1].chars().count()), end_position, String::new()),
        (start, false) if start > last => (end_position, end_position, format!("{eol}{joined}")),
        (start, false) => (Position::new(start, 0), end_position, joined),
    }
}

/// `{ startLineNumber, endLineNumberExclusive }` as the extension host gets a `LineRange`.
fn line_range(value: &Value) -> Option<diff::LineRange> {
    Some(diff::LineRange::new(value["startLineNumber"].as_u64()? as usize, value["endLineNumberExclusive"].as_u64()? as usize))
}

/// `DiffEditorSelectionHunkToolbarContext`: the change, the original with the change applied (`computeStagedValue`)
/// and both resources.
fn hunk_context(workbench: &Workbench, input: &DiffInput, change: &diff::LineRangeMapping) -> Value {
    let range = |range: diff::LineRange| json!({ "startLineNumber": range.start, "endLineNumberExclusive": range.end });
    let staged = match (workbench.editors.model(&input.original.scheme, &input.original.path), workbench.editors.model(&input.modified.scheme, &input.modified.path)) {
        (Some(original), Some(modified)) => {
            let mut lines = original.document().lines.clone();
            let replacement = modified.document().lines[change.modified.start - 1..change.modified.end - 1].to_vec();
            lines.splice(change.original.start - 1..change.original.end - 1, replacement);
            lines.join(original.document().eol)
        }
        _ => String::new(),
    };
    json!({
        "mapping": { "original": range(change.original), "modified": range(change.modified) },
        "originalWithModifiedChanges": staged,
        "originalUri": crate::editor::uri_value(&input.original.scheme, &input.original.path),
        "modifiedUri": crate::editor::uri_value(&input.modified.scheme, &input.modified.path),
    })
}

/// `DiffToolBar.layout`: centered on the item, then clipped to the view and to the item, a toolbar height away
/// from their edges, where the item has room for that.
fn toolbar_top(item_top: f32, item_height: f32, toolbar_height: f32, view_height: f32) -> f32 {
    let margin = toolbar_height;
    let centered = item_top + item_height / 2. - toolbar_height / 2.;
    let view = (margin, view_height - margin - toolbar_height);
    let parent = (item_top + margin, item_top + item_height - toolbar_height - margin);
    if view.0 <= view.1 && parent.0 < parent.1 {
        return centered.clamp(view.0, view.1).clamp(parent.0, parent.1);
    }
    centered
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.split('\n').map(str::to_string).collect()
    }

    fn apply(text: &str, range: (usize, usize), replacement: &str) -> String {
        let lines = lines(text);
        let replacement = if replacement.is_empty() { Vec::new() } else { self::lines(replacement) };
        let (start, end, new_text) = block_edit(&lines, diff::LineRange::new(range.0, range.1), &replacement, "\n");
        let offset = |position: Position| lines[..position.line].iter().map(|line| line.len() + 1).sum::<usize>() + position.column;
        let mut result = text.to_string();
        result.replace_range(offset(start)..offset(end), &new_text);
        result
    }

    #[test]
    fn block_edits_replace_whole_lines() {
        assert_eq!(apply("a\nB\nc", (2, 3), "b"), "a\nb\nc");
        assert_eq!(apply("a\nnew\nc", (2, 3), ""), "a\nc");
        assert_eq!(apply("a\nc", (2, 2), "b"), "a\nb\nc");
        assert_eq!(apply("a\nnew", (2, 3), ""), "a");
        assert_eq!(apply("a", (2, 2), "b"), "a\nb");
        assert_eq!(apply("x\ny", (1, 3), "a"), "a");
    }

    #[test]
    fn the_toolbar_stays_inside_its_change() {
        // A tall change centers the toolbar; scrolled mostly above the view it moves down, but stays a toolbar
        // height above the change's bottom.
        assert_eq!(toolbar_top(100., 200., 18., 600.), 191.);
        assert_eq!(toolbar_top(-150., 200., 18., 600.), 14.);
        assert_eq!(toolbar_top(-100., 200., 18., 600.), 18.);
        // A change too short for the margins keeps the centered position.
        assert_eq!(toolbar_top(0., 18., 18., 600.), 0.);
    }
}
