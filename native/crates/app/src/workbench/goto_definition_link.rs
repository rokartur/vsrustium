//! goToDefinitionAtPosition.ts and clickLinkGesture.ts: with the trigger modifier (Cmd, or Alt when
//! `editor.multiCursorModifier` is `ctrlCmd`) the word under the mouse that has a definition is drawn as a link, its
//! hover previews the definition's source, and a click goes to it (with the side-by-side modifier, to the side).

use std::ops::Range;

use gpui::{Context, Div, HighlightStyle, Hsla, Modifiers, Styled, Task, UnderlineStyle, Window, div, px};
use serde_json::json;

use super::Workbench;
use super::content_hover::{Anchor, Part, PartContent, Participant};
use super::goto_symbol::DEFINITION;
use crate::editor::{OpenEditor, Position, Selection, char_column, column_at_visible, expand_tabs, utf16_column, visible_column, word_at};
use crate::language_features::Candidate;
use crate::references_model::{Location, Range as DtoRange};

/// `GotoDefinitionAtPositionEditorContribution.MAX_SOURCE_PREVIEW_LINES`.
const MAX_SOURCE_PREVIEW_LINES: usize = 8;

#[derive(Default)]
pub(super) struct DefinitionLink {
    /// `_lastMouseMoveEvent`: the text position under the mouse, for a trigger key pressed later.
    last_move: Option<(usize, f32)>,
    trigger_held: bool,
    /// `_hasTriggerKeyOnMouseDown` and `_lineNumberOnMouseDown`.
    mouse_down: Option<(bool, usize)>,
    /// `currentWordAtPosition`: the model, line and word the request or the decoration is for.
    word: Option<(u32, usize, Range<usize>)>,
    decoration: Option<LinkDecoration>,
    /// The editor state the decoration was made in: a change of the text or a scroll removes it.
    state: Option<(u32, u32, f32, f32)>,
    /// The primary selection last seen: a change to a selection that is not empty resets the gesture (#7827).
    selection: Option<Selection>,
    /// The cursor `editor.action.showDefinitionPreviewHover` looked up the definition at (`toUnhookForKeyboard`).
    from_cursor: Option<Position>,
    hover_after_lookup: bool,
    /// `previousPromise`: dropping it cancels the provider calls.
    _request: Option<Task<()>>,
}

/// The `goto-definition-link` decoration on one line, with its markdown hover message.
struct LinkDecoration {
    model_id: u32,
    line: usize,
    columns: Range<usize>,
    hover: Option<String>,
}

impl Workbench {
    /// clickLinkGesture.ts `createOptions` on macOS: the trigger and side-by-side modifiers.
    fn link_modifiers(&self, modifiers: Modifiers) -> (bool, bool) {
        match self.multi_cursor_modifier_is_cmd() {
            true => (modifiers.alt, modifiers.platform),
            false => (modifiers.platform, modifiers.alt),
        }
    }

    /// `_onEditorMouseMove`; a move with a button held is a drag, which resets the gesture.
    pub(super) fn definition_link_mouse_moved(&mut self, line: usize, visible: f32, modifiers: Modifiers, pressed: bool, cx: &mut Context<Self>) {
        if pressed {
            return self.reset_definition_link(cx);
        }
        self.definition_link.last_move = Some((line, visible));
        let trigger = self.link_modifiers(modifiers).0;
        self.definition_link.trigger_held = trigger;
        self.start_find_definition_from_mouse(line, visible, trigger, cx);
    }

    pub(super) fn definition_link_mouse_left(&mut self) {
        self.definition_link.last_move = None;
    }

    /// `_onEditorKeyDown` and `_onEditorKeyUp` for the modifier keys: pressing the trigger key over a word looks it
    /// up, releasing it removes the link.
    pub(super) fn definition_link_modifiers_changed(&mut self, modifiers: Modifiers, cx: &mut Context<Self>) {
        let trigger = self.link_modifiers(modifiers).0;
        let pressed = trigger && !self.definition_link.trigger_held;
        self.definition_link.trigger_held = trigger;
        if !trigger {
            return self.cancel_definition_link(cx);
        }
        if let Some((line, visible)) = self.definition_link.last_move.filter(|_| pressed) {
            self.start_find_definition_from_mouse(line, visible, true, cx);
        }
    }

