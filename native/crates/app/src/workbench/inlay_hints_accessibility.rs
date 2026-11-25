//! inlayHintsAccessibilty.ts: Read Line with Inlay Hints announces the cursor line with its hints in place, and
//! Escape stops the reading. shortcut: the line goes to the live region instead of a focused aria element, and its
//! hints are read as last resolved; upgrade when the port gets focusable synthetic accessibility nodes.

use serde_json::{Value, json};

use super::Workbench;
use crate::commands::Outcome;

/// `_read`: the text before a hint is cut once the line reaches this column.
const MAX_READ_COLUMN: usize = 750;

impl Workbench {
    pub(super) fn register_inlay_hints_accessibility(&mut self) {
        self.commands.register("inlayHints.startReadingLineWithHint", |workbench, _, _, cx| {
            workbench.start_inlay_hints_reading();
            cx.notify();
            Outcome::done()
        });
        self.commands.register("inlayHints.stopReadingLineWithHint", |workbench, _, window, cx| {
            workbench.inlay_hints.reading = false;
            window.focus(&workbench.editor_focus, cx);
            Outcome::done()
        });
    }

    /// `isReadingLineWithInlayHints` and `editorHasInlayHintsProvider`.
    pub(super) fn inlay_hints_accessibility_context(&self, context: &mut std::collections::HashMap<String, Value>) {
        context.insert("isReadingLineWithInlayHints".into(), json!(self.inlay_hints.reading));
        context.insert("editorHasInlayHintsProvider".into(), json!(self.has_inlay_hints_provider()));
    }

    /// `startInlayHintsReading`: no hints on the cursor line plays the No Inlay Hints signal.
    fn start_inlay_hints_reading(&mut self) {
        let Some(editor) = self.editors.active() else { return };
        let line = editor.selection().head.line;
        let hints = self.inlay_hints_for_line(line);
        if hints.is_empty() {
            self.play_signal("accessibility.signals.noInlayHints", true, None);
            return;
        }
        let text = line_with_hints(&editor.document().lines[line], &hints);
        self.aria_alert(&text);
        self.inlay_hints.reading = true;
    }
}

/// `_read`: the line's text with each hint's label at its column, cut with `…` past column 750.
fn line_with_hints(line: &str, hints: &[(usize, String)]) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut text = String::new();
    let mut start = 0;
    for (column, label) in hints {
        let column = (*column).min(chars.len());
        if column > start {
            text.extend(&chars[start..column]);
            start = column;
        }
        if start > MAX_READ_COLUMN {
            text.push('\u{2026}');
            return text;
        }
        text.push_str(label);
    }
    text.extend(&chars[start..]);
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hints_are_read_in_place() {
        let hints = [(11, ": number".to_owned()), (18, "left: ".to_owned())];
        assert_eq!(line_with_hints("const total = add(1, 2);", &hints), "const total: number = add(left: 1, 2);");
        assert_eq!(line_with_hints("abc", &[(0, "x".to_owned())]), "xabc");
        let long = "a".repeat(800);
        assert_eq!(line_with_hints(&long, &[(760, "h".to_owned())]), format!("{}\u{2026}", "a".repeat(760)));
    }
}
