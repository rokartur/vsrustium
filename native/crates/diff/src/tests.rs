//! Upstream cases of `test/node/diffing/fixtures.test.ts`, `defaultLinesDiffComputer.test.ts` and `verifyLinesDiff.test.ts`.

use super::*;
use crate::computer::{apply_range_mappings, verify_lines_diff, Validity};
use crate::sequences::LinesSliceCharSequence;
use serde_json::{json, Value};
use std::path::PathBuf;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("VSCODE_SRC")).join("src/vs/editor/test/node/diffing/fixtures")
}

fn split(text: &str) -> Vec<String> {
    text.split('\n').map(str::to_string).collect()
}

fn read_fixture(folder: &str) -> (String, String, String, String) {
    let dir = fixtures_dir().join(folder);
    let mut names: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    names.sort();
    let first = names.iter().find(|n| n.starts_with("1.")).unwrap().clone();
    let second = names.iter().find(|n| n.starts_with("2.")).unwrap().clone();
    let read = |name: &str| std::fs::read_to_string(dir.join(name)).unwrap().replace("\r\n", "\n").replace('\r', "\n");
    (read(&first), read(&second), first, second)
}

fn format_range(range: &Range, lines: &[String]) -> String {
    let to_last_char = range.end.column == lines[range.end.line - 1].chars().count() + 1;
    format!("[{},{} -> {},{}{}]", range.start.line, range.start.column, range.end.line, range.end.column, if to_last_char { " EOL" } else { "" })
}

fn diffs_json(changes: &[LineRangeMapping], original: &[String], modified: &[String]) -> Value {
    Value::Array(
        changes
            .iter()
            .map(|c| {
                json!({
                    "originalRange": c.original.to_string(),
                    "modifiedRange": c.modified.to_string(),
                    "innerChanges": c.inner_changes.as_ref().map(|inner| inner.iter().map(|i| json!({
                        "originalRange": format_range(&i.original, original),
                        "modifiedRange": format_range(&i.modified, modified),
                    })).collect::<Vec<_>>()),
                })
            })
            .collect(),
    )
}

#[test]
fn advanced_fixtures_match_the_expected_diffs() {
    let mut folders: Vec<String> = std::fs::read_dir(fixtures_dir()).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    folders.sort();
    assert!(folders.len() > 50, "fixtures not found at {}", fixtures_dir().display());
    let mut failures = Vec::new();
    for folder in &folders {
        let (first, second, first_name, second_name) = read_fixture(folder);
        let (original, modified) = (split(&first), split(&second));
        let ignore_trim_whitespace = folder.contains("trimws");
        let diff = compute_diff(&original, &modified, DiffOptions { ignore_trim_whitespace, max_computation_time_ms: 0, compute_moves: true, extend_to_subwords: false });
        if !ignore_trim_whitespace {
            let inner: Vec<RangeMapping> = diff.changes.iter().flat_map(|c| c.inner_changes.clone().unwrap()).collect();
            if apply_range_mappings(&inner, &original, &modified).as_deref() != Some(second.as_str()) {
                failures.push(format!("{folder}: inner changes do not turn the original into the modified text"));
            }
        }
        let mut actual = json!({
            "original": { "content": first, "fileName": format!("./{first_name}") },
            "modified": { "content": second, "fileName": format!("./{second_name}") },
            "diffs": diffs_json(&diff.changes, &original, &modified),
        });
        if !diff.moves.is_empty() {
            actual["moves"] = Value::Array(
                diff.moves
                    .iter()
                    .map(|m| json!({
                        "originalRange": m.line_range_mapping.original.to_string(),
                        "modifiedRange": m.line_range_mapping.modified.to_string(),
                        "changes": diffs_json(&m.changes, &original, &modified),
                    }))
                    .collect(),
            );
        }
        let expected: Value = serde_json::from_str(&std::fs::read_to_string(fixtures_dir().join(folder).join("advanced.expected.diff.json")).unwrap()).unwrap();
        if actual != expected {
            failures.push(format!("{folder}:\n  expected {}\n  actual   {}", expected["diffs"], actual["diffs"]));
            if expected.get("moves") != actual.get("moves") {
                failures.push(format!("{folder} moves:\n  expected {:?}\n  actual   {:?}", expected.get("moves"), actual.get("moves")));
            }
        }
    }
    assert!(failures.is_empty(), "{} of {} fixtures differ:\n{}", failures.len(), folders.len(), failures.join("\n"));
}

#[test]
fn diffs_of_every_fixture_verify() {
    let mut folders: Vec<String> = std::fs::read_dir(fixtures_dir()).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    folders.sort();
    let mut invalid = Vec::new();
    for folder in &folders {
        let (first, second, ..) = read_fixture(folder);
        let (original, modified) = (split(&first), split(&second));
        for ignore_trim_whitespace in [false, true] {
            let diff = compute_diff(&original, &modified, DiffOptions { ignore_trim_whitespace, ..Default::default() });
            let validity = verify_lines_diff(&diff, &original, &modified, ignore_trim_whitespace);
            if validity != Validity::Valid {
                invalid.push(format!("{folder} advanced ignoreTrimWhitespace={ignore_trim_whitespace}: {validity:?}"));
            }
        }
    }
    assert_eq!(invalid, ["invalid-diff-bug advanced ignoreTrimWhitespace=true: Invalid", "invalid-diff-trimws advanced ignoreTrimWhitespace=true: Invalid"]);
}

