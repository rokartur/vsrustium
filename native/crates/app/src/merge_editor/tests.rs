//! model.test.ts of mergeEditor.

use diff::LineRange;

use super::*;

fn lines(text: &str) -> Vec<String> {
    text.replace("\r\n", "\n").split('\n').map(str::to_string).collect()
}

struct Harness {
    model: MergeModel,
    result: Vec<String>,
    version: u32,
}

impl Harness {
    fn new(base: &str, input1: &str, input2: &str, result: &str) -> Self {
        let mut model = MergeModel::new((1, 1, 1), lines(base), lines(input1), lines(input2));
        let result = lines(result);
        model.sync_result(&result, 1, true);
        Harness { model, result, version: 1 }
    }

    /// The test's `toggleConflict`.
    fn toggle(&mut self, index: usize, input: Input) {
        let state = self.model.accepted[index].toggle(input);
        let edit = self.model.set_state(index, state);
        self.apply(&edit);
    }

    fn reset(&mut self) {
        let edit = self.model.reset(self.result.len());
        self.apply(&edit);
    }

    fn apply(&mut self, edit: &LineEdit) {
        let (start, end, text) = text_edit(edit, &self.result, "\n");
        let joined = self.result.join("\n");
        let (start, end) = (offset(&self.result, start.line + 1, start.column + 1), offset(&self.result, end.line + 1, end.column + 1));
        let chars: Vec<char> = joined.chars().collect();
        let mut changed: String = chars[..start].iter().collect();
        changed.push_str(&text);
        changed.extend(&chars[end..]);
        self.result = lines(&changed);
        self.version += 1;
        self.model.sync_result(&self.result, self.version, false);
    }

    fn result(&self) -> String {
        self.result.join("\n")
    }

    /// The test's `getProjections`: each range's lines wrapped in `⟦…⟧` and labeled with its index, the result's
    /// with its state too.
    fn projections(&self) -> [Vec<String>; 4] {
        let ranges = &self.model.ranges;
        let index_labels: Vec<String> = (0..ranges.len()).map(small_number).collect();
        let state_labels: Vec<String> = (0..ranges.len()).map(|index| format!("{{{}}}{}", label(self.model.accepted[index]), small_number(index))).collect();
        [
            label_ranges(&self.model.base, ranges.iter().map(|range| range.base).zip(&index_labels)),
            label_ranges(&self.model.input1, ranges.iter().map(|range| range.input1).zip(&index_labels)),
            label_ranges(&self.model.input2, ranges.iter().map(|range| range.input2).zip(&index_labels)),
            label_ranges(&self.result, ranges.iter().map(|range| self.model.result_line_range(range.base)).zip(&state_labels)),
        ]
    }
}

/// `ModifiedBaseRangeState.toString` (`ModifiedBaseRangeStateBoth` prints `2✓` upstream too).
fn label(state: State) -> &'static str {
    match state {
        State::Base => "base",
        State::Input1 => "1\u{2713}",
        State::Input2 | State::Both { .. } => "2\u{2713}",
        State::Unrecognized => "unrecognized",
    }
}

fn small_number(index: usize) -> String {
    index.to_string().chars().map(|digit| char::from_u32(0x2080 + digit.to_digit(10).unwrap()).unwrap()).collect()
}

/// The char offset of a 1-based position in `lines` joined with "\n", validated as `validatePosition` does.
fn offset(lines: &[String], line: usize, column: usize) -> usize {
    let (line, column) = validate(lines, diff::Position { line, column });
    lines[..line - 1].iter().map(|line| line.chars().count() + 1).sum::<usize>() + column - 1
}

fn label_ranges<'a>(text: &[String], ranges: impl Iterator<Item = (LineRange, &'a String)>) -> Vec<String> {
    let mut chars: Vec<char> = text.join("\n").chars().collect();
    let mut edits: Vec<(usize, usize, &String)> = ranges.map(|(range, label)| (offset(text, range.start, 1), offset(text, range.end, 1), label)).collect();
    edits.sort_by_key(|edit| std::cmp::Reverse(edit.0));
    for (start, end, label) in edits {
        let replacement: Vec<char> = format!("\u{27e6}{}\u{27e7}{label}", chars[start..end].iter().collect::<String>()).chars().collect();
        chars.splice(start..end, replacement);
    }
    lines(&chars.into_iter().collect::<String>())
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| item.to_string()).collect()
}

