//! The editor commands in the command registry under their upstream ids (coreCommands.ts,
//! linesOperations.ts, clipboard.ts, multicursor.ts, folding.ts), so keybindings and
//! `vscode.commands.executeCommand` run them.

use std::time::Duration;

use config::configuration::Overrides;
use gpui::{ClipboardEntry, ClipboardItem, Context, Window};
use serde_json::{Value, json};

use super::Workbench;
use crate::commands::Outcome;
use crate::editor::auto_indent::{AutoIndent, Strategy};
use crate::editor::{CaseTransform, ContentChanged, Copied, LineCommentKind, OpenEditor, Selection, commands, smart_select};

type EditorCommand = fn(&mut OpenEditor) -> Option<ContentChanged>;

/// Cursor and edit commands: `EditorCommand`s, which run on the focused editor or else the active one.
const EDITOR_COMMANDS: [(&str, EditorCommand); 48] = [
    ("cursorLeft", |editor| { editor.move_horizontally(false, false); None }),
    ("cursorLeftSelect", |editor| { editor.move_horizontally(false, true); None }),
    ("cursorRight", |editor| { editor.move_horizontally(true, false); None }),
    ("cursorRightSelect", |editor| { editor.move_horizontally(true, true); None }),
    ("cursorUp", |editor| { editor.move_vertically(false, false); None }),
    ("cursorUpSelect", |editor| { editor.move_vertically(false, true); None }),
    ("cursorDown", |editor| { editor.move_vertically(true, false); None }),
    ("cursorDownSelect", |editor| { editor.move_vertically(true, true); None }),
    ("cursorWordLeft", |editor| { editor.move_word(false, false); None }),
    ("cursorWordLeftSelect", |editor| { editor.move_word(false, true); None }),
    ("cursorWordEndRight", |editor| { editor.move_word(true, false); None }),
    ("cursorWordEndRightSelect", |editor| { editor.move_word(true, true); None }),
    ("cursorWordPartLeft", |editor| { editor.move_word_part(false, false); None }),
    ("cursorWordPartLeftSelect", |editor| { editor.move_word_part(false, true); None }),
    // wordPartOperations.ts registerCommandAlias: the previous ids of the two above.
    ("cursorWordPartStartLeft", |editor| { editor.move_word_part(false, false); None }),
    ("cursorWordPartStartLeftSelect", |editor| { editor.move_word_part(false, true); None }),
    ("cursorWordPartRight", |editor| { editor.move_word_part(true, false); None }),
    ("cursorWordPartRightSelect", |editor| { editor.move_word_part(true, true); None }),
    ("cursorHome", |editor| { editor.move_to_line_edge(false, false); None }),
    ("cursorHomeSelect", |editor| { editor.move_to_line_edge(false, true); None }),
    ("cursorEnd", |editor| { editor.move_to_line_edge(true, false); None }),
    ("cursorEndSelect", |editor| { editor.move_to_line_edge(true, true); None }),
    ("cursorTop", |editor| { editor.move_to_document_edge(false, false); None }),
    ("cursorTopSelect", |editor| { editor.move_to_document_edge(false, true); None }),
    ("cursorBottom", |editor| { editor.move_to_document_edge(true, false); None }),
    ("cursorBottomSelect", |editor| { editor.move_to_document_edge(true, true); None }),
    ("expandLineSelection", |editor| { editor.expand_line_selection(); None }),
    ("deleteLeft", OpenEditor::delete_left),
    ("deleteRight", OpenEditor::delete_right),
    ("deleteWordLeft", OpenEditor::delete_word_left),
    ("deleteWordRight", OpenEditor::delete_word_right),
    ("deleteWordPartLeft", OpenEditor::delete_word_part_left),
    ("deleteWordPartRight", OpenEditor::delete_word_part_right),
    ("deleteAllLeft", OpenEditor::delete_all_left),
    ("deleteAllRight", OpenEditor::delete_all_right),
    ("editor.action.copyLinesUpAction", |editor| editor.copy_lines(false)),
    ("editor.action.copyLinesDownAction", |editor| editor.copy_lines(true)),
    ("editor.action.duplicateSelection", OpenEditor::duplicate_selection),
    ("editor.action.sortLinesAscending", |editor| editor.sort_lines(false)),
    ("editor.action.sortLinesDescending", |editor| editor.sort_lines(true)),
    ("editor.action.removeDuplicateLines", OpenEditor::delete_duplicate_lines),
    ("editor.action.reverseLines", OpenEditor::reverse_lines),
    ("editor.action.deleteLines", OpenEditor::delete_lines),
    ("editor.action.joinLines", OpenEditor::join_lines),
    ("editor.action.transpose", OpenEditor::transpose),
    ("editor.action.moveCarretLeftAction", |editor| editor.move_selected_text(true)),
    ("editor.action.moveCarretRightAction", |editor| editor.move_selected_text(false)),
    ("editor.action.transposeLetters", OpenEditor::transpose_letters),
];

