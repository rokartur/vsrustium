//! columnSelectionPaste.test.ts, textToHtmlTokenizer.test.ts and the clipboard cases of cursor.test.ts.
//! Selections are written 1-based as upstream's `new Selection(...)`.

use super::*;
use crate::editor::OpenEditor;
use crate::editor::edit::tests::{editor, plain};
use crate::editor::{ColumnSelectData, Pasted};

/// `new Selection(selectionStartLineNumber, selectionStartColumn, positionLineNumber, positionColumn)`.
fn sel(anchor_line: usize, anchor_column: usize, line: usize, column: usize) -> Selection {
    Selection { anchor: Position::new(anchor_line - 1, anchor_column - 1), head: Position::new(line - 1, column - 1) }
}

fn lines_of(editor: &OpenEditor) -> Vec<String> {
    editor.document().lines.clone()
}

const BLOCK: &str = "A\nBC\nD";

/// columnSelectionPaste.test.ts `paste`: a block copy, its rows as `multicursorText`.
fn block_paste(editor: &mut OpenEditor, text: &str, spread: bool) {
    let rows: Vec<String> = text.split('\n').map(String::from).collect();
    editor.paste(Pasted { text, paste_on_new_line: false, multicursor_text: Some(&rows), is_block: true }, spread, false);
}

fn assert_block_paste(text: &[&str], selections: Vec<Selection>, pasted: &str, tab_size: usize, expected: &[&str]) {
    let mut editor = editor(&text.join("\n"), "\n");
    editor.document_mut().indentation.tab_size = tab_size;
    editor.set_selections(selections);
    block_paste(&mut editor, pasted, true);
    assert_eq!(lines_of(&editor), expected, "pasting {pasted:?} into {text:?}");
}

#[test]
fn block_paste_at_one_cursor_or_selection() {
    let left_right = ["left--right", "left--right", "left--right"];
    let cases: [(&[&str], Selection, &str, &[&str]); 18] = [
        (&left_right, sel(1, 5, 1, 5), BLOCK, &["leftA --right", "leftBC--right", "leftD --right"]),
        (&left_right, sel(1, 5, 1, 7), BLOCK, &["leftA right", "leftBC--right", "leftD --right"]),
        (&left_right, sel(1, 7, 1, 5), BLOCK, &["leftA right", "leftBC--right", "leftD --right"]),
        (&left_right, sel(1, 5, 3, 7), BLOCK, &["leftA right", "    BC", "    D "]),
        (&left_right, sel(3, 7, 1, 5), BLOCK, &["leftA right", "    BC", "    D "]),
        (&["first", "second", "third", "unselected"], sel(1, 1, 4, 1), BLOCK, &["A unselected", "BC", "D "]),
        (&["left--right", "second", "third", "unselected"], sel(4, 1, 1, 5), BLOCK, &["leftA unselected", "    BC", "    D "]),
        (&["left--right", "left--right", "last"], sel(1, 5, 2, 7), BLOCK, &["leftA right", "lastBC", "    D "]),
        (&["left--right", "middle", "middle", "left--right"], sel(1, 5, 4, 7), BLOCK, &["leftA right", "    BC", "    D "]),
        (&["left--right", "x", ""], sel(1, 5, 1, 5), BLOCK, &["leftA --right", "x   BC", "    D "]),
        (&["left--right"], sel(1, 5, 1, 5), BLOCK, &["leftA --right", "    BC", "    D "]),
        (&["left"], sel(1, 5, 1, 5), BLOCK, &["leftA ", "    BC", "    D "]),
        (&[""], sel(1, 1, 1, 1), BLOCK, &["A ", "BC", "D "]),
        (&["left--right", "left--right", "left--right", "left--right"], sel(1, 5, 1, 5), "A\n\nD\n", &["leftA--right", "left --right", "leftD--right", "left --right"]),
        (&["left", "x", "", "y"], sel(1, 5, 1, 5), "A\n\nD\n", &["leftA", "x    ", "    D", "y    "]),
        (&["left"], sel(1, 5, 1, 5), "A\n\nD\n", &["leftA", "     ", "    D", "     "]),
        (&["\tleft", "    left", "  \tleft"], sel(1, 2, 1, 2), BLOCK, &["\tA left", "    BCleft", "  \tD left"]),
        (&["left--right", "left--right", "left--right", "left--right"], sel(1, 5, 1, 5), "\tX\n\u{4E2D}\n\u{1F600}\ne\u{301}", &["left\tX--right", "left\u{4E2D}   --right", "left\u{1F600}   --right", "lefte\u{301}    --right"]),
    ];
    for (text, selection, pasted, expected) in cases {
        assert_block_paste(text, vec![selection], pasted, 4, expected);
    }
    assert_block_paste(&["\t--left", "\t", ""], vec![sel(1, 4, 1, 4)], BLOCK, 8, &["\t--A left", "\t  BC", "          D "]);
    assert_block_paste(&["left--right", "e\u{301}", "\u{4E2D}", "\u{1F600}"], vec![sel(1, 5, 1, 5)], "A\nB\nC\nD", 4, &["leftA--right", "e\u{301}   B", "\u{4E2D}  C", "\u{1F600}  D"]);
    // Upstream's Selection(1, 3, 1, 3) counts the emoji's two UTF-16 units; the port's columns are chars.
    assert_block_paste(&["\u{1F600}left", "e\u{301}xleft", "\u{4E2D}left"], vec![sel(1, 2, 1, 2)], BLOCK, 4, &["\u{1F600}A left", "e\u{301}xBCleft", "\u{4E2D}D left"]);
}

