//! monospaceLineBreaksComputer.test.ts without the injected text cases (the port has no injected text). The
//! incremental cases compare `createLineBreaksFromPreviousLineBreaks` with a fresh computation; the port always
//! computes fresh, so they check both fresh results.

use super::*;
use crate::editor::{EditorGroup, TextDocument};

struct Case {
    before: &'static str,
    after: &'static str,
    tab_size: usize,
    columns_for_full_width_char: f64,
    two_cells: bool,
    indent: WrappingIndent,
    keep_all: bool,
    escaped: bool,
}

impl Case {
    fn new(before: &'static str, after: &'static str) -> Self {
        Case { before, after, tab_size: 4, columns_for_full_width_char: 2., two_cells: false, indent: WrappingIndent::None, keep_all: false, escaped: false }
    }

    fn defaults() -> Self {
        Case::new(BREAK_BEFORE, BREAK_AFTER)
    }

    fn indent(mut self, indent: WrappingIndent) -> Self {
        self.indent = indent;
        self
    }

    fn full_width(mut self, columns: f64, two_cells: bool) -> Self {
        (self.columns_for_full_width_char, self.two_cells) = (columns, two_cells);
        self
    }

    fn breaks(&self, column: isize, text: &str) -> Option<LineBreaks> {
        if column == -1 {
            return None;
        }
        let options = WrapOptions {
            column: column as usize,
            viewport_wrapping: false,
            tab_size: self.tab_size,
            indent: self.indent,
            keep_all: self.keep_all,
            wrap_on_escaped_line_feeds: self.escaped,
            columns_for_full_width_char: self.columns_for_full_width_char,
            two_cell_full_width: self.two_cells,
            break_before: self.before.into(),
            break_after: self.after.into(),
        };
        line_breaks(&Classifier::new(self.before, self.after), text, &options)
    }

    fn annotated(&self, column: isize, text: &str) -> String {
        to_annotated(text, self.breaks(column, text).as_ref())
    }

    /// `assertLineBreaks`.
    fn check(&self, column: isize, annotated: &str) -> Option<LineBreaks> {
        let text = annotated.replace('|', "");
        assert_eq!(self.annotated(column, &text), annotated);
        self.breaks(column, &text)
    }

    /// `assertIncrementalLineBreaks`: both widths computed fresh.
    fn check_both(&self, text: &str, first: (isize, &str), second: (isize, &str)) {
        assert_eq!(first.1.replace('|', ""), text);
        assert_eq!(second.1.replace('|', ""), text);
        assert_eq!(self.annotated(first.0, text), first.1);
        assert_eq!(self.annotated(second.0, text), second.1);
    }
}

fn to_annotated(text: &str, breaks: Option<&LineBreaks>) -> String {
    let Some(breaks) = breaks else { return text.to_string() };
    let mut out = String::new();
    let mut previous = 0;
    for (i, c) in text.chars().enumerate() {
        let segment = breaks.segment_of(i);
        if segment != previous {
            previous = segment;
            out.push('|');
        }
        out.push(c);
    }
    out
}

