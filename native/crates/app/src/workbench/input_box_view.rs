//! The input element of `InputBox` as the workbench draws it: the value with its selection, IME marked text
//! and blinking caret, its caret keys, mouse selection, text commands and the platform input handler. The quick
//! input, the Explorer's inline editor and the editor find widget's two inputs each own an `InputBox` and an `InputView`.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    Context, ElementInputHandler, FocusHandle, Font, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, ShapedLine, Task, TextRun, UTF16Selection, UnderlineStyle,
    Window, canvas, div, fill, point, prelude::*, px, size,
};

use super::Workbench;
use super::search_editor_widget::SEARCH_EDITOR_FIELDS;
use super::search_view::{SEARCH_FIELDS, SearchField};
use crate::commands::Outcome;
use crate::input_box::{InputBox, Motion};

/// Which input element: the quick input's, the Explorer's inline editor, the find widget's, one of the Search view's
/// inputs, one of the search editor's query widget, or the Keyboard Shortcuts editor's.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InputId {
    QuickInput,
    Explorer,
    Find,
    Replace,
    Search(SearchField),
    KeybindingsSearch,
    KeybindingsWhen,
    SearchEditor(SearchField),
}

const INPUTS: [InputId; 6] = [InputId::QuickInput, InputId::Explorer, InputId::Find, InputId::Replace, InputId::KeybindingsSearch, InputId::KeybindingsWhen];

/// The view state of one input element.
pub struct InputView {
    /// The IME's marked text, a UTF-16 range of the value.
    pub marked: Option<Range<usize>>,
    selecting: bool,
    caret_moved: Instant,
    blink: Option<Task<()>>,
    /// The shaped text, its window x, and the scroll that keeps the caret visible.
    text: Rc<RefCell<InputText>>,
}

#[derive(Default)]
struct InputText {
    line: Option<ShapedLine>,
    left: f32,
    scroll: f32,
}

impl Default for InputView {
    fn default() -> Self {
        InputView { marked: None, selecting: false, caret_moved: Instant::now(), blink: None, text: Rc::default() }
    }
}

impl InputView {
    /// The element hides: the IME composition and the caret blink end.
    pub fn reset(&mut self) {
        self.marked = None;
        self.selecting = false;
        self.blink = None;
        self.text = Rc::default();
    }
}

/// What the input element draws besides its value.
pub struct InputLook {
    pub password: bool,
    pub placeholder: Option<String>,
    pub focused: bool,
}

impl Workbench {
    pub(super) fn input_view(&self, id: InputId) -> &InputView {
        match id {
            InputId::QuickInput => &self.quick_inputs.input,
            InputId::Explorer => &self.explorer.input,
            InputId::Find => &self.find.find_view,
            InputId::Replace => &self.find.replace_view,
            InputId::Search(field) => &self.search.input(field).view,
            InputId::KeybindingsSearch => &self.keybindings_editor.search_view,
            InputId::KeybindingsWhen => &self.keybindings_editor.when_view,
            InputId::SearchEditor(field) => &self.search_editor_widget.view.input(field).view,
        }
    }

    pub(super) fn input_view_mut(&mut self, id: InputId) -> &mut InputView {
        match id {
            InputId::QuickInput => &mut self.quick_inputs.input,
            InputId::Explorer => &mut self.explorer.input,
            InputId::Find => &mut self.find.find_view,
            InputId::Replace => &mut self.find.replace_view,
            InputId::Search(field) => &mut self.search.input_mut(field).view,
            InputId::KeybindingsSearch => &mut self.keybindings_editor.search_view,
            InputId::KeybindingsWhen => &mut self.keybindings_editor.when_view,
            InputId::SearchEditor(field) => &mut self.search_editor_widget.view.input_mut(field).view,
        }
    }