#[test]
fn prepend_line() {
    let mut harness = Harness::new("line1\nline2", "0\nline1\nline2", "0\nline1\nline2", "");
    assert_eq!(
        harness.projections(),
        [strings(&["⟦⟧₀line1", "line2"]), strings(&["⟦0", "⟧₀line1", "line2"]), strings(&["⟦0", "⟧₀line1", "line2"]), strings(&["⟦⟧{unrecognized}₀"])]
    );
    harness.toggle(0, Input::One);
    assert_eq!(harness.result(), "0\nline1\nline2");
    harness.toggle(0, Input::Two);
    assert_eq!(harness.result(), "0\n0\nline1\nline2");
}

#[test]
fn empty_base() {
    let mut harness = Harness::new("", "input1", "input2", "");
    assert_eq!(harness.projections(), [strings(&["⟦⟧₀"]), strings(&["⟦input1⟧₀"]), strings(&["⟦input2⟧₀"]), strings(&["⟦⟧{base}₀"])]);
    harness.toggle(0, Input::One);
    assert_eq!(harness.result(), "input1");
    // Upstream's test diffs with the legacy computer, whose whole-line change cannot be combined, so it gets
    // "input2"; the advanced computer the merge editor uses finds two insertions at one position and combines them.
    harness.toggle(0, Input::Two);
    assert_eq!(harness.result(), "input1input2");
}

#[test]
fn can_merge_word_changes() {
    let mut harness = Harness::new("hello", "hallo", "helloworld", "");
    assert_eq!(harness.projections(), [strings(&["⟦hello⟧₀"]), strings(&["⟦hallo⟧₀"]), strings(&["⟦helloworld⟧₀"]), strings(&["⟦⟧{unrecognized}₀"])]);
    harness.toggle(0, Input::One);
    harness.toggle(0, Input::Two);
    assert_eq!(harness.result(), "halloworld");
}

#[test]
fn can_combine_insertions_at_end_of_document() {
    let mut harness = Harness::new(
        "Zürich\nBern\nBasel\nChur\nGenf\nThun",
        "Zürich\nBern\nChur\nDavos\nGenf\nThun\nfunction f(b:boolean) {}",
        "Zürich\nBern\nBasel (FCB)\nChur\nGenf\nThun\nfunction f(a:number) {}",
        "Zürich\nBern\nBasel\nChur\nDavos\nGenf\nThun",
    );
    assert_eq!(
        harness.projections(),
        [
            strings(&["Zürich", "Bern", "⟦Basel", "⟧₀Chur", "⟦⟧₁Genf", "Thun⟦⟧₂"]),
            strings(&["Zürich", "Bern", "⟦⟧₀Chur", "⟦Davos", "⟧₁Genf", "Thun", "⟦function f(b:boolean) {}⟧₂"]),
            strings(&["Zürich", "Bern", "⟦Basel (FCB)", "⟧₀Chur", "⟦⟧₁Genf", "Thun", "⟦function f(a:number) {}⟧₂"]),
            strings(&["Zürich", "Bern", "⟦Basel", "⟧{base}₀Chur", "⟦Davos", "⟧{1✓}₁Genf", "Thun⟦⟧{base}₂"]),
        ]
    );
    harness.toggle(2, Input::One);
    harness.toggle(2, Input::Two);
    assert_eq!(harness.result(), "Zürich\nBern\nBasel\nChur\nDavos\nGenf\nThun\nfunction f(b:boolean) {}\nfunction f(a:number) {}");
}