type IndentingCommand = fn(&mut OpenEditor, AutoIndent) -> Option<ContentChanged>;

/// Edit commands that indent by the language's onEnter and indentation rules (`CursorConfiguration.autoIndent`).
const INDENTING_EDITOR_COMMANDS: [(&str, IndentingCommand); 10] = [
    ("tab", OpenEditor::tab),
    ("outdent", OpenEditor::outdent),
    ("editor.action.indentLines", OpenEditor::indent_lines),
    ("editor.action.outdentLines", OpenEditor::outdent),
    ("editor.action.moveLinesUpAction", |editor, auto| editor.move_lines(false, auto)),
    ("editor.action.moveLinesDownAction", |editor, auto| editor.move_lines(true, auto)),
    ("editor.action.insertLineBefore", OpenEditor::insert_line_before),
    ("editor.action.insertLineAfter", OpenEditor::insert_line_after),
    ("editor.action.reindentlines", OpenEditor::reindent_lines),
    ("editor.action.reindentselectedlines", OpenEditor::reindent_selected_lines),
];

/// Commands of the editor with text focus only (`MultiCommand`s and `EditorHandlerCommand`s): elsewhere they
/// belong to the focused input element, the quick input's.
const FOCUSED_EDITOR_COMMANDS: [(&str, EditorCommand); 3] = [
    ("undo", OpenEditor::undo),
    ("redo", OpenEditor::redo),
    ("editor.action.selectAll", |editor| { editor.select_all(); None }),
];