#[test]
fn block_paste_at_several_cursors_distributes_or_repeats() {
    let cases: [(&[&str], Vec<Selection>, &[&str]); 4] = [
        (&["xx", "yy", "zz"], vec![sel(3, 2, 3, 2), sel(1, 2, 1, 2), sel(2, 2, 2, 2)], &["xAx", "yBCy", "zDz"]),
        (&["[one]", "[two]", "[three]"], vec![sel(1, 2, 1, 5), sel(2, 2, 2, 5), sel(3, 2, 3, 7)], &["[A]", "[BC]", "[D]"]),
        (&["xx", "yy", "zz"], vec![sel(1, 2, 1, 2), sel(3, 2, 3, 2)], &["xA", "BC", "Dx", "yy", "zA", "BC", "Dz"]),
        (&["xx", "yy", "zz", "[one]", "[two]"], vec![sel(1, 2, 1, 2), sel(4, 2, 5, 5)], &["xA", "BC", "Dx", "yy", "zz", "[A", "BC", "D]"]),
    ];
    for (text, selections, expected) in cases {
        assert_block_paste(text, selections, BLOCK, 4, expected);
    }
    assert_block_paste(&["xx", "yy"], vec![sel(1, 2, 1, 2), sel(2, 2, 2, 2)], "A\nB\n", 4, &["xAx", "yBy"]);
}

