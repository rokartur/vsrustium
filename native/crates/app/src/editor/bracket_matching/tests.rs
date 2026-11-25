//! bracketMatching.test.ts, positions 0-based.

use serde_json::json;

use crate::editor::OpenEditor;
use crate::editor::cursor::{Position, Selection};
use crate::editor::edit::tests::editor;
use crate::languages::Languages;

fn with_brackets(text: &str) -> (OpenEditor, Languages) {
    let mut languages = Languages::new(&[]);
    languages.configure("bracketMode", &json!({ "brackets": [["{", "}"], ["[", "]"], ["(", ")"]] }));
    let mut editor = editor(text, "\n");
    editor.document.language_id = "bracketMode".into();
    (editor, languages)
}

fn at(column: usize) -> Selection {
    Selection::caret(Position::new(0, column - 1))
}

fn jump(editor: &mut OpenEditor, languages: &Languages) -> usize {
    editor.jump_to_bracket(languages);
    editor.selection().head.column + 1
}

#[test]
fn issue_183_jump_to_matching_bracket_position() {
    let (mut editor, languages) = with_brackets("var x = (3 + (5-7)) + ((5+3)+5);");
    editor.set_selections(vec![at(20)]);
    assert_eq!([jump(&mut editor, &languages), jump(&mut editor, &languages), jump(&mut editor, &languages)], [9, 19, 9]);
    editor.set_selections(vec![at(23)]);
    assert_eq!([jump(&mut editor, &languages), jump(&mut editor, &languages), jump(&mut editor, &languages)], [31, 23, 31]);
}

#[test]
fn jump_to_next_bracket() {
    let (mut editor, languages) = with_brackets("var x = (3 + (5-7)); y();");
    editor.set_selections(vec![at(16)]);
    assert_eq!([jump(&mut editor, &languages), jump(&mut editor, &languages), jump(&mut editor, &languages)], [18, 14, 18]);
    editor.set_selections(vec![at(21)]);
    assert_eq!([jump(&mut editor, &languages), jump(&mut editor, &languages), jump(&mut editor, &languages)], [23, 24, 23]);
    editor.set_selections(vec![at(26)]);
    assert_eq!(jump(&mut editor, &languages), 26);
}

#[test]
fn select_to_next_bracket() {
    let (mut editor, languages) = with_brackets("var x = (3 + (5-7)); y();");
    let mut select = |column: usize| {
        editor.set_selections(vec![at(column)]);
        editor.select_to_bracket(&languages, true);
        let selection = editor.selection();
        (selection.anchor.column + 1, selection.head.column + 1)
    };
    assert_eq!(select(9), (9, 20));
    assert_eq!(select(20), (20, 9));
    assert_eq!(select(16), (14, 19));
    assert_eq!(select(21), (23, 25));
    assert_eq!(select(26), (26, 26));
}

const OBJECT: &str = "const x = {\n    something: [0, 1, 2],\n    another: true,\n    somethingmore: [0, 2, 4]\n};";

#[test]
fn issue_1772_jump_to_enclosing_brackets() {
    let (mut editor, languages) = with_brackets(OBJECT);
    editor.set_selections(vec![Selection::caret(Position::new(2, 4))]);
    editor.jump_to_bracket(&languages);
    assert_eq!(editor.selection(), Selection::caret(Position::new(4, 0)));
}

#[test]
fn issue_43371_argument_to_not_select_brackets() {
    let (mut editor, languages) = with_brackets(OBJECT);
    editor.set_selections(vec![Selection::caret(Position::new(2, 4))]);
    editor.select_to_bracket(&languages, false);
    assert_eq!(editor.selection(), Selection { anchor: Position::new(0, 11), head: Position::new(4, 0) });
}

#[test]
fn issue_45369_select_to_bracket_with_multicursor() {
    let (mut editor, languages) = with_brackets("{  }   {   }   { }");
    let mut select = |columns: [usize; 3]| {
        editor.set_selections(columns.map(at).into());
        editor.select_to_bracket(&languages, true);
        editor.selections.iter().map(|selection| (selection.anchor.column + 1, selection.head.column + 1)).collect::<Vec<_>>()
    };
    assert_eq!(select([3, 10, 17]), [(1, 5), (8, 13), (16, 19)]);
    assert_eq!(select([1, 6, 14]), [(1, 5), (8, 13), (16, 19)]);
    assert_eq!(select([5, 13, 19]), [(5, 1), (13, 8), (19, 16)]);
}

#[test]
fn removes_brackets() {
    let (mut editor, languages) = with_brackets("var x = (3 + (5-7)); y();");
    editor.set_selections(vec![at(9)]);
    editor.remove_brackets(&languages);
    assert_eq!(editor.document.lines, ["var x = 3 + (5-7); y();"]);
    let (mut editor, languages) = with_brackets("var x = (3 + (5-7)); y();");
    editor.set_selections(vec![at(16)]);
    assert_eq!(editor.remove_brackets(&languages).len(), 1);
    assert_eq!(editor.document.lines, ["var x = (3 + 5-7); y();"]);
    editor.remove_brackets(&languages);
    assert_eq!(editor.document.lines, ["var x = 3 + 5-7; y();"]);
    assert!(editor.remove_brackets(&languages).is_empty());
    assert_eq!(editor.document.lines, ["var x = 3 + 5-7; y();"]);
}