    /// `_onEditorKeyDown` for any other key: holding the trigger modifier with it removes the link, so Cmd+C over a
    /// word does not leave it underlined.
    pub(super) fn definition_link_key_down(&mut self, modifiers: Modifiers, cx: &mut Context<Self>) {
        if self.link_modifiers(modifiers).0 || self.definition_link.from_cursor.is_some() {
            self.cancel_definition_link(cx);
        }
    }

    pub(super) fn definition_link_mouse_down(&mut self, line: usize, modifiers: Modifiers) {
        self.definition_link.mouse_down = Some((self.link_modifiers(modifiers).0, line));
    }

    /// `_onEditorMouseUp` and `onExecute`: a click with the trigger modifier, pressed and released on one line,
    /// goes to the definition at the released position.
    pub(super) fn definition_link_mouse_up(&mut self, line: usize, visible: f32, modifiers: Modifiers, click_count: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((trigger_on_down, down_line)) = self.definition_link.mouse_down.take() else { return };
        let (trigger, side_by_side) = self.link_modifiers(modifiers);
        if !trigger_on_down || down_line != line || !trigger || click_count > 1 {
            return;
        }
        let Some(position) = self.link_target(line, visible) else { return };
        self.go_to_definition_at(position, side_by_side, window, cx);
        self.reset_definition_link(cx);
    }

