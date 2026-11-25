//! The editor commands in the command registry under their upstream ids (coreCommands.ts,
//! linesOperations.ts, clipboard.ts, multicursor.ts, folding.ts), so keybindings and
//! `vscode.commands.executeCommand` run them.

use std::path::{Path, PathBuf};
use std::time::Duration;

use config::configuration::Overrides;
use gpui::{ClipboardEntry, ClipboardItem, Context, Window};
use serde_json::{Value, json};

use super::Workbench;
use crate::commands::Outcome;
use crate::editor::auto_indent::{AutoIndent, Strategy};
use crate::editor::word_operations::{self, AutoClosing, WordClassifier, WordDelete};
use crate::editor::{CaseTransform, ContentChanged, LineCommentKind, OpenEditor, Pasted, Selection, clipboard, commands, smart_select};

type EditorCommand = fn(&mut OpenEditor) -> Option<ContentChanged>;

/// Cursor and edit commands: `EditorCommand`s, which run on the focused editor or else the active one.
const EDITOR_COMMANDS: [(&str, EditorCommand); 34] = [
    ("cursorLeft", |editor| { editor.move_horizontally(false, false); None }),
    ("cursorLeftSelect", |editor| { editor.move_horizontally(false, true); None }),
    ("cursorRight", |editor| { editor.move_horizontally(true, false); None }),
    ("cursorRightSelect", |editor| { editor.move_horizontally(true, true); None }),
    ("cursorUp", |editor| { editor.move_vertically(false, false); None }),
    ("cursorUpSelect", |editor| { editor.move_vertically(false, true); None }),
    ("cursorDown", |editor| { editor.move_vertically(true, false); None }),
    ("cursorDownSelect", |editor| { editor.move_vertically(true, true); None }),
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
const INDENTING_EDITOR_COMMANDS: [(&str, IndentingCommand); 11] = [
    ("lineBreakInsert", OpenEditor::line_break_insert),
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
        self.register_word_commands();
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
            workbench.suggest_will_type(&text, window, cx);
            let overtype = workbench.overtype;
            let outcome = workbench.run_indenting(|editor, auto| editor.type_keyboard(&text, auto, overtype), cx);
            workbench.suggest_typed(cx);
            workbench.parameter_hints_typed(&text, cx);
            outcome
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
        self.commands.register("editor.action.unicodeHighlight.showExcludeOptions", |workbench, args, window, cx| {
            workbench.show_unicode_exclude_options(&args.first().cloned().unwrap_or_default(), window, cx);
            Outcome::done()
        });
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
        self.commands.register("editor.action.showContextMenu", |workbench, _, window, cx| {
            workbench.show_editor_context_menu_at_cursor(window, cx);
            Outcome::done()
        });
        self.commands.register("editor.action.toggleRenderControlCharacter", |workbench, _, _, cx| {
            let on = workbench.setting("editor.renderControlCharacters") == json!(true);
            workbench.update_setting("editor.renderControlCharacters", json!(!on));
            cx.notify();
            Outcome::done()
        });
        // breadcrumbsControl.ts `ToggleBreadcrumb`.
        self.commands.register("breadcrumbs.toggle", |workbench, _, _, cx| {
            let enabled = workbench.setting("breadcrumbs.enabled") != json!(false);
            workbench.update_setting("breadcrumbs.enabled", json!(!enabled));
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
        self.commands.register("editor.action.jumpToBracket", |workbench, _, _, cx| {
            let languages = &workbench.languages;
            let Some(editor) = workbench.editors.active_mut() else { return Outcome::done() };
            let selections_before = editor.selections.clone();
            editor.jump_to_bracket(languages);
            workbench.editor_command_ran(None, selections_before, cx)
        });
        self.commands.register("editor.action.selectToBracket", |workbench, args, _, cx| {
            let select_brackets = args.first().is_none_or(|args| args["selectBrackets"] != json!(false));
            let languages = &workbench.languages;
            let Some(editor) = workbench.editors.active_mut() else { return Outcome::done() };
            let selections_before = editor.selections.clone();
            editor.select_to_bracket(languages, select_brackets);
            workbench.editor_command_ran(None, selections_before, cx)
        });
        self.commands.register("editor.action.removeBrackets", |workbench, _, _, cx| {
            if workbench.edit_blocked(false, cx) {
                return Outcome::done();
            }
            let languages = &workbench.languages;
            let Some(editor) = workbench.editors.active_mut() else { return Outcome::done() };
            let selections_before = editor.selections.clone();
            let changes = editor.remove_brackets(languages);
            workbench.editor_changes_ran(changes, selections_before, cx)
        });
        // textMateTokenizationFeature.contribution.ts, which vscode-colorize-perf-tests times.
        self.commands.register("_workbench.colorizeTextMateTokens", |workbench, args, _, _| {
            let path = args.first().and_then(|uri| uri["path"].as_str()).map(PathBuf::from);
            let text = path.as_ref().and_then(|path| std::fs::read_to_string(path).ok());
            let (Some(path), Some(text)) = (path, text) else {
                let resource = args.first().and_then(|uri| uri["path"].as_str()).map_or("undefined".into(), |path| crate::editor::uri_string("file", Path::new(path)));
                return Outcome::error(format!("Cannot resolve text model for resource {resource}"));
            };
            let lines: Vec<&str> = text.split("\r\n").flat_map(|line| line.split(['\r', '\n'])).collect();
            let language = workbench.languages.language_of(&path, lines[0]);
            let Some(grammar) = workbench.languages.grammars.grammar(language) else {
                return Outcome::error(format!("Cannot resolve tokenizer for language {language}"));
            };
            let started = std::time::Instant::now();
            let mut state = None;
            for line in lines {
                if let Ok((_, next)) = grammar.tokenize_line(line, state.as_ref()) {
                    state = Some(next);
                }
            }
            Outcome::Ready(Ok(json!({ "tokenizeTime": started.elapsed().as_secs_f64() * 1000. })))
        });
        self.commands.register("editor.action.forceRetokenize", |workbench, _, _, cx| {
            let Some(editor) = workbench.editors.active_mut() else { return Outcome::done() };
            let started = std::time::Instant::now();
            editor.document.tokens.force_retokenize(&editor.document.lines);
            eprintln!("tokenization took {}", started.elapsed().as_secs_f64() * 1000.);
            cx.notify();
            Outcome::done()
        });
        // clipboard.ts `ExecCommandCopyWithSyntaxHighlightingAction`: an editor action, so the active editor without focus too.
        self.commands.register("editor.action.clipboardCopyWithSyntaxHighlightingAction", |workbench, _, window, cx| {
            let Some(editor) = workbench.editors.active() else { return Outcome::done() };
            if !workbench.editor_bool_setting(editor, "editor.emptySelectionClipboard") && editor.selection().is_empty() {
                return Outcome::done();
            }
            window.focus(&workbench.editor_focus, cx);
            workbench.copy_editor_text(true, cx);
            Outcome::done()
        });
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
            let Some(editor) = workbench.editors.active() else { return Outcome::done() };
            let empty_selection_clipboard = workbench.editor_bool_setting(editor, "editor.emptySelectionClipboard");
            let pasted = Pasted {
                text: clipboard.text(),
                paste_on_new_line: empty_selection_clipboard && metadata["isFromEmptySelection"] == json!(true),
                multicursor_text: multicursor_text.as_deref(),
                is_block: metadata["isBlock"] == json!(true),
            };
            let overrides = Overrides { resource: Some(&editor.document.path), override_identifier: Some(&editor.document.language_id) };
            let settings = workbench.settings.lock().unwrap();
            let spread = settings.get_value(Some("editor.multiCursorPaste"), &overrides) != Some(json!("full"));
            let overtype = workbench.overtype && settings.get_value(Some("editor.overtypeOnPaste"), &overrides) != Some(json!(false));
            let indent_on_paste = settings.get_value(Some("editor.autoIndentOnPaste"), &overrides) == Some(json!(true));
            let within_string = settings.get_value(Some("editor.autoIndentOnPasteWithinString"), &overrides) != Some(json!(false));
            drop(settings);
            workbench.run_indenting(
                |editor, auto| {
                    // codeEditorWidget.ts `_paste`: the pasted range runs from the selection start before to the one after.
                    let start = editor.selection().start();
                    let mut changes: Vec<_> = editor.paste(pasted, spread, overtype).into_iter().collect();
                    if indent_on_paste {
                        let end = editor.selection().start();
                        changes.extend(editor.auto_indent_on_paste(start, end, auto, within_string));
                    }
                    changes
                },
                cx,
            )
        });
    }

    /// clipboard.ts `registerExecCommandImpl` for the editor with text focus: nothing for an empty selection
    /// without `editor.emptySelectionClipboard`, else the copy, then the cut.
    fn copy_to_clipboard(&mut self, cut: bool, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let Some(editor) = self.editors.active().filter(|_| self.editor_focus.is_focused(window)) else {
            let command = if cut { "editor.action.clipboardCutAction" } else { "editor.action.clipboardCopyAction" };
            return self.input_text_command(command, window, cx).unwrap_or(Outcome::done());
        };
        let empty_selection_clipboard = self.editor_bool_setting(editor, "editor.emptySelectionClipboard");
        if !empty_selection_clipboard && editor.selection().is_empty() {
            return Outcome::done();
        }
        self.copy_editor_text(false, cx);
        match cut && !self.edit_blocked(true, cx) {
            true => self.run_on_active_editor(|editor| editor.cut(empty_selection_clipboard), cx),
            false => Outcome::done(),
        }
    }

    /// clipboardUtils.ts `generateDataToCopy`: the active editor's text with its metadata, and its HTML when
    /// `force` (Copy with Syntax Highlighting) or `editor.copyWithSyntaxHighlighting` and under 65536 long.
    fn copy_editor_text(&mut self, force: bool, cx: &mut Context<Self>) {
        let editor = self.editors.active().expect("the copy commands check for an active editor");
        let empty_selection_clipboard = self.editor_bool_setting(editor, "editor.emptySelectionClipboard");
        let copied = editor.copy_text(empty_selection_clipboard);
        // `sourceText.length`: the pieces when there are several, else the UTF-16 length.
        let length = copied.multicursor_text.as_ref().map_or_else(|| copied.text.encode_utf16().count(), Vec::len);
        let rich_text = (force || (self.editor_bool_setting(editor, "editor.copyWithSyntaxHighlighting") && length < 65536)).then(|| self.rich_text_to_copy(empty_selection_clipboard)).flatten();
        let (html, mode) = rich_text.unzip();
        let metadata = json!({
            "version": 1,
            "isFromEmptySelection": copied.from_empty_selection,
            "isBlock": copied.is_block,
            "multicursorText": copied.multicursor_text,
            "mode": mode,
        });
        cx.write_to_clipboard(ClipboardItem::new_string_with_json_metadata(copied.text, metadata));
        if let Some(html) = html {
            crate::clipboard::write_html(&html);
        }
    }

    /// `ViewModel.getRichTextToCopy` for the active editor with the theme's token colors and the editor font.
    fn rich_text_to_copy(&self, empty_selection_clipboard: bool) -> Option<(String, String)> {
        let editor = self.editors.active()?;
        let overrides = Overrides { resource: Some(&editor.document.path), override_identifier: Some(&editor.document.language_id) };
        let settings = self.settings.lock().unwrap();
        let setting = |key| settings.get_value(Some(key), &overrides);
        let font = clipboard::font_info(setting("editor.fontFamily"), setting("editor.fontWeight"), setting("editor.fontSize"), setting("editor.lineHeight"), 1. + f64::from(self.editor_zoom.level()) * 0.1);
        drop(settings);
        let mut color_map = vec!["#000000".to_string()];
        color_map.extend(self.theme.tokens.color_map().iter().skip(1).map(|color| color[..7].to_lowercase()));
        let document = &editor.document;
        let line_tokens = |line| document.tokens.html_tokens(&self.theme, &document.lines, line);
        clipboard::rich_text_to_copy(&document.lines, &document.language_id, &editor.selections, empty_selection_clipboard, &font, &color_map, document.indentation.tab_size, line_tokens)
    }

    /// A boolean editor setting for the editor's resource and language that defaults to true.
    fn editor_bool_setting(&self, editor: &OpenEditor, key: &str) -> bool {
        let overrides = Overrides { resource: Some(&editor.document.path), override_identifier: Some(&editor.document.language_id) };
        self.settings.lock().unwrap().get_value(Some(key), &overrides) != Some(json!(false))
    }

    /// wordOperations.ts and wordPartOperations.ts.
    fn register_word_commands(&mut self) {
        for (id, word_move) in word_operations::MOVE_COMMANDS {
            self.commands.register(id, move |workbench, _, _, cx| {
                let Some(classifier) = workbench.word_classifier(word_move.default_separators) else { return Outcome::done() };
                workbench.run_on_active_editor(|editor| { editor.move_word(&classifier, word_move.navigation, word_move.right, word_move.select); None }, cx)
            });
        }
        // cursorWordPartStartLeft and its Select are registerCommandAlias ids of the left moves.
        let part_moves = [("cursorWordPartLeft", false, false), ("cursorWordPartLeftSelect", false, true), ("cursorWordPartStartLeft", false, false), ("cursorWordPartStartLeftSelect", false, true), ("cursorWordPartRight", true, false), ("cursorWordPartRightSelect", true, true)];
        for (id, right, select) in part_moves {
            self.commands.register(id, move |workbench, _, _, cx| {
                let Some(classifier) = workbench.word_classifier(false) else { return Outcome::done() };
                workbench.run_on_active_editor(|editor| { editor.move_word_part(&classifier, right, select); None }, cx)
            });
        }
        for (id, word_delete) in word_operations::DELETE_COMMANDS {
            self.commands.register(id, move |workbench, _, _, cx| {
                if workbench.edit_blocked(false, cx) {
                    return Outcome::done();
                }
                workbench.run_word_delete(word_delete, id, cx)
            });
        }
        self.commands.register("deleteInsideWord", |workbench, args, _, cx| {
            if workbench.edit_blocked(false, cx) {
                return Outcome::done();
            }
            let only_word = args.first().is_some_and(|args| args["onlyWord"] == json!(true));
            let Some(classifier) = workbench.word_classifier(false) else { return Outcome::done() };
            workbench.run_on_active_editor(|editor| editor.delete_inside_word(&classifier, only_word), cx)
        });
    }

    /// `getMapForWordSeparators` with the active editor's `editor.wordSeparators` (or the default one) and
    /// `editor.wordSegmenterLocales`.
    fn word_classifier(&self, default_separators: bool) -> Option<WordClassifier> {
        let editor = self.editors.active()?;
        let overrides = Overrides { resource: Some(&editor.document.path), override_identifier: Some(&editor.document.language_id) };
        let settings = self.settings.lock().unwrap();
        let configured = settings.get_value(Some("editor.wordSeparators"), &overrides);
        let separators = match configured.as_ref().and_then(Value::as_str) {
            Some(separators) if !default_separators => separators,
            _ => word_operations::DEFAULT_WORD_SEPARATORS,
        };
        let locales = word_operations::segmenter_locales(settings.get_value(Some("editor.wordSegmenterLocales"), &overrides).as_ref());
        Some(WordClassifier::new(separators, &locales))
    }

    /// `DeleteWordCommand.runEditorCommand` with the auto-closing settings and pairs of the editor's language.
    /// The port does not auto-close typed brackets, so no cursor has auto-closed characters.
    fn run_word_delete(&mut self, word_delete: WordDelete, id: &'static str, cx: &mut Context<Self>) -> Outcome {
        let Some(classifier) = self.word_classifier(false) else { return Outcome::done() };
        let editor = self.editors.active().expect("word_classifier found it");
        let overrides = Overrides { resource: Some(&editor.document.path), override_identifier: Some(&editor.document.language_id) };
        let settings = self.settings.lock().unwrap();
        let setting = |key| settings.get_value(Some(key), &overrides).and_then(|value| value.as_str().map(str::to_string)).unwrap_or_default();
        let (delete, brackets, quotes) = (setting("editor.autoClosingDelete"), setting("editor.autoClosingBrackets"), setting("editor.autoClosingQuotes"));
        drop(settings);
        let pairs = self.languages.rules(&editor.document.language_id).map(|rules| rules.auto_closing_pairs.clone()).unwrap_or_default();
        let auto_closing = AutoClosing { delete: &delete, brackets: &brackets, quotes: &quotes, pairs: &pairs, auto_closed: &[] };
        self.run_on_active_editor(|editor| editor.delete_word(&classifier, &auto_closing, word_delete, id), cx)
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
        self.editor_changes_ran(changes, selections_before, cx)
    }

    /// An editor command that made several model changes: each reaches the host in turn.
    fn editor_changes_ran(&mut self, changes: Vec<ContentChanged>, selections_before: Vec<Selection>, cx: &mut Context<Self>) -> Outcome {
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

#[cfg(test)]
mod tests;