#[test]
fn move_detection_does_not_panic_when_a_move_starts_in_the_first_line_of_only_one_side() {
    let original = split("\t\t\t\t// ignore\n\t\t\t}\n\t}\n\t\tthis._recommendations = [];\n\t\tthis._ignoredRecommendations = [];\n\t\t\t\t\t}");
    let modified = split("\t\tthis._recommendations = [];\n\t\tthis._ignoredRecommendations = [];\n\n\t\t\t\t// ignore\n\t\t\t}\n\t\t}");
    let diff = compute_diff(&original, &modified, DiffOptions { compute_moves: true, ..Default::default() });
    assert_eq!(diff.moves, []);
}

#[test]
fn diffs_reconstruct_the_modified_text() {
    for (before, after) in [
        ("same", "same"),
        ("", "new\nfile\n"),
        ("old\nfile\n", ""),
        ("a\nb\nc", "\ta\nb\n\tc"),
        ("first\n\nlast", "FIRST\n\nLAST"),
        ("a a\nunchanged\nz z", "b b\nunchanged\ny y"),
        ("\u{1f600}a\u{1f600}a", "\u{1f600}b\u{1f600}b"),
        ("one\ntwo\nthree\nfour", "three\nfour\none\ntwo"),
    ] {
        let (original, modified) = (split(before), split(after));
        let diff = compute_diff(&original, &modified, DiffOptions { compute_moves: true, extend_to_subwords: true, ..Default::default() });
        let inner: Vec<RangeMapping> = diff.changes.iter().flat_map(|c| c.inner_changes.clone().unwrap()).collect();
        assert_eq!(apply_range_mappings(&inner, &original, &modified).as_deref(), Some(after), "{before:?} -> {after:?}");
    }
}

#[test]
fn line_range_mapping_simple_and_empty_lines() {
    let mapping = line_range_mapping(
        &RangeMapping { original: Range::new(2, 1, 3, 1), modified: Range::new(2, 1, 2, 1) },
        &split("const abc = \"helloworld\".split(\"\");\n\n"),
        &split("const asciiLower = \"helloworld\".split(\"\");\n"),
    );
    assert_eq!((mapping.original.to_string(), mapping.modified.to_string()), ("[2,3)".into(), "[2,2)".into()));
    let mapping = line_range_mapping(&RangeMapping { original: Range::new(2, 1, 2, 1), modified: Range::new(2, 1, 4, 1) }, &split("\n"), &split("\n\n\n"));
    assert_eq!((mapping.original.to_string(), mapping.modified.to_string()), ("[2,2)".into(), "[2,4)".into()));
}

fn slice_fixture() -> LinesSliceCharSequence {
    LinesSliceCharSequence::new(&split("line1: foo\nline2: fizzbuzz\nline3: barr\nline4: hello world\nline5: bazz"), Range::new(2, 1, 5, 1), true)
}

#[test]
fn lines_slice_translate_offset() {
    use crate::algorithms::Sequence;
    let sequence = slice_fixture();
    let result: Vec<String> = (0..sequence.len()).map(|offset| sequence.translate_offset(offset, false)).map(|p| format!("({},{})", p.line, p.column)).collect();
    let mut expected: Vec<String> = (1..=16).map(|c| format!("(2,{c})")).collect();
    expected.extend((1..=12).map(|c| format!("(3,{c})")));
    expected.extend((1..=19).map(|c| format!("(4,{c})")));
    assert_eq!(result, expected);
}

#[test]
fn lines_slice_extend_to_full_lines() {
    let sequence = slice_fixture();
    assert_eq!(sequence.text(sequence.extend_to_full_lines(OffsetRange::new(20, 25))), "line3: barr\n");
    assert_eq!(sequence.text(sequence.extend_to_full_lines(OffsetRange::new(20, 45))), "line3: barr\nline4: hello world\n");
}

#[test]
fn verify_lines_diff_cases() {
    let original = split("a\n  b\nc");
    let modified = split("a\nb\nc");
    let change = |inner: Option<Vec<RangeMapping>>| LinesDiff {
        changes: vec![LineRangeMapping { original: LineRange::new(2, 3), modified: LineRange::new(2, 3), inner_changes: inner }],
        moves: Vec::new(),
        hit_timeout: false,
    };
    let correct = change(Some(vec![RangeMapping { original: Range::new(2, 1, 2, 3), modified: Range::new(2, 1, 2, 1) }]));
    assert_eq!(verify_lines_diff(&correct, &original, &modified, false), Validity::Valid);
    let wrong = change(Some(vec![RangeMapping { original: Range::new(2, 1, 2, 2), modified: Range::new(2, 1, 2, 1) }]));
    assert_eq!(verify_lines_diff(&wrong, &original, &modified, false), Validity::Invalid);
    assert_eq!(verify_lines_diff(&wrong, &original, &modified, true), Validity::Valid);
    assert_eq!(verify_lines_diff(&change(None), &original, &modified, false), Validity::Unverified);
}