    pub(super) fn input_focus(&self, id: InputId) -> &FocusHandle {
        match id {
            InputId::QuickInput => &self.quick_inputs.focus,
            InputId::Explorer => &self.explorer.input_focus,
            InputId::Find => &self.find.find_focus,
            InputId::Replace => &self.find.replace_focus,
            InputId::Search(field) => &self.search.input(field).focus,
            InputId::KeybindingsSearch => &self.keybindings_editor.search_focus,
            InputId::KeybindingsWhen => &self.keybindings_editor.when_focus,
            InputId::SearchEditor(field) => &self.search_editor_widget.view.input(field).focus,
        }
    }

    fn input_box(&self, id: InputId) -> Option<&InputBox> {
        match id {
            InputId::QuickInput => self.quick_inputs.current().map(|input| &input.input_box),
            InputId::Explorer => self.explorer.edit.as_ref().map(|edit| &edit.input),
            InputId::Find => Some(&self.find.find_input),
            InputId::Replace => Some(&self.find.replace_input),
            InputId::Search(field) => Some(&self.search.input(field).value),
            InputId::KeybindingsSearch => Some(&self.keybindings_editor.search),
            InputId::KeybindingsWhen => self.keybindings_editor.when.as_ref().map(|when| &when.input),
            InputId::SearchEditor(field) => Some(&self.search_editor_widget.view.input(field).value),
        }
    }

    /// The input element with the keyboard focus.
    pub(super) fn focused_input(&self, window: &Window) -> Option<InputId> {
        INPUTS.into_iter().chain(SEARCH_FIELDS.map(InputId::Search)).chain(SEARCH_EDITOR_FIELDS.map(InputId::SearchEditor)).find(|id| self.input_focus(*id).is_focused(window))
    }

    /// Any input element in an IME composition: the IME owns the keys.
    pub(super) fn input_composing(&self) -> bool {
        INPUTS.into_iter().chain(SEARCH_FIELDS.map(InputId::Search)).chain(SEARCH_EDITOR_FIELDS.map(InputId::SearchEditor)).any(|id| self.input_view(id).marked.is_some())
    }

    /// Runs `edit` on the element's value and restarts the caret blink.
    pub(super) fn edit_input_box(&mut self, id: InputId, window: &mut Window, cx: &mut Context<Self>, edit: impl FnOnce(&mut InputBox)) -> Outcome {
        self.input_view_mut(id).caret_moved = Instant::now();
        match id {
            InputId::QuickInput => self.with_current_quick_input(window, cx, |input| input.edit_input(edit)),
            InputId::Explorer => {
                self.edit_explorer_input(edit, cx);
                Outcome::done()
            }
            InputId::Find | InputId::Replace => {
                self.edit_find_input(id, edit, window, cx);
                Outcome::done()
            }
            InputId::Search(field) => {
                self.edit_search_input(field, edit, window, cx);
                Outcome::done()
            }
            InputId::KeybindingsSearch => {
                self.edit_keybindings_search(edit, window, cx);
                Outcome::done()
            }
            InputId::KeybindingsWhen => {
                self.edit_when_input(edit, cx);
                Outcome::done()
            }
            InputId::SearchEditor(field) => {
                self.edit_search_editor_input(field, edit, window, cx);
                Outcome::done()
            }
        }
    }