#[test]
fn conflicts_are_reset() {
    let harness = Harness::new(
        "import { h } from 'vs/base/browser/dom';\nimport { Disposable, IDisposable } from 'vs/base/common/lifecycle';\nimport { CodeEditorWidget } from 'vs/editor/browser/widget/codeEditorWidget';\nimport { EditorOption } from 'vs/editor/common/config/editorOptions';\nimport { autorun, IReader, observableFromEvent, ObservableValue } from 'vs/workbench/contrib/audioCues/browser/observable';\nimport { LineRange } from 'vs/workbench/contrib/mergeEditor/browser/model/lineRange';\n",
        "import { h } from 'vs/base/browser/dom';\nimport { Disposable, IDisposable } from 'vs/base/common/lifecycle';\nimport { observableSignalFromEvent } from 'vs/base/common/observable';\nimport { CodeEditorWidget } from 'vs/editor/browser/widget/codeEditorWidget';\nimport { autorun, IReader, observableFromEvent } from 'vs/workbench/contrib/audioCues/browser/observable';\nimport { LineRange } from 'vs/workbench/contrib/mergeEditor/browser/model/lineRange';\n",
        "import { h } from 'vs/base/browser/dom';\nimport { Disposable, IDisposable } from 'vs/base/common/lifecycle';\nimport { CodeEditorWidget } from 'vs/editor/browser/widget/codeEditorWidget';\nimport { autorun, IReader, observableFromEvent, ObservableValue } from 'vs/workbench/contrib/audioCues/browser/observable';\nimport { LineRange } from 'vs/workbench/contrib/mergeEditor/browser/model/lineRange';\n",
        "import { h } from 'vs/base/browser/dom';\r\nimport { Disposable, IDisposable } from 'vs/base/common/lifecycle';\r\nimport { observableSignalFromEvent } from 'vs/base/common/observable';\r\nimport { CodeEditorWidget } from 'vs/editor/browser/widget/codeEditorWidget';\r\n<<<<<<< Updated upstream\r\nimport { autorun, IReader, observableFromEvent, ObservableValue } from 'vs/workbench/contrib/audioCues/browser/observable';\r\n=======\r\nimport { autorun, IReader, observableFromEvent } from 'vs/workbench/contrib/audioCues/browser/observable';\r\n>>>>>>> Stashed changes\r\nimport { LineRange } from 'vs/workbench/contrib/mergeEditor/browser/model/lineRange';\r\n",
    );
    let [base, input1, input2, result] = harness.projections();
    assert_eq!(
        base,
        strings(&[
            "import { h } from 'vs/base/browser/dom';",
            "import { Disposable, IDisposable } from 'vs/base/common/lifecycle';",
            "⟦⟧₀import { CodeEditorWidget } from 'vs/editor/browser/widget/codeEditorWidget';",
            "⟦import { EditorOption } from 'vs/editor/common/config/editorOptions';",
            "import { autorun, IReader, observableFromEvent, ObservableValue } from 'vs/workbench/contrib/audioCues/browser/observable';",
            "⟧₁import { LineRange } from 'vs/workbench/contrib/mergeEditor/browser/model/lineRange';",
            "",
        ])
    );
    assert_eq!(
        input1,
        strings(&[
            "import { h } from 'vs/base/browser/dom';",
            "import { Disposable, IDisposable } from 'vs/base/common/lifecycle';",
            "⟦import { observableSignalFromEvent } from 'vs/base/common/observable';",
            "⟧₀import { CodeEditorWidget } from 'vs/editor/browser/widget/codeEditorWidget';",
            "⟦import { autorun, IReader, observableFromEvent } from 'vs/workbench/contrib/audioCues/browser/observable';",
            "⟧₁import { LineRange } from 'vs/workbench/contrib/mergeEditor/browser/model/lineRange';",
            "",
        ])
    );
    assert_eq!(
        input2,
        strings(&[
            "import { h } from 'vs/base/browser/dom';",
            "import { Disposable, IDisposable } from 'vs/base/common/lifecycle';",
            "⟦⟧₀import { CodeEditorWidget } from 'vs/editor/browser/widget/codeEditorWidget';",
            "⟦import { autorun, IReader, observableFromEvent, ObservableValue } from 'vs/workbench/contrib/audioCues/browser/observable';",
            "⟧₁import { LineRange } from 'vs/workbench/contrib/mergeEditor/browser/model/lineRange';",
            "",
        ])
    );
    assert_eq!(
        result,
        strings(&[
            "import { h } from 'vs/base/browser/dom';",
            "import { Disposable, IDisposable } from 'vs/base/common/lifecycle';",
            "⟦import { observableSignalFromEvent } from 'vs/base/common/observable';",
            "⟧{1✓}₀import { CodeEditorWidget } from 'vs/editor/browser/widget/codeEditorWidget';",
            "⟦<<<<<<< Updated upstream",
            "import { autorun, IReader, observableFromEvent, ObservableValue } from 'vs/workbench/contrib/audioCues/browser/observable';",
            "=======",
            "import { autorun, IReader, observableFromEvent } from 'vs/workbench/contrib/audioCues/browser/observable';",
            ">>>>>>> Stashed changes",
            "⟧{unrecognized}₁import { LineRange } from 'vs/workbench/contrib/mergeEditor/browser/model/lineRange';",
            "",
        ])
    );
}