#[test]
fn monospace_line_breaks_computer() {
    let case = Case::new("(", "\t).");
    case.check(5, "");
    case.check(5, "aaa");
    case.check(5, "aaaaa");
    case.check(-1, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    case.check(5, "aaaaa|a");
    case.check(5, "aaaaa|.");
    case.check(5, "aaaaa|a.|aaa.|aa");
    case.check(5, "aaaaa|a..|aaa.|aa");
    case.check(5, "aaaaa|a...|aaa.|aa");
    case.check(5, "aaaaa|a....|aaa.|aa");
    case.check(5, "\t");
    case.check(5, "\t|aaa");
    case.check(5, "\t|a\t|aa");
    case.check(5, "aa\ta");
    case.check(5, "aa\t|aa");
    case.check(5, "aaa.|aa");
    case.check(5, "aaa(.|aa");
    case.check(5, "aaa))|).aaa");
    case.check(5, "aaa))|).|aaaa");
    case.check(5, "aaa)|().|aaa");
    case.check(5, "aaa|(().|aaa");
    case.check(5, "aa.|(().|aaa");
    case.check(5, "aa.|(.).|aaa");
}

#[test]
fn incremental_1() {
    let case = Case::defaults();
    case.check_both("just some text and more", (10, "just some |text and |more"), (15, "just some text |and more"));
    case.check_both(
        "Cu scripserit suscipiantur eos, in affert pericula contentiones sed, cetero sanctus et pro. Ius vidit magna regione te, sit ei elaboraret liberavisse. Mundi verear eu mea, eam vero scriptorem in, vix in menandri assueverit. Natum definiebas cu vim. Vim doming vocibus efficiantur id. In indoctum deseruisse voluptatum vim, ad debitis verterem sed.",
        (47, "Cu scripserit suscipiantur eos, in affert |pericula contentiones sed, cetero sanctus et |pro. Ius vidit magna regione te, sit ei |elaboraret liberavisse. Mundi verear eu mea, |eam vero scriptorem in, vix in menandri |assueverit. Natum definiebas cu vim. Vim |doming vocibus efficiantur id. In indoctum |deseruisse voluptatum vim, ad debitis verterem |sed."),
        (142, "Cu scripserit suscipiantur eos, in affert pericula contentiones sed, cetero sanctus et pro. Ius vidit magna regione te, sit ei elaboraret |liberavisse. Mundi verear eu mea, eam vero scriptorem in, vix in menandri assueverit. Natum definiebas cu vim. Vim doming vocibus efficiantur |id. In indoctum deseruisse voluptatum vim, ad debitis verterem sed."),
    );
    case.check_both(
        "An his legere persecuti, oblique delicata efficiantur ex vix, vel at graecis officiis maluisset. Et per impedit voluptua, usu discere maiorum at. Ut assum ornatus temporibus vis, an sea melius pericula. Ea dicunt oblique phaedrum nam, eu duo movet nobis. His melius facilis eu, vim malorum temporibus ne. Nec no sale regione, meliore civibus placerat id eam. Mea alii fabulas definitionem te, agam volutpat ad vis, et per bonorum nonumes repudiandae.",
        (57, "An his legere persecuti, oblique delicata efficiantur ex |vix, vel at graecis officiis maluisset. Et per impedit |voluptua, usu discere maiorum at. Ut assum ornatus |temporibus vis, an sea melius pericula. Ea dicunt |oblique phaedrum nam, eu duo movet nobis. His melius |facilis eu, vim malorum temporibus ne. Nec no sale |regione, meliore civibus placerat id eam. Mea alii |fabulas definitionem te, agam volutpat ad vis, et per |bonorum nonumes repudiandae."),
        (58, "An his legere persecuti, oblique delicata efficiantur ex |vix, vel at graecis officiis maluisset. Et per impedit |voluptua, usu discere maiorum at. Ut assum ornatus |temporibus vis, an sea melius pericula. Ea dicunt oblique |phaedrum nam, eu duo movet nobis. His melius facilis eu, |vim malorum temporibus ne. Nec no sale regione, meliore |civibus placerat id eam. Mea alii fabulas definitionem |te, agam volutpat ad vis, et per bonorum nonumes |repudiandae."),
    );
    let same = Case::defaults().indent(WrappingIndent::Same);
    same.check_both("\t\t\"owner\": \"vscode\",", (14, "\t\t\"owner|\": |\"vscod|e\","), (16, "\t\t\"owner\":| |\"vscode\"|,"));
    same.check_both(
        "🐇👬🌖🌞🏇🍼🐇👬🌖🌞🏇🍼🐇👬🌖🌞🏇🍼🐇👬🌖🌞🏇🍼🐇&👬🌖🌞🏇🍼🐇👬🌖🌞🏇🍼🐇👬🌖🌞🏇🍼🐇👬🌖🌞🏇🍼🐇👬",
        (51, "🐇👬🌖🌞🏇🍼🐇👬🌖🌞🏇🍼🐇👬🌖🌞🏇🍼🐇👬🌖🌞🏇🍼🐇&|👬🌖🌞🏇🍼🐇👬🌖🌞🏇🍼🐇👬🌖🌞🏇🍼🐇👬🌖🌞🏇🍼🐇👬"),
        (50, "🐇👬🌖🌞🏇🍼🐇👬🌖🌞🏇🍼🐇👬🌖🌞🏇🍼🐇👬🌖🌞🏇🍼🐇|&|👬🌖🌞🏇🍼🐇👬🌖🌞🏇🍼🐇👬🌖🌞🏇🍼🐇👬🌖🌞🏇🍼🐇👬"),
    );
    same.check_both("🐇👬&🌞🌖", (5, "🐇👬&|🌞🌖"), (4, "🐇👬|&|🌞🌖"));
    same.check_both(
        "\t\tfunc('🌞🏇🍼🌞🏇🍼🐇&👬🌖🌞👬🌖🌞🏇🍼🐇👬', WrappingIndent.Same);",
        (26, "\t\tfunc|('🌞🏇🍼🌞🏇🍼🐇&|👬🌖🌞👬🌖🌞🏇🍼🐇|👬', |WrappingIndent.|Same);"),
        (27, "\t\tfunc|('🌞🏇🍼🌞🏇🍼🐇&|👬🌖🌞👬🌖🌞🏇🍼🐇|👬', |WrappingIndent.|Same);"),
    );
    same.check_both(
        "factory, \"xtxtfunc(x\"🌞🏇🍼🌞🏇🍼🐇&👬🌖🌞👬🌖🌞🏇🍼🐇👬x\"",
        (16, "factory, |\"xtxtfunc|(x\"🌞🏇🍼🌞🏇🍼|🐇&|👬🌖🌞👬🌖🌞🏇🍼|🐇👬x\""),
        (17, "factory, |\"xtxtfunc|(x\"🌞🏇🍼🌞🏇🍼🐇|&👬🌖🌞👬🌖🌞🏇🍼|🐇👬x\""),
    );
}

#[test]
fn issue_95686_loop_forever() {
    let text = "\t\t\t\t\t\t<tr dmx-class:table-danger=\"(alt <= 50)\" dmx-class:table-warning=\"(alt <= 200)\" dmx-class:table-primary=\"(alt <= 400)\" dmx-class:table-info=\"(alt <= 800)\" dmx-class:table-success=\"(alt >= 400)\">";
    let one_per_char: String = text.chars().map(String::from).collect::<Vec<_>>().join("|");
    Case::defaults().indent(WrappingIndent::Same).check_both(
        text,
        (179, "\t\t\t\t\t\t<tr dmx-class:table-danger=\"(alt <= 50)\" dmx-class:table-warning=\"(alt <= 200)\" dmx-class:table-primary=\"(alt <= 400)\" dmx-class:table-info=\"(alt <= 800)\" |dmx-class:table-success=\"(alt >= 400)\">"),
        (1, &one_per_char),
    );
}

#[test]
fn issue_110392_resize_with_panel_on_the_right() {
    let text = "你好 **hello** **hello** **hello-world** hey there!";
    let one_per_char: String = text.chars().map(String::from).collect::<Vec<_>>().join("|");
    for two_cells in [false, true] {
        Case::defaults()
            .indent(WrappingIndent::Same)
            .full_width(1.6605405405405405, two_cells)
            .check_both(text, (15, "你好 **hello** |**hello** |**hello-world**| hey there!"), (1, &one_per_char));
    }
}

#[test]
fn cjk_and_kinsoku_shori() {
    let case = Case::new("(", "\t)");
    case.check(5, "aa \u{5b89}|\u{5b89}");
    case.check(5, "\u{3042} \u{5b89}|\u{5b89}");
    case.check(5, "\u{3042}\u{3042}|\u{5b89}\u{5b89}");
    case.check(5, "aa |\u{5b89})\u{5b89}|\u{5b89}");
    case.check(5, "aa \u{3042}|\u{5b89}\u{3042})|\u{5b89}");
    case.check(5, "aa |(\u{5b89}aa|\u{5b89}");
}

#[test]
fn two_cell_full_width_character_width() {
    let text = "\u{3042}".repeat(4);
    let wrap = |columns: f64, two_cells: bool| Case::new("(", "\t)").full_width(columns, two_cells).annotated(5, &text);
    assert_eq!(wrap(3., false), "\u{3042}|\u{3042}|\u{3042}|\u{3042}");
    assert_eq!(wrap(3., true), "\u{3042}\u{3042}|\u{3042}\u{3042}");
    assert_eq!(wrap(2., false), "\u{3042}\u{3042}|\u{3042}\u{3042}");
}

#[test]
fn two_cell_width_determines_whether_continuation_indentation_fits() {
    let text = "    \u{3042}\u{3042}\u{3042}";
    let indent = |column: isize, columns: f64, two_cells: bool| Case::new("(", "\t)").indent(WrappingIndent::Same).full_width(columns, two_cells).breaks(column, text).map(|breaks| breaks.wrapped_indent);
    assert_eq!(indent(6, 3., false), Some(0));
    assert_eq!(indent(6, 3., true), Some(4));
    assert_eq!(indent(5, 1., false), Some(4));
    assert_eq!(indent(5, 1., true), Some(0));
}

#[test]
fn two_cell_width_uses_shared_full_width_classification() {
    let wrap = |text: &str, two_cells: bool| Case::new("(", "\t)").full_width(3., two_cells).annotated(5, text);
    assert_eq!(wrap(&"\u{FF21}".repeat(4), false), "\u{FF21}|\u{FF21}|\u{FF21}|\u{FF21}");
    assert_eq!(wrap(&"\u{FF21}".repeat(4), true), "\u{FF21}\u{FF21}|\u{FF21}\u{FF21}");
    assert_eq!(wrap(&"\u{A500}".repeat(4), false), "\u{A500}|\u{A500}|\u{A500}|\u{A500}");
    assert_eq!(wrap(&"\u{A500}".repeat(4), true), "\u{A500}\u{A500}|\u{A500}\u{A500}");
}

#[test]
fn two_cell_width_preserves_natural_control_character_metrics() {
    for two_cells in [false, true] {
        let case = Case::new("(", "\t)").full_width(3., two_cells);
        assert_eq!(case.annotated(4, "\u{1}\u{1}"), "\u{1}|\u{1}");
        assert_eq!(case.breaks(3, "\u{1}\t").map(|breaks| breaks.visible_columns), Some(vec![3., 4.]));
    }
}

#[test]
fn two_cell_width_treats_combining_marks_independently() {
    for two_cells in [false, true] {
        assert_eq!(Case::new("(", "\t)").full_width(3., two_cells).annotated(4, "\u{3042}\u{301}aa"), "\u{3042}|\u{301}aa");
    }
}

#[test]
fn two_cell_width_supports_incremental_line_breaks() {
    let case = Case::defaults().full_width(3., true);
    case.check_both("ああああああ", (5, "ああ|ああ|ああ"), (7, "あああ|あああ"));
    case.indent(WrappingIndent::Same).check_both("    ああああ", (8, "    ああ|ああ"), (10, "    あああ|あ"));
}

#[test]
fn wrapping_indent_same() {
    Case::new("", "\t ").indent(WrappingIndent::Same).check(38, " *123456789012345678901234567890123456|7890");
}

#[test]
fn issue_16332_scroll_bar_overlaying_on_top_of_text() {
    Case::new("", "\t ").indent(WrappingIndent::Indent).check(24, "a/ very/long/line/of/tex|t/that/expands/beyon|d/your/typical/line/|of/code/");
}

#[test]
fn issue_35162_wrapping_indent_not_consistently_working() {
    let breaks = Case::new("", "\t ").indent(WrappingIndent::Indent).check(24, "                t h i s |i s |a l |o n |g l |i n |e").unwrap();
    assert_eq!(breaks.wrapped_indent, 20);
}

#[test]
fn issue_75494_surrogate_pairs() {
    Case::new("\t", " ").indent(WrappingIndent::Same).check(49, "🐇👬🌖🌞🏇🍼🐇👬🌖🌞🏇🍼🐇👬🌖🌞🏇🍼🐇👬🌖🌞🏇🍼|🐇👬🌖🌞🏇🍼🐇👬🌖🌞🏇🍼🐇👬🌖🌞🏇🍼🐇👬🌖🌞🏇🍼|🐇👬");
    Case::defaults().indent(WrappingIndent::Same).check(4, "🐇👬|&|🌞🌖");
    Case::defaults().indent(WrappingIndent::Same).check(17, "factory, |\"xtxtfunc|(x\"🌞🏇🍼🌞🏇🍼🐇|&👬🌖🌞👬🌖🌞🏇🍼|🐇👬x\"");
}

#[test]
fn wrapping_indent_deep_indent() {
    let breaks = Case::new("", "\t ").indent(WrappingIndent::DeepIndent).check(26, "        W e A r e T e s t |i n g D e |e p I n d |e n t a t |i o n").unwrap();
    assert_eq!(breaks.wrapped_indent, 16);
}

#[test]
fn issues_33366_152773_112382_punctuation_and_control_characters() {
    let same = Case::defaults().indent(WrappingIndent::Same);
    same.check(23, "this is a line of |text, text that sits |on a line");
    same.check(24, "this is a line of |(text), text that sits |on a line");
    same.check(6, "\u{6}\u{6}\u{6}|\u{6}\u{6}\u{6}");
}

#[test]
fn word_break_with_cjk_text() {
    Case::defaults().indent(WrappingIndent::Same).check(5, "你好|1111");
    let mut keep_all = Case::defaults().indent(WrappingIndent::Same);
    keep_all.keep_all = true;
    keep_all.check(8, "你好1111");
}

#[test]
fn issue_258022_wrap_on_escaped_line_feeds() {
    let mut case = Case::defaults();
    case.escaped = true;
    let text = "\"Short text with\\nescaped newline and an escaped\\\\nbackslash\"";
    assert!(case.annotated(30, text).contains("with\\n|"));
    assert!(case.annotated(35, text).contains("with\\n|"));
    assert!(!case.annotated(35, text).contains("\\\\n|"));
}

fn editor(text: &str, column: usize) -> EditorGroup {
    let languages = crate::languages::Languages::new(&[]);
    let document = TextDocument::from_bytes("file", std::path::Path::new("/a.txt"), text.as_bytes(), None, &languages, &crate::default_settings().lock().unwrap());
    let mut group = EditorGroup::default();
    group.open(document, false);
    let editor = group.active_mut().unwrap();
    let options = WrapOptions {
        column,
        viewport_wrapping: true,
        tab_size: 4,
        indent: WrappingIndent::Same,
        keep_all: false,
        wrap_on_escaped_line_feeds: false,
        columns_for_full_width_char: 2.,
        two_cell_full_width: false,
        break_before: BREAK_BEFORE.into(),
        break_after: BREAK_AFTER.into(),
    };
    let lines = editor.document().lines.clone();
    editor.word_wrap.configure(Some(options), &lines);
    group
}

#[test]
fn view_lines_map_wrapped_and_folded_lines() {
    let mut group = editor("aaaa bbbb cccc\nx\n  yyy zzzz", 6);
    let editor = group.active_mut().unwrap();
    // "aaaa |bbbb |cccc", "x", "  yyy |  zzzz".
    assert_eq!(editor.view_line_count(), 6);
    assert_eq!((editor.view_line(1), editor.view_line(2)), (3, 4));
    assert_eq!((0..6).map(|line| editor.model_line(line)).collect::<Vec<_>>(), [0, 0, 0, 1, 2, 2]);
    let segment = editor.segment(5);
    assert_eq!((segment.start, segment.end, segment.indent, segment.first, segment.last), (6, 10, 2, false, true));
    assert_eq!(editor.position_view_line(Position::new(0, 5), false), 1);
    assert_eq!(editor.position_view_line(Position::new(0, 5), true), 0);
    // An edit breaks the changed lines again.
    editor.type_text("dddd ");
    assert_eq!(editor.view_line_count(), 7);
    editor.set_selections(vec![Selection::caret(Position::new(1, 1))]);
    editor.type_text(" wwwww");
    assert_eq!(editor.view_line_count(), 8);
    // Line 2 starts after the wrapped lines of lines 0 and 1.
    editor.set_selections(vec![Selection::caret(Position::new(2, 0))]);
    assert_eq!(editor.view_line(2), 6);
}

#[test]
fn cursor_moves_go_by_view_lines() {
    let mut group = editor("aaaa bbbb cccc\nxy", 6);
    let editor = group.active_mut().unwrap();
    editor.set_selections(vec![Selection::caret(Position::new(0, 2))]);
    editor.move_vertically(true, false);
    assert_eq!(editor.selection().head, Position::new(0, 7));
    editor.move_vertically(true, false);
    assert_eq!(editor.selection().head, Position::new(0, 12));
    editor.move_vertically(true, false);
    assert_eq!(editor.selection().head, Position::new(1, 2));
    editor.move_vertically(false, false);
    // The leftover column comes back on the longer view line.
    assert_eq!(editor.selection().head, Position::new(0, 12));
    // End stops at the view line's end, then goes to the model line's.
    editor.set_selections(vec![Selection::caret(Position::new(0, 1))]);
    editor.move_to_line_edge(true, false);
    assert_eq!(editor.selection().head, Position::new(0, 5));
    assert!(editor.head_at_row_end(0));
    assert_eq!(editor.head_view_line(0), 0);
    editor.move_to_line_edge(true, false);
    assert_eq!(editor.selection().head, Position::new(0, 14));
    // Home goes to the view line's start, then the model line's.
    editor.set_selections(vec![Selection::caret(Position::new(0, 7))]);
    editor.move_to_line_edge(false, false);
    assert_eq!(editor.selection().head, Position::new(0, 5));
    editor.move_to_line_edge(false, false);
    assert_eq!(editor.selection().head, Position::new(0, 0));
}