impl Workbench {
    pub(super) fn register_editor_commands(&mut self) {
        for (id, command) in EDITOR_COMMANDS {
            let edit = !id.starts_with("cursor") && id != "expandLineSelection";
            let cursor_edit = matches!(id, "deleteLeft" | "deleteRight");
            self.commands.register(id, move |workbench, _, _, cx| {
                if edit && workbench.edit_blocked(cursor_edit, cx) {
                    return Outcome::done();
                }
                workbench.run_on_active_editor(command, cx)
            });
        }
        for (id, command) in INDENTING_EDITOR_COMMANDS {
            self.commands.register(id, move |workbench, _, _, cx| {
                if workbench.edit_blocked(false, cx) {
                    return Outcome::done();
                }
                workbench.run_indenting(|editor, auto| command(editor, auto).into_iter().collect(), cx)
            });
        }
        for (id, command) in FOCUSED_EDITOR_COMMANDS {
            let edit = id != "editor.action.selectAll";
            self.commands.register(id, move |workbench, _, window, cx| match workbench.editor_focus.is_focused(window) {
                true if edit && workbench.edit_blocked(false, cx) => Outcome::done(),
                true => workbench.run_on_active_editor(command, cx),
                // The implementations' priorities: DOM input 1000, explorer 110, custom editor 105.
                false => workbench
                    .input_text_command(id, window, cx)
                    .or_else(|| workbench.explorer_undo(id, window, cx))
                    .or_else(|| workbench.custom_text_editor_undo(id, command, cx))
                    .unwrap_or(Outcome::done()),
            });
        }
        self.commands.register("type", |workbench, args, window, cx| {
            let Some(text) = args.first().and_then(|args| args["text"].as_str()).map(str::to_owned) else { return Outcome::done() };
            if !workbench.editor_focus.is_focused(window) || workbench.edit_blocked(true, cx) {
                return Outcome::done();
            }
            let overtype = workbench.overtype;
            workbench.run_indenting(|editor, auto| editor.type_keyboard(&text, auto, overtype), cx)
        });
        for &id in commands::IDS {
            // multicursor.ts FocusNextCursor/FocusPreviousCursor do nothing in a read-only editor.
            let writes = matches!(id, "editor.action.focusNextCursor" | "editor.action.focusPreviousCursor");
            self.commands.register(id, move |workbench, _, _, cx| {
                if workbench.editors.active().is_some() && !(writes && workbench.active_editor_readonly().is_some()) {
                    workbench.run_editor_command(id, "keyboard", cx);
                }
                Outcome::done()
            });
        }
        for (id, transform) in CaseTransform::ALL {
            self.commands.register(id, move |workbench, _, _, cx| {
                if workbench.edit_blocked(false, cx) {
                    return Outcome::done();
                }
                workbench.run_on_active_editor(|editor: &mut OpenEditor| editor.transform_case(transform), cx)
            });
        }
        let comment_actions = [
            ("editor.action.commentLine", Some(LineCommentKind::Toggle)),
            ("editor.action.addCommentLine", Some(LineCommentKind::ForceAdd)),
            ("editor.action.removeCommentLine", Some(LineCommentKind::ForceRemove)),
            ("editor.action.blockComment", None),
        ];
        for (id, kind) in comment_actions {
            self.commands.register(id, move |workbench, _, _, cx| match workbench.edit_blocked(false, cx) {
                true => Outcome::done(),
                false => workbench.run_comment_action(kind, cx),
            });
        }
        let unicode_highlight_actions = [
            ("editor.action.unicodeHighlight.disableHighlightingOfAmbiguousCharacters", "editor.unicodeHighlight.ambiguousCharacters"),
            ("editor.action.unicodeHighlight.disableHighlightingOfInvisibleCharacters", "editor.unicodeHighlight.invisibleCharacters"),
            ("editor.action.unicodeHighlight.disableHighlightingOfNonBasicAsciiCharacters", "editor.unicodeHighlight.nonBasicASCII"),
        ];
        for (id, key) in unicode_highlight_actions {
            self.commands.register(id, move |workbench, _, _, cx| {
                workbench.update_setting(key, false.into());
                cx.notify();
                Outcome::done()
            });
        }
        // `editor.action.smartSelect.grow` is the expand command's old id (registerCommandAlias).
        for (id, forward) in [("editor.action.smartSelect.expand", true), ("editor.action.smartSelect.grow", true), ("editor.action.smartSelect.shrink", false)] {
            self.commands.register(id, move |workbench, _, _, cx| workbench.run_smart_select(forward, cx));
        }
        for (id, up) in [("editor.action.inPlaceReplace.up", false), ("editor.action.inPlaceReplace.down", true)] {
            self.commands.register(id, move |workbench, _, _, cx| match workbench.edit_blocked(false, cx) {
                true => Outcome::done(),
                false => workbench.run_in_place_replace(up, cx),
            });
        }
        // shortcut: GPUI has no screen reader announcements, so "Anchor set at {0}:{1}" is not read out; add one with them.
        self.commands.register("editor.action.setSelectionAnchor", |workbench, _, _, cx| {
            if let Some(editor) = workbench.editors.active_mut() {
                editor.set_selection_anchor();
                cx.notify();
            }
            Outcome::done()
        });
        let anchor_moves: [(&str, fn(&mut OpenEditor)); 2] = [
            ("editor.action.goToSelectionAnchor", OpenEditor::go_to_selection_anchor),
            ("editor.action.selectFromAnchorToCursor", OpenEditor::select_from_anchor_to_cursor),
        ];
        for (id, command) in anchor_moves {
            // `setPosition` and `setSelection` of the API: the host hears of it, the view does not reveal it.
            self.commands.register(id, move |workbench, _, _, cx| {
                let Some(editor) = workbench.editors.active_mut().filter(|editor| editor.selection_anchor.is_some()) else { return Outcome::done() };
                command(editor);
                workbench.host.send_selection(editor, "api");
                cx.notify();
                Outcome::done()
            });
        }
        self.commands.register("editor.action.cancelSelectionAnchor", |workbench, _, _, cx| {
            if let Some(editor) = workbench.editors.active_mut() {
                editor.selection_anchor = None;
                cx.notify();
            }
            Outcome::done()
        });
        self.commands.register("editor.action.insertFinalNewLine", |workbench, _, _, cx| workbench.run_on_active_editor(OpenEditor::insert_final_new_line, cx));
        let zooms: [(&str, fn(f32) -> f32); 3] = [
            ("editor.action.fontZoomIn", |level| level + 1.),
            ("editor.action.fontZoomOut", |level| level - 1.),
            ("editor.action.fontZoomReset", |_| 0.),
        ];
        for (id, level) in zooms {
            self.commands.register(id, move |workbench, _, _, cx| {
                if workbench.editor_zoom.set_level(level(workbench.editor_zoom.level())) {
                    cx.notify();
                }
                Outcome::done()
            });
        }
        self.commands.register("editor.action.toggleRenderWhitespace", |workbench, _, _, cx| {
            let next = if workbench.setting("editor.renderWhitespace") == "none" { "all" } else { "none" };
            workbench.update_setting("editor.renderWhitespace", json!(next));
            cx.notify();
            Outcome::done()
        });
        self.commands.register("editor.action.toggleMinimap", |workbench, _, _, cx| {
            let enabled = workbench.setting("editor.minimap.enabled") != json!(false);
            workbench.update_setting("editor.minimap.enabled", json!(!enabled));
            cx.notify();
            Outcome::done()
        });
        self.commands.register("editor.action.toggleColumnSelection", |workbench, _, _, cx| {
            let on = workbench.setting("editor.columnSelection") != json!(true);
            workbench.update_setting("editor.columnSelection", json!(on));
            if let Some(editor) = workbench.editors.active_mut() {
                let before = editor.selections.clone();
                let reveal = editor.column_selection_mode_changed(on);
                workbench.cursors_changed(before, reveal, "keyboard", cx);
            }
            cx.notify();
            Outcome::done()
        });
        self.commands.register("workbench.action.toggleMultiCursorModifier", |workbench, _, _, cx| {
            let next = if workbench.multi_cursor_modifier_is_cmd() { "alt" } else { "ctrlCmd" };
            workbench.update_setting("editor.multiCursorModifier", json!(next));
            cx.notify();
            Outcome::done()
        });
        self.commands.register("cursorUndo", |workbench, _, _, cx| workbench.run_cursor_undo(false, cx));
        self.commands.register("cursorRedo", |workbench, _, _, cx| workbench.run_cursor_undo(true, cx));
        self.commands.register("editor.action.clipboardCopyAction", |workbench, _, window, cx| workbench.copy_to_clipboard(false, window, cx));
        self.commands.register("editor.action.clipboardCutAction", |workbench, _, window, cx| workbench.copy_to_clipboard(true, window, cx));
        self.commands.register("editor.action.clipboardPasteAction", |workbench, _, window, cx| {
            if !workbench.editor_focus.is_focused(window) {
                return workbench.input_text_command("editor.action.clipboardPasteAction", window, cx).unwrap_or(Outcome::done());
            }
            if workbench.edit_blocked(true, cx) {
                return Outcome::done();
            }
            let Some(ClipboardEntry::String(clipboard)) = cx.read_from_clipboard().and_then(|item| item.entries().first().cloned()) else { return Outcome::done() };
            let metadata = clipboard.metadata_json::<Value>().unwrap_or_default();
            let multicursor_text: Option<Vec<String>> = serde_json::from_value(metadata["multicursorText"].clone()).ok();
            let from_empty_selection = metadata["isFromEmptySelection"] == json!(true);
            let Some(editor) = workbench.editors.active() else { return Outcome::done() };
            let overrides = Overrides { resource: Some(&editor.document.path), override_identifier: Some(&editor.document.language_id) };
            let overtype = workbench.overtype && workbench.settings.lock().unwrap().get_value(Some("editor.overtypeOnPaste"), &overrides) != Some(json!(false));
            workbench.run_on_active_editor(|editor: &mut OpenEditor| editor.paste(clipboard.text(), from_empty_selection, multicursor_text.as_deref(), overtype), cx)
        });
    }