#[test]
fn multi_cursor_paste_full_and_missing_metadata() {
    let run = |text: &str, selections: Vec<Selection>, pasted: Pasted, spread: bool| {
        let mut editor = editor(text, "\n");
        editor.set_selections(selections);
        editor.paste(pasted, spread, false);
        editor.document().lines.join("\n")
    };
    let cursors = || vec![sel(1, 2, 1, 2), sel(2, 2, 2, 2), sel(3, 2, 3, 2)];
    // Block mode respects multiCursorPaste full without row metadata.
    assert_eq!(run("xx\nyy\nzz", cursors(), Pasted { is_block: true, ..plain(BLOCK) }, false), "xA\nBC\nDx\nyA\nBC\nDy\nzA\nBC\nDz");
    // Full mode keeps single-cursor multiline pasting and the multi-cursor metadata distribution.
    assert_eq!(run("left--right\nlast", vec![sel(1, 5, 1, 5)], Pasted { is_block: true, ..plain(BLOCK) }, false), "leftA\nBC\nD--right\nlast");
    let rows = ["A".to_string(), "BC".to_string(), "D".to_string()];
    assert_eq!(run("xx\nyy\nzz", cursors(), Pasted { multicursor_text: Some(&rows), is_block: true, ..plain(BLOCK) }, false), "xAx\nyBCy\nzDz");
    // Missing block metadata still spreads at a single cursor; no metadata spreads lines or pastes whole.
    assert_eq!(run("left--right\nlast", vec![sel(1, 5, 1, 5)], Pasted { is_block: true, ..plain(BLOCK) }, true), "leftA --right\nlastBC\n    D ");
    assert_eq!(run("xx\nyy\nzz", cursors(), plain(BLOCK), true), "xAx\nyBCy\nzDz");
    assert_eq!(run("xx\nyy", vec![sel(1, 2, 1, 2), sel(2, 2, 2, 2)], plain("A\nB"), false), "xA\nBx\nyA\nBy");
    // Older metadata without a block flag pastes normally.
    assert_eq!(run("left--right", vec![sel(1, 5, 1, 5)], Pasted { multicursor_text: Some(&rows), ..plain(BLOCK) }, true), "leftA\nBC\nD--right");
    // cursor.test.ts issue #256039: rows from empty selections go one per cursor with multiCursorPaste full.
    let lines = ["line1\n".to_string(), "line2\n".to_string()];
    assert_eq!(run("line1\nline2\nline3", vec![sel(1, 1, 1, 1), sel(2, 1, 2, 1)], Pasted { paste_on_new_line: true, multicursor_text: Some(&lines), ..plain("line1\nline2\n") }, false), "line1\nline1\nline2\nline2\nline3");
}

#[test]
fn block_paste_undo_redo_and_cursor() {
    let mut cursors = editor("xx\nyy\nzz", "\n");
    let selections = vec![sel(1, 2, 1, 2), sel(2, 2, 2, 2), sel(3, 2, 3, 2)];
    cursors.set_selections(selections.clone());
    block_paste(&mut cursors, BLOCK, true);
    let after = vec![sel(1, 3, 1, 3), sel(2, 4, 2, 4), sel(3, 3, 3, 3)];
    assert_eq!((lines_of(&cursors).iter().map(String::as_str).collect::<Vec<_>>(), cursors.selections.clone()), (vec!["xAx", "yBCy", "zDz"], after.clone()));
    cursors.undo();
    assert_eq!((lines_of(&cursors).iter().map(String::as_str).collect::<Vec<_>>(), cursors.selections.clone()), (vec!["xx", "yy", "zz"], selections));
    cursors.redo();
    assert_eq!((lines_of(&cursors).iter().map(String::as_str).collect::<Vec<_>>(), cursors.selections.clone()), (vec!["xAx", "yBCy", "zDz"], after));

    let mut aligned = editor("\tfirst\nmiddle\nlast--suffix\nx\n", "\n");
    let selection = sel(3, 7, 1, 2);
    aligned.set_selections(vec![selection]);
    block_paste(&mut aligned, "\nBC\n", true);
    let expected = (vec!["\t  suffix", "x   BC", "      "], vec![sel(3, 7, 3, 7)]);
    assert_eq!((lines_of(&aligned).iter().map(String::as_str).collect::<Vec<_>>(), aligned.selections.clone()), expected);
    aligned.undo();
    assert_eq!((lines_of(&aligned).iter().map(String::as_str).collect::<Vec<_>>(), aligned.selections.clone()), (vec!["\tfirst", "middle", "last--suffix", "x", ""], vec![selection]));
    aligned.redo();
    assert_eq!((lines_of(&aligned).iter().map(String::as_str).collect::<Vec<_>>(), aligned.selections.clone()), expected);

    for (text, expected) in [("left\nleft\nleft", vec!["leftA ", "leftBC", "left  "]), ("left\nx\n", vec!["leftA ", "x   BC", "      "]), ("left", vec!["leftA ", "    BC", "      "])] {
        let mut trailing = editor(text, "\n");
        trailing.set_selections(vec![sel(1, 5, 1, 5)]);
        block_paste(&mut trailing, "A\nBC\n", true);
        assert_eq!((lines_of(&trailing).iter().map(String::as_str).collect::<Vec<_>>(), trailing.selections.clone()), (expected, vec![sel(3, 7, 3, 7)]));
    }
}