#[test]
fn auto_solve_equal_edits() {
    let base = "const { readFileSync } = require('fs');\n\nlet paths = process.argv.slice(2);\nmain(paths);\n\nfunction main(paths) {\n    // print the welcome message\n    printMessage();\n\n    let data = getLineCountInfo(paths);\n    console.log(\"Lines: \" + data.totalLineCount);\n}\n\n/**\n * Prints the welcome message\n*/\nfunction printMessage() {\n    console.log(\"Welcome To Line Counter\");\n}\n\n/**\n * @param {string[]} paths\n*/\nfunction getLineCountInfo(paths) {\n    let lineCounts = paths.map(path => ({ path, count: getLinesLength(readFileSync(path, 'utf8')) }));\n    return {\n        totalLineCount: lineCounts.reduce((acc, { count }) => acc + count, 0),\n        lineCounts,\n    };\n}\n\n/**\n * @param {string} str\n */\nfunction getLinesLength(str) {\n    return str.split('\\n').length;\n}\n";
    let input1 = "const { readFileSync } = require('fs');\n\nlet paths = process.argv.slice(2);\nmain(paths);\n\nfunction main(paths) {\n    // print the welcome message\n    printMessage();\n\n    const data = getLineCountInfo(paths);\n    console.log(\"Lines: \" + data.totalLineCount);\n}\n\nfunction printMessage() {\n    console.log(\"Welcome To Line Counter\");\n}\n\n/**\n * @param {string[]} paths\n*/\nfunction getLineCountInfo(paths) {\n    let lineCounts = paths.map(path => ({ path, count: getLinesLength(readFileSync(path, 'utf8')) }));\n    return {\n        totalLineCount: lineCounts.reduce((acc, { count }) => acc + count, 0),\n        lineCounts,\n    };\n}\n\n/**\n * @param {string} str\n */\nfunction getLinesLength(str) {\n    return str.split('\\n').length;\n}\n";
    let input2 = "const { readFileSync } = require('fs');\n\nlet paths = process.argv.slice(2);\nrun(paths);\n\nfunction run(paths) {\n    // print the welcome message\n    printMessage();\n\n    const data = getLineCountInfo(paths);\n    console.log(\"Lines: \" + data.totalLineCount);\n}\n\nfunction printMessage() {\n    console.log(\"Welcome To Line Counter\");\n}\n\n/**\n * @param {string[]} paths\n*/\nfunction getLineCountInfo(paths) {\n    let lineCounts = paths.map(path => ({ path, count: getLinesLength(readFileSync(path, 'utf8')) }));\n    return {\n        totalLineCount: lineCounts.reduce((acc, { count }) => acc + count, 0),\n        lineCounts,\n    };\n}\n\n/**\n * @param {string} str\n */\nfunction getLinesLength(str) {\n    return str.split('\\n').length;\n}\n";
    let mut harness = Harness::new(base, input1, input2, "<<<<<<< uiae\n>>>>>>> Stashed changes");
    harness.reset();
    assert_eq!(harness.result(), input2);
    assert_eq!(harness.model.unhandled_count(), 0);
}

#[test]
fn states_toggle_as_upstream() {
    assert_eq!(State::Base.toggle(Input::One), State::Input1);
    assert_eq!(State::Input1.toggle(Input::Two), State::Both { first: Input::One, smart: true });
    assert_eq!(State::Both { first: Input::Two, smart: false }.toggle(Input::Two), State::Input1);
    assert_eq!(State::Unrecognized.toggle(Input::Two), State::Input2);
}