    fn copy_to_clipboard(&mut self, cut: bool, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let Some(editor) = self.editors.active().filter(|_| self.editor_focus.is_focused(window)) else {
            let command = if cut { "editor.action.clipboardCutAction" } else { "editor.action.clipboardCopyAction" };
            return self.input_text_command(command, window, cx).unwrap_or(Outcome::done());
        };
        cx.write_to_clipboard(clipboard_item(editor.copy_text()));
        match cut && !self.edit_blocked(true, cx) {
            true => self.run_on_active_editor(OpenEditor::cut, cx),
            false => Outcome::done(),
        }
    }

    /// Runs an editor command, then tells the host and scrolls the cursor into view as a key press does.
    pub(super) fn run_on_active_editor(&mut self, command: impl FnOnce(&mut OpenEditor) -> Option<ContentChanged>, cx: &mut Context<Self>) -> Outcome {
        let Some(editor) = self.editors.active_mut() else { return Outcome::done() };
        let selections_before = editor.selections.clone();
        let change = command(editor);
        self.editor_command_ran(change, selections_before, cx)
    }

    /// Runs an editor command that indents by the language's rules, with `editor.autoIndent` of the
    /// editor's language; each model change it makes reaches the host in turn.
    pub(super) fn run_indenting(&mut self, command: impl FnOnce(&mut OpenEditor, AutoIndent) -> Vec<ContentChanged>, cx: &mut Context<Self>) -> Outcome {
        let Some(editor) = self.editors.active_mut() else { return Outcome::done() };
        let overrides = Overrides { resource: Some(&editor.document.path), override_identifier: Some(&editor.document.language_id) };
        let strategy = Strategy::from_setting(self.settings.lock().unwrap().get_value(Some("editor.autoIndent"), &overrides).as_ref());
        let selections_before = editor.selections.clone();
        let changes = command(editor, AutoIndent { languages: &self.languages, strategy });
        if changes.is_empty() {
            return self.editor_command_ran(None, selections_before, cx);
        }
        for change in changes {
            self.editor_command_ran(Some(change), selections_before.clone(), cx);
        }
        Outcome::done()
    }