#[test]
fn block_paste_keeps_crlf_and_overtypes() {
    let mut crlf = editor("left\nx", "\r\n");
    crlf.set_selections(vec![sel(1, 5, 1, 5)]);
    block_paste(&mut crlf, "A\r\nBC\r\nD", true);
    assert_eq!(lines_of(&crlf), ["leftA ", "x   BC", "    D "]);
    let mut overtype = editor("left--right\nleft--right\nx\n", "\n");
    overtype.set_selections(vec![sel(1, 5, 1, 5)]);
    let text = "A\nBC\nD\nE";
    let rows: Vec<String> = text.split('\n').map(String::from).collect();
    overtype.paste(Pasted { text, paste_on_new_line: false, multicursor_text: Some(&rows), is_block: true }, true, true);
    assert_eq!(lines_of(&overtype), ["leftA right", "leftBCright", "x   D ", "    E "]);
}

#[test]
fn block_paste_follows_the_multi_cursor_paste_setting() {
    let mut results = Vec::new();
    for spread in [true, false, true] {
        let mut editor = editor("left\nx\n", "\n");
        editor.set_selections(vec![sel(1, 5, 1, 5)]);
        block_paste(&mut editor, BLOCK, spread);
        results.push(editor.document().lines.clone());
    }
    assert_eq!(results, [vec!["leftA ", "x   BC", "    D "], vec!["leftA", "BC", "D", "x", ""], vec!["leftA ", "x   BC", "    D "]]);
}

/// columnSelectionPaste.test.ts 'clipboard metadata': a column selection from `from` to `to` (1-based).
fn select_column(editor: &mut OpenEditor, from: (usize, usize), to: (usize, usize)) {
    editor.set_selections(vec![sel(from.0, from.1, from.0, from.1)]);
    editor.column_select(ColumnSelectData { from_line: from.0 - 1, from_visible: from.1 - 1, to_line: to.0 - 1, to_visible: to.1 - 1 });
}

#[test]
fn copies_carry_the_column_block_identity() {
    for cut in [false, true] {
        let mut editor = editor("<AA>\n<BB>\n<CC>", "\n");
        select_column(&mut editor, (1, 2), (3, 4));
        let copied = editor.copy_text(true);
        if cut {
            editor.cut(true);
        }
        let rows = ["AA", "BB", "CC"].map(String::from).to_vec();
        assert_eq!((copied.text.as_str(), copied.is_block, copied.multicursor_text, copied.from_empty_selection), ("AA\nBB\nCC", true, Some(rows), false));
        assert_eq!(lines_of(&editor), if cut { ["<>", "<>", "<>"] } else { ["<AA>", "<BB>", "<CC>"] });
    }
    let mut reversed = editor("<AA>\n<BB>\n<CC>", "\n");
    select_column(&mut reversed, (3, 4), (1, 2));
    let copied = reversed.copy_text(true);
    assert_eq!((copied.text.as_str(), copied.is_block), ("AA\nBB\nCC", true));
    let mut one_row = editor("<AA>", "\n");
    select_column(&mut one_row, (1, 2), (1, 4));
    let copied = one_row.copy_text(true);
    assert_eq!((copied.text.as_str(), copied.multicursor_text, copied.is_block), ("AA", None, true));
    let mut aligned = editor("<AA>\n<BB>\n<CC>", "\n");
    aligned.set_selections(vec![sel(1, 2, 1, 4), sel(2, 2, 2, 4), sel(3, 2, 3, 4)]);
    assert!(!aligned.copy_text(true).is_block);
    let mut zero_width = editor("AA\nBB\nCC", "\n");
    select_column(&mut zero_width, (1, 2), (3, 2));
    let copied = zero_width.copy_text(true);
    assert_eq!((copied.multicursor_text, copied.is_block), (Some(["AA\n", "BB\n", "CC\n"].map(String::from).to_vec()), true));
    let mut changed = editor("<AA>\n<BB>\n<CC>", "\n");
    select_column(&mut changed, (1, 2), (3, 4));
    changed.set_selections(vec![sel(1, 2, 3, 4)]);
    assert!(!changed.copy_text(true).is_block);
}