    /// The gesture's reset on a model change, an edit, a scroll or a selection that is not empty.
    pub(super) fn follow_definition_link(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return self.reset_definition_link(cx) };
        let selection = editor.selection();
        let selecting = self.definition_link.selection.replace(selection).is_some_and(|last| last != selection) && !selection.is_empty();
        let Some(state) = self.definition_link.state else { return };
        let cursor_moved = self.definition_link.from_cursor.is_some_and(|position| position != selection.head);
        if (editor.model_id, editor.version_id, editor.scroll.top, editor.scroll.left) != state || selecting || cursor_moved {
            self.reset_definition_link(cx);
        }
    }

    /// `_resetHandler`.
    fn reset_definition_link(&mut self, cx: &mut Context<Self>) {
        self.definition_link.last_move = None;
        self.definition_link.mouse_down = None;
        self.cancel_definition_link(cx);
    }

    /// `onCancel`: the link goes and the word is forgotten.
    fn cancel_definition_link(&mut self, cx: &mut Context<Self>) {
        let link = &mut self.definition_link;
        if link.word.is_none() && link.decoration.is_none() {
            return;
        }
        link.word = None;
        link.from_cursor = None;
        link.hover_after_lookup = false;
        link.decoration = None;
        link.state = None;
        link._request = None;
        cx.notify();
    }

    /// `isEnabled` without the mouse button: the position under the mouse is on text (`CONTENT_TEXT`) of a model
    /// with a definition provider.
    fn link_target(&self, line: usize, visible: f32) -> Option<Position> {
        let editor = self.editors.active()?;
        let text = editor.document.lines.get(line)?;
        let tab_size = editor.document.indentation.tab_size;
        if visible >= visible_column(text, text.chars().count(), tab_size) as f32 {
            return None;
        }
        let uri = editor.document.uri();
        let candidate = Candidate { uri: &uri, language: &editor.document.language_id };
        if self.language_features.ordered(DEFINITION.register, &candidate).is_empty() {
            return None;
        }
        Some(Position::new(line, column_at_visible(text, visible, tab_size)))
    }

    /// `startFindDefinitionFromMouse` and `startFindDefinition`: the word under the mouse is looked up once; with
    /// a result it becomes the link.
    fn start_find_definition_from_mouse(&mut self, line: usize, visible: f32, trigger: bool, cx: &mut Context<Self>) {
        match self.link_target(line, visible).filter(|_| trigger) {
            Some(position) => _ = self.start_find_definition(position, cx),
            None => self.cancel_definition_link(cx),
        }
    }

    /// `startFindDefinition`: whether a lookup is under way.
    fn start_find_definition(&mut self, position: Position, cx: &mut Context<Self>) -> bool {
        let editor = self.editors.active().expect("the link is in the active editor");
        let line = position.line;
        let Some(word) = word_at(&editor.document.lines[line], position.column) else {
            self.cancel_definition_link(cx);
            return false;
        };
        let key = (editor.model_id, line, word.clone());
        if self.definition_link.word.as_ref() == Some(&key) {
            return false;
        }
        self.definition_link.word = Some(key);
        self.definition_link.decoration = None;
        let state = (editor.model_id, editor.version_id, editor.scroll.top, editor.scroll.left);
        self.definition_link.state = Some(state);
        let text = &editor.document.lines[line];
        let dto = json!({ "lineNumber": line + 1, "column": utf16_column(text, position.column) + 1 });
        let request = self.provide_locations(DEFINITION, editor, dto, false, cx);
        self.definition_link._request = Some(cx.spawn(async move |workbench, cx| {
            let links = request.await;
            let _ = workbench.update(cx, |workbench, cx| workbench.definitions_found(state, line, word, links, cx));
        }));
        cx.notify();
        true
    }

    pub(super) fn register_definition_link_commands(&mut self) {
        self.commands.register("editor.action.showDefinitionPreviewHover", |workbench, _, _, cx| {
            workbench.show_definition_preview_hover(cx);
            crate::commands::Outcome::done()
        });
    }

    /// hoverActions.ts ShowDefinitionPreviewHoverAction and `startFindDefinitionFromCursor`: the definition at the
    /// cursor is looked up as with Cmd over it, then the hover shows at the cursor; a cursor move or a key removes the link.
    fn show_definition_preview_hover(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        let head = editor.selection().head;
        let pending = self.start_find_definition(head, cx);
        self.definition_link.from_cursor = Some(head);
        self.definition_link.hover_after_lookup = pending;
        if !pending {
            self.show_content_hover_at_cursor(cx);
        }
    }

    fn definitions_found(&mut self, state: (u32, u32, f32, f32), line: usize, word: Range<usize>, links: Vec<Location>, cx: &mut Context<Self>) {
        self.set_definition_link(state, line, word, links, cx);
        if std::mem::take(&mut self.definition_link.hover_after_lookup) {
            self.show_content_hover_at_cursor(cx);
        }
    }

    /// The rest of `startFindDefinition`: several results link the combined origin ranges with a count, one result
    /// links its origin range (or the word) with a preview of its source.
    fn set_definition_link(&mut self, state: (u32, u32, f32, f32), line: usize, word: Range<usize>, links: Vec<Location>, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active().filter(|_| self.definition_link.state == Some(state)) else { return };
        let Some(first) = links.first() else {
            self.definition_link.decoration = None;
            return cx.notify();
        };
        let text = &editor.document.lines[line];
        let origin = |link: &Location| DtoRange::from_dto(&link.link["originSelectionRange"]);
        let to_columns = |range: DtoRange| char_column(text, range.start.1 - 1)..char_column(text, range.end.1 - 1);
        let mut columns = origin(first).filter(|range| range.start.0 == line + 1 && range.end.0 == line + 1).map_or(word, to_columns);
        let hover = if links.len() > 1 {
            for range in links.iter().filter_map(origin).filter(|range| range.start.0 == line + 1 && range.end.0 == line + 1) {
                let other = to_columns(range);
                columns = columns.start.min(other.start)..columns.end.max(other.end);
            }
            Some(format!("Click to show {} definitions.", links.len()))
        } else {
            // A target that cannot be read or a start outside it gets no link.
            let Some(preview) = self.definition_preview(first) else {
                self.definition_link.decoration = None;
                return cx.notify();
            };
            preview
        };
        self.definition_link.decoration = Some(LinkDecoration { model_id: state.0, line, columns, hover });
        cx.notify();
    }

    /// `getPreviewValue`: the definition's range (or, from 8 lines on, the lines up to the next one indented as its
    /// first) without the common indentation, in a code block of the file's language; `None` when the target cannot
    /// be read or the range starts outside it.
    fn definition_preview(&self, link: &Location) -> Option<Option<String>> {
        let open = self.editors.editors.iter().chain(&self.editors.models).find(|editor| editor.document.uri_string() == link.uri);
        let (_, path) = super::host_requests::uri_parts(&link.resource).ok()?;
        let lines: Vec<String> = match open {
            Some(editor) => editor.document.lines.clone(),
            None => std::fs::read_to_string(&path).ok()?.split('\n').map(|line| line.trim_end_matches('\r').to_string()).collect(),
        };
        let range = link.range;
        let start = range.start.0;
        if start < 1 || start > lines.len() {
            return None;
        }
        let first_non_whitespace = |line: usize| lines[line - 1].chars().position(|c| !c.is_whitespace()).map_or(0, |index| index + 1);
        let (end_line, end_column) = if range.end.0 - range.start.0 >= MAX_SOURCE_PREVIEW_LINES {
            // `getPreviewRangeBasedOnIndentation`.
            let start_indent = first_non_whitespace(start);
            let max_line = lines.len().min(start + MAX_SOURCE_PREVIEW_LINES);
            let mut end = start + 1;
            while end < max_line && first_non_whitespace(end) != start_indent {
                end += 1;
            }
            (end + 1, 1)
        } else {
            (range.end.0, range.end.1)
        };
        // `validateRange`.
        let (end_line, end_column) = if end_line > lines.len() { (lines.len(), lines[lines.len() - 1].encode_utf16().count() + 1) } else { (end_line, end_column) };
        let start_column = range.start.1.min(lines[start - 1].encode_utf16().count() + 1);
        let mut min_indent = first_non_whitespace(start);
        for line in start + 1..end_line {
            min_indent = min_indent.min(first_non_whitespace(line));
        }
        let mut value = String::new();
        for line in start..=end_line {
            let text = &lines[line - 1];
            let from = if line == start { char_column(text, start_column - 1) } else { 0 };
            let to = if line == end_line { char_column(text, end_column - 1) } else { text.chars().count() };
            let part: String = text.chars().skip(from).take(to.saturating_sub(from)).collect();
            if line > start {
                value.push('\n');
            }
            value.push_str(&part);
        }
        // `replace(new RegExp('^\\s{minIndent - 1}', 'gm'), '')`: lines with fewer whitespace chars keep theirs.
        let strip = min_indent.saturating_sub(1);
        let value: Vec<&str> = value.split('\n').map(|line| if line.chars().take(strip).filter(|c| c.is_whitespace()).count() == strip { &line[line.char_indices().nth(strip).map_or(line.len(), |(index, _)| index)..] } else { line }).collect();
        let value = value.join("\n");
        let value = value.trim();
        if value.is_empty() {
            return Some(None);
        }
        let first_line = lines.first().map_or("", String::as_str);
        let language = self.languages.guess_language(&path, first_line).unwrap_or_default();
        // MarkdownString.appendCodeblock.
        Some(Some(format!("\n```{language}\n{value}\n```\n")))
    }

    /// The link's `inlineClassName` on a line: `editorLink.activeForeground` text, underlined in the same color.
    pub(super) fn definition_link_styles(&self, editor: &OpenEditor, line: usize, tab_size: usize) -> Vec<(Range<usize>, HighlightStyle)> {
        let Some(decoration) = self.definition_link.decoration.as_ref().filter(|decoration| decoration.model_id == editor.model_id && decoration.line == line) else { return Vec::new() };
        let Some(color) = self.theme.color("editorLink.activeForeground").map(Hsla::from) else { return Vec::new() };
        let text = &editor.document.lines[line];
        let byte = |column: usize| expand_tabs(&text.chars().take(column).collect::<String>(), tab_size).len();
        let underline = UnderlineStyle { thickness: px(1.), color: Some(color), wavy: false };
        vec![(byte(decoration.columns.start)..byte(decoration.columns.end), HighlightStyle { color: Some(color), underline: Some(underline), ..Default::default() })]
    }

    /// goToDefinitionAtPosition.css `cursor: pointer` over the link.
    pub(super) fn definition_link_cursor(&self, editor: &OpenEditor, line: usize, char_width: f32) -> Option<Div> {
        let decoration = self.definition_link.decoration.as_ref().filter(|decoration| decoration.model_id == editor.model_id && decoration.line == line)?;
        let text = &editor.document.lines[line];
        let tab_size = editor.document.indentation.tab_size;
        let left = visible_column(text, decoration.columns.start, tab_size) as f32 * char_width;
        let right = visible_column(text, decoration.columns.end, tab_size) as f32 * char_width;
        Some(div().absolute().top_0().h_full().left(px(left)).w(px(right - left)).cursor_pointer())
    }

    /// markdownHoverParticipant.ts `computeSync`: the link decoration's hover message, at ordinal 1000.
    pub(super) fn definition_link_hover_parts(&self, editor: &OpenEditor, anchor: Anchor) -> Vec<Part> {
        let Some(decoration) = self.definition_link.decoration.as_ref() else { return Vec::new() };
        let Some(hover) = decoration.hover.as_ref() else { return Vec::new() };
        if decoration.model_id != editor.model_id || decoration.line != anchor.line || decoration.columns.start > anchor.start || anchor.end > decoration.columns.end {
            return Vec::new();
        }
        let (start, end) = (Position::new(anchor.line, decoration.columns.start), Position::new(anchor.line, decoration.columns.end));
        vec![Part { participant: Participant::Markdown, ordinal: 1000, start, end, content: PartContent::Markdown(vec![json!({ "value": hover })]) }]
    }
}