    fn editor_command_ran(&mut self, change: Option<ContentChanged>, selections_before: Vec<Selection>, cx: &mut Context<Self>) -> Outcome {
        let moved = self.editors.active().is_some_and(|editor| editor.selections != selections_before);
        self.marked_text = None;
        self.editor_edited(change, moved, "keyboard", cx);
        Outcome::Ready(Ok(Value::Null))
    }

    /// cursorUndo.ts `CursorUndo` and `CursorRedo`: the selections, then the scroll position they had.
    fn run_cursor_undo(&mut self, redo: bool, cx: &mut Context<Self>) -> Outcome {
        let Some(editor) = self.editors.active_mut() else { return Outcome::done() };
        let selections_before = editor.selections.clone();
        let Some(scroll) = (if redo { editor.cursor_redo() } else { editor.cursor_undo() }) else { return Outcome::done() };
        let outcome = self.editor_command_ran(None, selections_before, cx);
        let viewport = self.editors.viewport;
        if self.editors.active_mut().expect("still open").set_scroll(scroll.top, scroll.left, viewport) {
            self.editor_scrolled(cx);
        }
        outcome
    }

    /// smartSelect.ts `GrowSelectionAction` and `ShrinkSelectionAction` with the `editor.smartSelect` options.
    fn run_smart_select(&mut self, forward: bool, cx: &mut Context<Self>) -> Outcome {
        let Some(editor) = self.editors.active_mut() else { return Outcome::done() };
        let overrides = Overrides { resource: Some(&editor.document.path), override_identifier: Some(&editor.document.language_id) };
        let settings = self.settings.lock().unwrap();
        let enabled = |key| settings.get_value(Some(key), &overrides) != Some(json!(false));
        let options = smart_select::Options { select_leading_and_trailing_whitespace: enabled("editor.smartSelect.selectLeadingAndTrailingWhitespace"), select_subwords: enabled("editor.smartSelect.selectSubwords") };
        drop(settings);
        let selections_before = editor.selections.clone();
        editor.smart_select(forward, &self.languages, options);
        self.editor_command_ran(None, selections_before, cx)
    }