    /// The caret blinks while the input has focus; a key restarts the blink.
    pub(super) fn blink_caret(&mut self, id: InputId, window: &mut Window, cx: &mut Context<Self>) {
        self.input_view_mut(id).caret_moved = Instant::now();
        self.input_view_mut(id).blink = Some(cx.spawn_in(window, async move |workbench, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(500)).await;
                if workbench.update(cx, |_, cx| cx.notify()).is_err() {
                    return;
                }
            }
        }));
    }

    /// The text commands of the focused input element by their editor ids (`MultiCommand`s fall back to the native input).
    pub(super) fn input_text_command(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) -> Option<Outcome> {
        let id = self.focused_input(window)?;
        let selected = self.input_box(id)?.selected_text();
        let password = id == InputId::QuickInput && self.quick_inputs.current().is_some_and(|input| input.password);
        match command {
            "editor.action.clipboardCopyAction" | "editor.action.clipboardCutAction" if selected.is_empty() || password => return Some(Outcome::done()),
            "editor.action.clipboardCopyAction" | "editor.action.clipboardCutAction" => cx.write_to_clipboard(gpui::ClipboardItem::new_string(selected)),
            _ => {}
        }
        let pasted = cx.read_from_clipboard().and_then(|item| item.text()).unwrap_or_default();
        Some(self.edit_input_box(id, window, cx, |input| match command {
            "undo" => input.undo(false),
            "redo" => input.undo(true),
            "editor.action.selectAll" => input.select_all(),
            "editor.action.clipboardCutAction" => input.insert(None, ""),
            // A single-line input drops the line breaks of what is pasted.
            "editor.action.clipboardPasteAction" => input.insert(None, &pasted.replace(['\r', '\n'], "")),
            _ => {}
        }))
    }

    /// Keys of the input element that no keybinding took: caret moves and deletes.
    fn input_key_down(&mut self, id: InputId, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        if modifiers.control || self.input_view(id).marked.is_some() {
            return;
        }
        let word = modifiers.alt;
        let line = modifiers.platform;
        let motion = |back: bool| match (back, line, word) {
            (true, true, _) => Motion::Home,
            (false, true, _) => Motion::End,
            (true, false, true) => Motion::WordLeft,
            (false, false, true) => Motion::WordRight,
            (true, false, false) => Motion::Left,
            (false, false, false) => Motion::Right,
        };
        if let InputId::Search(field) = id
            && self.search_input_key(field, keystroke, window, cx)
        {
            cx.stop_propagation();
            return;
        }
        if let InputId::SearchEditor(field) = id
            && self.search_editor_input_key(field, keystroke, window, cx)
        {
            cx.stop_propagation();
            return;
        }
        if id == InputId::Explorer && self.explorer_input_key(&keystroke.key, window, cx) {
            cx.stop_propagation();
            return;
        }
        if matches!(id, InputId::Find | InputId::Replace) && self.find_input_key(id, keystroke, window, cx) {
            cx.stop_propagation();
            return;
        }
        let extend = modifiers.shift;
        let edit: Box<dyn FnOnce(&mut InputBox)> = match keystroke.key.as_str() {
            "left" => Box::new(move |input| input.move_caret(motion(true), extend)),
            "right" => Box::new(move |input| input.move_caret(motion(false), extend)),
            "home" | "up" if keystroke.key == "home" || line => Box::new(move |input| input.move_caret(Motion::Home, extend)),
            "end" | "down" if keystroke.key == "end" || line => Box::new(move |input| input.move_caret(Motion::End, extend)),
            "backspace" => Box::new(move |input| input.delete(motion(true))),
            "delete" => Box::new(move |input| input.delete(motion(false))),
            // Tab moves the focus between the widget's controls; the input is the only one the port draws focusable.
            "tab" => Box::new(|_| {}),
            _ => return,
        };
        cx.stop_propagation();
        self.edit_input_box(id, window, cx, edit);
    }

    /// A press in the input text puts the caret there; a double press selects the word, a triple one all.
    fn input_mouse_down(&mut self, id: InputId, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.input_focus(id).clone(), cx);
        let index = self.input_index_at(id, f32::from(event.position.x));
        self.input_view_mut(id).selecting = true;
        let extend = event.modifiers.shift;
        let clicks = event.click_count;
        self.edit_input_box(id, window, cx, |input| match clicks {
            1 if extend => input.selection.1 = index,
            1 => input.selection = (index, index),
            2 => input.select_word_at(index),
            _ => input.select_all(),
        });
    }

    fn input_mouse_move(&mut self, id: InputId, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.input_view(id).selecting || event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let index = self.input_index_at(id, f32::from(event.position.x));
        self.edit_input_box(id, window, cx, |input| input.selection.1 = index);
    }

    /// The char offset of the value closest to a window x.
    fn input_index_at(&self, id: InputId, x: f32) -> usize {
        let text = self.input_view(id).text.borrow();
        let Some(line) = text.line.as_ref() else { return 0 };
        let byte = line.closest_index_for_x(px(x - text.left + text.scroll));
        line.text[..byte.min(line.text.len())].chars().count()
    }

    /// The input handler's view of the value: UTF-16 offsets.
    pub(super) fn input_text_for_range(&self, id: InputId, range: Range<usize>, adjusted_range: &mut Option<Range<usize>>) -> Option<String> {
        let units: Vec<u16> = self.input_box(id)?.value.encode_utf16().collect();
        let range = range.start.min(units.len())..range.end.min(units.len());
        *adjusted_range = Some(range.clone());
        Some(String::from_utf16_lossy(&units[range]))
    }

    pub(super) fn input_selected_range(&self, id: InputId) -> Option<UTF16Selection> {
        let input = self.input_box(id)?;
        let (anchor, head) = (chars_to_utf16(&input.value, input.selection.0), chars_to_utf16(&input.value, input.selection.1));
        Some(UTF16Selection { range: anchor.min(head)..anchor.max(head), reversed: head < anchor })
    }

    /// Typed text and IME composition; marked text replaces the previous marked text and stays marked.
    pub(super) fn input_replace(&mut self, id: InputId, range: Option<Range<usize>>, text: &str, mark: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.input_box(id) else { return };
        let range = range.or(self.input_view(id).marked.clone()).map(|range| utf16_to_chars(&input.value, range.start)..utf16_to_chars(&input.value, range.end));
        let start = range.as_ref().map_or(input.selected_range().start, |range| range.start);
        let start_utf16 = chars_to_utf16(&input.value, start);
        self.input_view_mut(id).marked = (mark && !text.is_empty()).then(|| start_utf16..start_utf16 + text.encode_utf16().count());
        self.edit_input_box(id, window, cx, |input| input.insert(range, text));
    }

    /// The input element: the value (or placeholder) with its selection, marked text and caret.
    pub(super) fn input_text(&self, id: InputId, look: InputLook, cx: &mut Context<Self>) -> impl IntoElement {
        let input = self.input_box(id).cloned().unwrap_or_default();
        let view = self.input_view(id);
        let display: String = if look.password { input.value.chars().map(|_| '\u{2022}').collect() } else { input.value.clone() };
        let byte_at = |chars: usize| display.char_indices().nth(chars).map_or(display.len(), |(byte, _)| byte);
        let selection = byte_at(input.selected_range().start)..byte_at(input.selected_range().end);
        let caret = byte_at(input.selection.1);
        let marked = view.marked.clone().map(|range| byte_at(utf16_to_chars(&input.value, range.start))..byte_at(utf16_to_chars(&input.value, range.end)));
        let placeholder = look.placeholder.filter(|_| input.value.is_empty());
        let text_color = self.foreground_or("input.foreground");
        let placeholder_color = self.color("input.placeholderForeground");
        let selection_color = self.color(if look.focused { "selection.background" } else { "editor.inactiveSelectionBackground" });
        let selection_color = if self.theme.color("selection.background").is_some() { selection_color } else { self.color("editor.selectionBackground") };
        // Native carets blink at a 1s period; edits and moves restart it.
        let caret_on = look.focused && !self.input_carets_hidden && (view.caret_moved.elapsed().as_millis() / 500) % 2 == 0;
        let layout = view.text.clone();
        let focus = self.input_focus(id).clone();
        let workbench = cx.entity();
        div()
            .id(match id {
                InputId::QuickInput => gpui::ElementId::from("quick-input-text"),
                InputId::Explorer => "explorer-input-text".into(),
                InputId::Find => "find-input-text".into(),
                InputId::Replace => "replace-input-text".into(),
                InputId::Search(field) => ("search-input-text", field as usize).into(),
                InputId::KeybindingsSearch => "keybindings-search-text".into(),
                InputId::KeybindingsWhen => "keybindings-when-text".into(),
                InputId::SearchEditor(field) => ("search-editor-input-text", field as usize).into(),
            })
            .flex_1()
            .h_full()
            .overflow_hidden()
            // inputBox.css: the input element has the text cursor.
            .cursor_text()
            .track_focus(&focus)
            .on_key_down(cx.listener(move |workbench, event, window, cx| workbench.input_key_down(id, event, window, cx)))
            .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, event, window, cx| workbench.input_mouse_down(id, event, window, cx)))
            .on_mouse_move(cx.listener(move |workbench, event, window, cx| workbench.input_mouse_move(id, event, window, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(move |workbench, _, _, _| workbench.input_view_mut(id).selecting = false))
            .on_mouse_up_out(MouseButton::Left, cx.listener(move |workbench, _, _, _| workbench.input_view_mut(id).selecting = false))
            .child(
                canvas(
                    move |_, _, _| (),
                    move |bounds, _, window, cx| {
                        let font: Font = window.text_style().font();
                        let font_size = window.text_style().font_size.to_pixels(window.rem_size());
                        let line_height = px(18.);
                        let origin_y = bounds.origin.y + (bounds.size.height - line_height) / 2.;
                        if let Some(placeholder) = placeholder {
                            let run = TextRun { len: placeholder.len(), font: font.clone(), color: placeholder_color.into(), background_color: None, underline: None, strikethrough: None };
                            let line = window.text_system().shape_line(placeholder.into(), font_size, &[run], None);
                            let _ = line.paint(point(bounds.origin.x, origin_y), line_height, gpui::TextAlign::Left, None, window, cx);
                        }
                        let mut runs = Vec::new();
                        let plain = |len: usize| TextRun { len, font: font.clone(), color: text_color.into(), background_color: None, underline: None, strikethrough: None };
                        match &marked {
                            Some(marked) if !marked.is_empty() => {
                                runs.push(plain(marked.start));
                                let underline = UnderlineStyle { thickness: px(1.), color: Some(text_color.into()), wavy: false };
                                runs.push(TextRun { underline: Some(underline), ..plain(marked.len()) });
                                runs.push(plain(display.len() - marked.end));
                            }
                            _ => runs.push(plain(display.len())),
                        }
                        runs.retain(|run| run.len > 0);
                        let line = window.text_system().shape_line(display.clone().into(), font_size, &runs, None);
                        let caret_x = f32::from(line.x_for_index(caret));
                        let width = f32::from(bounds.size.width);
                        let mut text = layout.borrow_mut();
                        let mut scroll = text.scroll;
                        if caret_x - scroll > width - 1. {
                            scroll = caret_x - width + 1.;
                        }
                        if caret_x < scroll {
                            scroll = caret_x;
                        }
                        scroll = scroll.min((f32::from(line.width()) - width + 1.).max(0.)).max(0.);
                        let left = f32::from(bounds.origin.x) - scroll;
                        if !selection.is_empty() {
                            let start = f32::from(line.x_for_index(selection.start));
                            let end = f32::from(line.x_for_index(selection.end));
                            window.paint_quad(fill(gpui::Bounds::new(point(px(left + start), origin_y), size(px(end - start), line_height)), selection_color));
                        }
                        let _ = line.paint(point(px(left), origin_y), line_height, gpui::TextAlign::Left, None, window, cx);
                        if caret_on && selection.is_empty() {
                            window.paint_quad(fill(gpui::Bounds::new(point(px(left + caret_x), origin_y), size(px(1.), line_height)), text_color));
                        }
                        *text = InputText { line: Some(line), left: f32::from(bounds.origin.x), scroll };
                        drop(text);
                        window.handle_input(&focus, ElementInputHandler::new(bounds, workbench), cx);
                    },
                )
                .size_full(),
            )
    }
}

/// The char offset of a UTF-16 offset into `text`; an offset inside a surrogate pair rounds up.
pub fn utf16_to_chars(text: &str, offset: usize) -> usize {
    let mut units = 0;
    for (index, c) in text.chars().enumerate() {
        if units >= offset {
            return index;
        }
        units += c.len_utf16();
    }
    text.chars().count()
}

pub fn chars_to_utf16(text: &str, chars: usize) -> usize {
    text.chars().take(chars).map(char::len_utf16).sum()
}