#[test]
fn a_copied_block_pastes_into_another_editor() {
    let mut source = editor("<AA>\n<BB>\n<CC>", "\n");
    select_column(&mut source, (1, 2), (3, 4));
    let copied = source.copy_text(true);
    let mut target = editor("left--right\nleft--right\nleft--right", "\n");
    target.set_selections(vec![sel(1, 5, 1, 5)]);
    target.paste(Pasted { text: &copied.text, paste_on_new_line: false, multicursor_text: copied.multicursor_text.as_deref(), is_block: copied.is_block }, true, false);
    assert_eq!(lines_of(&target), ["leftAA--right", "leftBB--right", "leftCC--right"]);
}

#[test]
fn empty_selection_clipboard_off_copies_and_cuts_only_selections() {
    let mut editor = editor("one\ntwo", "\n");
    editor.set_selections(vec![sel(1, 2, 1, 2), sel(2, 1, 2, 3)]);
    let copied = editor.copy_text(false);
    assert_eq!((copied.text.as_str(), copied.from_empty_selection), ("tw", false));
    editor.cut(false);
    assert_eq!(lines_of(&editor), ["one", "o"]);
}

#[test]
fn cut_takes_the_last_line_with_its_line_break() {
    // cursor.test.ts bug #16740 and issue #128602.
    let mut with_text = editor("asdasd\nqwerty", "\n");
    with_text.set_selections(vec![sel(2, 1, 2, 1)]);
    with_text.cut(true);
    assert_eq!(lines_of(&with_text), ["asdasd"]);
    let mut empty_last = editor("asdasd\n", "\n");
    empty_last.set_selections(vec![sel(2, 1, 2, 1)]);
    empty_last.cut(true);
    assert_eq!(lines_of(&empty_last), ["asdasd"]);
    empty_last.cut(true);
    assert_eq!(lines_of(&empty_last), [""]);
    let mut three = editor("a1\na2\na3", "\n");
    three.set_selections(vec![sel(1, 1, 1, 1), sel(2, 1, 2, 1), sel(3, 1, 3, 1)]);
    three.cut(true);
    assert_eq!(lines_of(&three), [""]);
}

const COLOR_MAP: [&str; 6] = ["", "#000000", "#ffffff", "#ff0000", "#00ff00", "#0000ff"];

fn color_map() -> Vec<String> {
    COLOR_MAP.map(String::from).to_vec()
}

fn token(end: usize, foreground: usize, font_style: i32) -> HtmlToken {
    HtmlToken { end, foreground, font_style }
}

const BOLD_ITALIC: i32 = textmate::BOLD | textmate::ITALIC;

#[test]
fn tokenize_line_to_html_spans() {
    let tokens = [token(4, 3, BOLD_ITALIC), token(5, 1, 0), token(10, 4, 0), token(11, 1, 0), token(17, 5, textmate::UNDERLINE)];
    let html = |start, end| tokenize_line_to_html("Ciao hello world!", &tokens, &color_map(), start, end, 4, true);
    let ciao = "<span style=\"color: #ff0000;font-style: italic;font-weight: bold;\">Ciao</span>";
    let space = "<span style=\"color: #000000;\"> </span>";
    let hello = "<span style=\"color: #00ff00;\">hello</span>";
    assert_eq!(html(0, 17), format!("<div>{ciao}{space}{hello}{space}<span style=\"color: #0000ff;text-decoration: underline;\">world!</span></div>"));
    assert_eq!(html(0, 12), format!("<div>{ciao}{space}{hello}{space}<span style=\"color: #0000ff;text-decoration: underline;\">w</span></div>"));
    assert_eq!(html(0, 11), format!("<div>{ciao}{space}{hello}{space}</div>"));
    assert_eq!(html(1, 11), format!("<div><span style=\"color: #ff0000;font-style: italic;font-weight: bold;\">iao</span>{space}{hello}{space}</div>"));
    assert_eq!(html(4, 11), format!("<div><span style=\"color: #000000;\">&#160;</span>{hello}{space}</div>"));
    assert_eq!(html(5, 11), format!("<div>{hello}{space}</div>"));
    assert_eq!(html(5, 10), format!("<div>{hello}</div>"));
    assert_eq!(html(6, 9), "<div><span style=\"color: #00ff00;\">ell</span></div>");
}