    /// inPlaceReplace.ts `InPlaceReplaceController.run`: the replaced value stays outlined for 350 ms.
    fn run_in_place_replace(&mut self, up: bool, cx: &mut Context<Self>) -> Outcome {
        let Some(editor) = self.editors.active_mut() else { return Outcome::done() };
        let selections_before = editor.selections.clone();
        let Some((change, (line, range))) = editor.in_place_replace(up, &self.languages) else { return Outcome::done() };
        let version = editor.version_id;
        editor.value_set_replacement = Some((version, line, range));
        let (scheme, path) = (editor.document.scheme.clone(), editor.document.path.clone());
        cx.spawn(async move |workbench, cx| {
            cx.background_executor().timer(Duration::from_millis(350)).await;
            let _ = workbench.update(cx, |workbench, cx| {
                let Some(editor) = workbench.editors.model_mut(&scheme, &path) else { return };
                if editor.value_set_replacement.as_ref().is_some_and(|(set_at, ..)| *set_at == version) {
                    editor.value_set_replacement = None;
                    cx.notify();
                }
            });
        })
        .detach();
        self.editor_command_ran(Some(change), selections_before, cx)
    }

    /// comment.ts `CommentLineAction` and `BlockCommentAction` with the `editor.comments` options, on the
    /// language at each cursor.
    fn run_comment_action(&mut self, kind: Option<LineCommentKind>, cx: &mut Context<Self>) -> Outcome {
        let Some(editor) = self.editors.active_mut() else { return Outcome::done() };
        let overrides = Overrides { resource: Some(&editor.document.path), override_identifier: Some(&editor.document.language_id) };
        let settings = self.settings.lock().unwrap();
        let insert_space = settings.get_value(Some("editor.comments.insertSpace"), &overrides) != Some(json!(false));
        let ignore_empty_lines = settings.get_value(Some("editor.comments.ignoreEmptyLines"), &overrides) != Some(json!(false));
        drop(settings);
        let selections_before = editor.selections.clone();
        let change = match kind {
            Some(kind) => editor.comment_lines(kind, insert_space, ignore_empty_lines, &self.languages),
            None => editor.block_comment(insert_space, &self.languages),
        };
        self.editor_command_ran(change, selections_before, cx)
    }
}

/// The copied text with the `ClipboardStoredMetadata` paste reads back.
fn clipboard_item(copied: Copied) -> ClipboardItem {
    let metadata = json!({ "isFromEmptySelection": copied.from_empty_selection, "multicursorText": copied.multicursor_text });
    ClipboardItem::new_string_with_json_metadata(copied.text, metadata)
}
