//! The text of a single-line input element (base/browser/ui/inputbox/inputBox.ts): the value, its selection
//! as the browser keeps it, and the element's own undo. The quick input and the Explorer's inline editor
//! each own one; workbench/input_box_view.rs draws it and maps keys to these edits.

/// Caret moves of a single-line text input (macOS: Option+arrow by word, Cmd+arrow to the ends).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Motion {
    Left,
    Right,
    WordLeft,
    WordRight,
    Home,
    End,
}

#[derive(Clone, Default, Debug)]
pub struct InputBox {
    pub value: String,
    /// Anchor and caret, char offsets into `value`.
    pub selection: (usize, usize),
    /// Values and selections before each edit, a run of typing as one step.
    undo_stack: Vec<(String, (usize, usize))>,
    redo_stack: Vec<(String, (usize, usize))>,
    typing: bool,
}

impl InputBox {
    pub fn new(value: String) -> InputBox {
        InputBox { value, ..InputBox::default() }
    }

    /// Assigning the element's value: the caret goes to its end.
    pub fn set_value(&mut self, value: String) {
        self.value = value;
        let end = self.len();
        self.selection = (end, end);
    }

    pub fn len(&self) -> usize {
        self.value.chars().count()
    }

    /// `setSelectionRange`, clamped to the value.
    pub fn select(&mut self, start: usize, end: usize) {
        let len = self.len();
        self.selection = (start.min(len), end.min(len));
    }

    pub fn selected_range(&self) -> std::ops::Range<usize> {
        self.selection.0.min(self.selection.1)..self.selection.0.max(self.selection.1)
    }

    /// Typing or pasting: the text replaces the selection (or `range`, char offsets) and the caret goes after it.
    pub fn insert(&mut self, range: Option<std::ops::Range<usize>>, text: &str) {
        let range = range.unwrap_or_else(|| self.selected_range());
        let typing = range.is_empty() && !text.is_empty();
        if !(typing && self.typing) {
            self.undo_stack.push((self.value.clone(), self.selection));
        }
        self.typing = typing;
        self.redo_stack.clear();
        let chars: Vec<char> = self.value.chars().collect();
        let end = range.end.min(chars.len());
        let start = range.start.min(end);
        self.value = chars[..start].iter().copied().chain(text.chars()).chain(chars[end..].iter().copied()).collect();
        let caret = start + text.chars().count();
        self.selection = (caret, caret);
    }

    /// Moves the caret, or with `extend` the selection's moving end.
    pub fn move_caret(&mut self, motion: Motion, extend: bool) {
        let range = self.selected_range();
        let target = match motion {
            // Without shift an arrow collapses a selection to the side it points to.
            Motion::Left if !extend && !range.is_empty() => range.start,
            Motion::Right if !extend && !range.is_empty() => range.end,
            _ => self.motion_target(motion),
        };
        self.selection = if extend { (self.selection.0, target) } else { (target, target) };
        self.typing = false;
    }

    /// Cmd+Z and Shift+Cmd+Z in the input element.
    pub fn undo(&mut self, redo: bool) {
        let (from, to) = if redo { (&mut self.redo_stack, &mut self.undo_stack) } else { (&mut self.undo_stack, &mut self.redo_stack) };
        let Some((value, selection)) = from.pop() else { return };
        to.push((std::mem::replace(&mut self.value, value), self.selection));
        self.typing = false;
        self.selection = selection;
    }

    pub fn caret_at_end(&self) -> bool {
        let end = self.len();
        self.selection == (end, end)
    }

    /// Backspace and Delete with their word and line variants: the selection, else up to the motion's target.
    pub fn delete(&mut self, motion: Motion) {
        let range = self.selected_range();
        if range.is_empty() {
            let target = self.motion_target(motion);
            self.insert(Some(target.min(range.start)..target.max(range.start)), "");
        } else {
            self.insert(Some(range), "");
        }
    }

    pub fn select_all(&mut self) {
        self.selection = (0, self.len());
    }

    /// A double click: the word at `index`.
    pub fn select_word_at(&mut self, index: usize) {
        self.selection = (index, index);
        self.move_caret(Motion::WordLeft, false);
        self.move_caret(Motion::WordRight, true);
    }

    pub fn selected_text(&self) -> String {
        let range = self.selected_range();
        self.value.chars().skip(range.start).take(range.len()).collect()
    }

    fn motion_target(&self, motion: Motion) -> usize {
        let chars: Vec<char> = self.value.chars().collect();
        let caret = self.selection.1.min(chars.len());
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        match motion {
            Motion::Left => caret.saturating_sub(1),
            Motion::Right => (caret + 1).min(chars.len()),
            Motion::Home => 0,
            Motion::End => chars.len(),
            Motion::WordLeft => {
                let mut at = caret;
                while at > 0 && !is_word(chars[at - 1]) {
                    at -= 1;
                }
                while at > 0 && is_word(chars[at - 1]) {
                    at -= 1;
                }
                at
            }
            Motion::WordRight => {
                let mut at = caret;
                while at < chars.len() && !is_word(chars[at]) {
                    at += 1;
                }
                while at < chars.len() && is_word(chars[at]) {
                    at += 1;
                }
                at
            }
        }
    }
}
