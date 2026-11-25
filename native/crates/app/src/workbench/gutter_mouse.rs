//! mouseHandler.ts presses on the glyph margin and line numbers (`startedOnLineNumbers`): viewController.ts
//! `dispatchMouse` selects whole lines (`lineSelect`, `lineSelectDrag`), or with the multi-cursor modifier adds a
//! cursor on the line (`createCursor` with `wholeLine`) or extends the last one (`lastCursorLineSelect`).

use gpui::{Context, Div, InteractiveElement, MouseButton, MouseDownEvent, MouseMoveEvent};

use super::Workbench;
use crate::editor::{Position, Reveal, Selection};

impl Workbench {
    /// The gutter's mouse listeners for the row of `line`.
    pub(super) fn gutter_mouse(&self, gutter: Div, line: usize, cx: &mut Context<Self>) -> Div {
        gutter
            .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| {
                window.focus(&workbench.editor_focus, cx);
                workbench.gutter_line_select(line, event.modifiers, cx);
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| workbench.show_line_number_context_menu(line, event.position, window, cx)))
            .on_mouse_move(cx.listener(move |workbench, event: &MouseMoveEvent, _, cx| {
                if event.pressed_button == Some(MouseButton::Left) {
                    workbench.gutter_line_select_drag(line, cx);
                }
            }))
    }

    pub(super) fn gutter_line_select(&mut self, line: usize, modifiers: gpui::Modifiers, cx: &mut Context<Self>) {
        let multi_cursor = match self.multi_cursor_modifier_is_cmd() {
            true => modifiers.platform,
            false => modifiers.alt,
        };
        let editor = self.editors.active_mut().expect("the gutter is clickable only with an editor open");
        let before = editor.selections.clone();
        let (start, end) = line_range(&editor.document().lines, line);
        match (multi_cursor, modifiers.shift) {
            (true, true) => editor.last_cursor_select_line(line),
            (true, false) => {
                let mut selections = editor.selections.clone();
                selections.push(Selection { anchor: start, head: end });
                editor.set_selections(selections);
            }
            (false, true) => {
                let anchor = editor.selection().anchor;
                self.line_select_start = Some((anchor, anchor));
                return self.gutter_line_select_drag(line, cx);
            }
            (false, false) => editor.set_selections(vec![Selection { anchor: start, head: end }]),
        }
        self.line_select_start = Some((start, end)).filter(|_| !multi_cursor);
        self.cursors_changed(before, Reveal::All, "mouse", cx);
    }

    /// `CursorMoveCommands.line` in selection mode: from the line range where the press started to `line`.
    pub(super) fn gutter_line_select_drag(&mut self, line: usize, cx: &mut Context<Self>) {
        let Some((entering_start, entering_end)) = self.line_select_start else { return };
        let editor = self.editors.active_mut().expect("the gutter is clickable only with an editor open");
        let before = editor.selections.clone();
        let (start, end) = line_range(&editor.document().lines, line);
        let head = match line.cmp(&entering_start.line) {
            std::cmp::Ordering::Less => start,
            std::cmp::Ordering::Greater => end,
            std::cmp::Ordering::Equal => entering_end,
        };
        // `SingleCursorState._computeSelection`: a head before the start range selects from its end.
        let anchor = if entering_start != entering_end && head <= entering_start { entering_end } else { entering_start };
        editor.set_selections(vec![Selection { anchor, head }]);
        self.cursors_changed(before, Reveal::All, "mouse", cx);
    }
}

/// The selection of a whole line: to the next line's start, or the last line's end.
fn line_range(lines: &[String], line: usize) -> (Position, Position) {
    let end = match line + 1 < lines.len() {
        true => Position::new(line + 1, 0),
        false => Position::new(line, lines[line].chars().count()),
    };
    (Position::new(line, 0), end)
}