#[test]
fn tokenize_line_to_html_handles_spaces_35954() {
    let tokens = [token(2, 1, 0), token(6, 3, BOLD_ITALIC), token(9, 1, 0), token(14, 4, 0), token(15, 1, 0), token(21, 5, textmate::UNDERLINE)];
    let html = |end| tokenize_line_to_html("  Ciao   hello world!", &tokens, &color_map(), 0, end, 4, true);
    let lead = "<span style=\"color: #000000;\">&#160; </span>";
    let bold = |text| format!("<span style=\"color: #ff0000;font-style: italic;font-weight: bold;\">{text}</span>");
    let gap = "<span style=\"color: #000000;\"> &#160; </span><span style=\"color: #00ff00;\">hello</span><span style=\"color: #000000;\"> </span>";
    assert_eq!(html(21), format!("<div>{lead}{}{gap}<span style=\"color: #0000ff;text-decoration: underline;\">world!</span></div>", bold("Ciao")));
    assert_eq!(html(17), format!("<div>{lead}{}{gap}<span style=\"color: #0000ff;text-decoration: underline;\">wo</span></div>", bold("Ciao")));
    assert_eq!(html(3), format!("<div>{lead}{}</div>", bold("C")));
}

#[test]
fn tokenize_line_to_html_with_tabs_and_a_start_offset_263387() {
    let tokens = [token(1, 1, 0), token(2, 3, 0), token(3, 1, 0), token(4, 4, 0)];
    let html = |start| tokenize_line_to_html("\ta\tb", &tokens, &color_map(), start, 4, 4, true);
    assert_eq!(
        html(0),
        "<div><span style=\"color: #000000;\">&#160; &#160; </span><span style=\"color: #ff0000;\">a</span><span style=\"color: #000000;\"> &#160; </span><span style=\"color: #00ff00;\">b</span></div>"
    );
    assert_eq!(html(2), "<div><span style=\"color: #000000;\">&#160; &#160;</span><span style=\"color: #00ff00;\">b</span></div>");
}

#[test]
fn rich_text_wraps_the_lines_in_the_editor_font_and_colors() {
    let lines = ["let a;".to_string(), String::new(), "b".to_string()];
    let font = font_info(None, None, None, None, 1.);
    let tokens = |line: usize| vec![token(lines[line].chars().count(), 3, 0)];
    let (html, mode) = rich_text_to_copy(&lines, "typescript", &[sel(1, 5, 3, 2)], true, &font, &color_map(), 4, tokens).expect("a single selection");
    assert_eq!(mode, "typescript");
    assert_eq!(
        html,
        "<div style=\"color: #000000;background-color: #ffffff;font-family: Menlo, Monaco, 'Courier New', monospace;font-weight: normal;font-size: 12px;line-height: 18px;white-space: pre;\"><div><span style=\"color: #ff0000;\">a;</span></div><br><div><span style=\"color: #ff0000;\">b</span></div></div>"
    );
    assert!(rich_text_to_copy(&lines, "plaintext", &[sel(1, 1, 1, 2)], true, &font, &color_map(), 4, tokens).is_none());
    assert!(rich_text_to_copy(&lines, "typescript", &[sel(1, 1, 1, 1)], false, &font, &color_map(), 4, tokens).is_none());
    assert!(rich_text_to_copy(&lines, "typescript", &[sel(1, 1, 1, 2), sel(3, 1, 3, 2)], true, &font, &color_map(), 4, tokens).is_none());
    let zoomed = font_info(Some(serde_json::json!("Fira Code")), Some(serde_json::json!("bold")), Some(serde_json::json!(14)), Some(serde_json::json!(1.2)), 1.1);
    assert_eq!((html_font_family(&zoomed.family), zoomed.weight.as_str(), zoomed.size, zoomed.line_height), ("'Fira Code', Menlo, Monaco, 'Courier New', monospace".to_string(), "bold", 14. * 1.1, 17. * 1.1));
    assert_eq!(html_font_family("a;b"), DEFAULT_FONT_FAMILY);
}
