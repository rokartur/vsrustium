//! The 'Auto Indent On Paste' suites and the Ruby, PHP, Go, CPP, HTML, Visual Basic, Latex and Lua 'Auto Indent
//! On Type' suites of indentation.test.ts, on the language configurations of
//! editor/test/common/modes/supports. Positions are upstream's 1-based (line, column).

use super::*;
use crate::syntax::Tokenization;

fn javascript_indentation_rules() -> Value {
    json!({
        "decreaseIndentPattern": r"^((?!.*?\/\*).*\*\/)?\s*[\}\]\)].*$",
        "increaseIndentPattern": r#"^((?!\/\/).)*(\{([^}"'`]*|(\t|[ ])*\/\/.*)|\([^)"'`]*|\[[^\]"'`]*)$"#,
        "unIndentedLinePattern": r"^(\t|[ ])*[ ]\*[^/]*\*\/\s*$|^(\t|[ ])*[ ]\*\/\s*$|^(\t|[ ])*[ ]\*([ ]([^\*]|\*(?!\/))*)?$",
        "indentNextLinePattern": r"^((.*=>\s*)|((.*[^\w]+|\s*)(if|while|for)\s*\(.*\)\s*))$",
    })
}

fn javascript_on_enter_rules() -> Value {
    json!([
        { "beforeText": r"^\s*\/\*\*(?!\/)([^\*]|\*(?!\/))*$", "afterText": r"^\s*\*\/$", "action": { "indent": "indentOutdent", "appendText": " * " } },
        { "beforeText": r"^\s*\/\*\*(?!\/)([^\*]|\*(?!\/))*$", "action": { "indent": "none", "appendText": " * " } },
        { "beforeText": r"^(\t|[ ])*[ ]\*([ ]([^\*]|\*(?!\/))*)?$", "previousLineText": r"(?=^(\s*(\/\*\*|\*)).*)(?=(?!(\s*\*\/)))", "action": { "indent": "none", "appendText": "* " } },
        { "beforeText": r"^(\t|[ ])*[ ]\*\/\s*$", "action": { "indent": "none", "removeText": 1 } },
        { "beforeText": r"^(\t|[ ])*[ ]\*[^/]*\*\/\s*$", "action": { "indent": "none", "removeText": 1 } },
        { "beforeText": r"^\s*(\bcase\s.+:|\bdefault:)$", "afterText": r"^(?!\s*(\bcase\b|\bdefault\b))", "action": { "indent": "indent" } },
        { "previousLineText": r"^\s*(((else ?)?if|for|while)\s*\(.*\)\s*|else\s*)$", "beforeText": r"^\s+([^{i\s]|i(?!f\b))", "action": { "indent": "outdent" } },
        { "beforeText": r"^.*\([^\)]*$", "afterText": r"^\s*\).*$", "action": { "indent": "indentOutdent", "appendText": "\t" } },
        { "beforeText": r"^.*\{[^\}]*$", "afterText": r"^\s*\}.*$", "action": { "indent": "indentOutdent", "appendText": "\t" } },
        { "beforeText": r"^.*\[[^\]]*$", "afterText": r"^\s*\].*$", "action": { "indent": "indentOutdent", "appendText": "\t" } },
    ])
}

fn standard_brackets() -> Value {
    json!([["{", "}"], ["[", "]"], ["(", ")"]])
}

fn typescript() -> Value {
    json!({
        "brackets": [["${", "}"], ["{", "}"], ["[", "]"], ["(", ")"]],
        "comments": { "lineComment": "//", "blockComment": ["/*", "*/"] },
        "autoClosingPairs": [
            { "open": "{", "close": "}" }, { "open": "[", "close": "]" }, { "open": "(", "close": ")" },
            { "open": "'", "close": "'", "notIn": ["string", "comment"] }, { "open": "\"", "close": "\"", "notIn": ["string"] },
            { "open": "`", "close": "`", "notIn": ["string", "comment"] }, { "open": "/**", "close": " */", "notIn": ["string"] },
        ],
        "indentationRules": javascript_indentation_rules(),
        "onEnterRules": javascript_on_enter_rules(),
    })
}

fn ruby() -> Value {
    json!({ "brackets": standard_brackets(), "indentationRules": {
        "decreaseIndentPattern": r"^\s*([}\]]([,)]?\s*(#|$)|\.[a-zA-Z_]\w*\b)|(end|rescue|ensure|else|elsif)\b|(in|when)\s)",
        "increaseIndentPattern": r#"^\s*((begin|class|(private|protected)\s+def|def|else|elsif|ensure|for|if|module|rescue|unless|until|when|in|while|case)|([^#]*\sdo\b)|([^#]*=\s*(case|if|unless)))\b([^#\{;]|(\"|'|\/).*\4)*(#.*)?$"#,
    } })
}

fn php() -> Value {
    json!({
        "brackets": standard_brackets(),
        "indentationRules": {
            "increaseIndentPattern": r"({(?!.*}).*|\(|\[|((else(\s)?)?if|else|for(each)?|while|switch|case).*:)\s*((\/[/*].*|)?$|\?>)",
            "decreaseIndentPattern": r"^(.*\*\/)?\s*((\})|(\)+[;,])|(\]\)*[;,])|\b(else:)|\b((end(if|for(each)?|while|switch));))",
        },
        "onEnterRules": [
            { "beforeText": r"^\s*\/\*\*(?!\/)([^\*]|\*(?!\/))*$", "afterText": r"^\s*\*\/$", "action": { "indent": "indentOutdent", "appendText": " * " } },
            { "beforeText": r"^\s*\/\*\*(?!\/)([^\*]|\*(?!\/))*$", "action": { "indent": "none", "appendText": " * " } },
            { "beforeText": r"^(\t|(\ \ ))*\ \*(\ ([^\*]|\*(?!\/))*)?$", "action": { "indent": "none", "appendText": "* " } },
            { "beforeText": r"^(\t|(\ \ ))*\ \*\/\s*$", "action": { "indent": "none", "removeText": 1 } },
            { "beforeText": r"^(\t|(\ \ ))*\ \*[^/]*\*\/\s*$", "action": { "indent": "none", "removeText": 1 } },
            { "beforeText": r"^\s+([^{i\s]|i(?!f\b))", "previousLineText": r"^\s*(((else ?)?if|for(each)?|while)\s*\(.*\)\s*|else\s*)$", "action": { "indent": "outdent" } },
        ],
    })
}

fn go() -> Value {
    json!({ "brackets": standard_brackets(), "indentationRules": {
        "decreaseIndentPattern": r"^\s*(\bcase\b.*:|\bdefault\b:|}[)}]*[),]?|\)[,]?)$",
        "increaseIndentPattern": r#"^.*(\bcase\b.*:|\bdefault\b:|(\b(func|if|else|switch|select|for|struct)\b.*)?{[^}"'`]*|\([^)"'`]*)$"#,
    } })
}

fn cpp() -> Value {
    json!({ "brackets": standard_brackets(), "onEnterRules": [
        { "previousLineText": r"^\s*(((else ?)?if|for|while)\s*\(.*\)\s*|else\s*)$", "beforeText": r"^\s+([^{i\s]|i(?!f\b))", "action": { "indent": "outdent" } },
    ] })
}

fn html() -> Value {
    let open_tag = r#"<(?!(?:area|base|br|col|embed|hr|img|input|keygen|link|menuitem|meta|param|source|track|wbr))([_:\w][_:\w\-.\d]*)(?:(?:[^'"/>]|"[^"]*"|'[^']*')*?(?!\/)>)[^<]*$"#;
    json!({
        "brackets": [["<!--", "-->"], ["{", "}"], ["(", ")"]],
        "indentationRules": {
            "decreaseIndentPattern": r"^\s*(<\/(?!html)[-_\.A-Za-z0-9]+\b[^>]*>|-->|\})",
            "increaseIndentPattern": r#"<(?!\?|(?:area|base|br|col|frame|hr|html|img|input|keygen|link|menuitem|meta|param|source|track|wbr)\b|[^>]*\/>)([-_\.A-Za-z0-9]+)(?=\s|>)\b[^>]*>(?!.*<\/\1>)|<!--(?!.*-->)|\{[^}"']*$"#,
        },
        "onEnterRules": [
            { "beforeText": { "pattern": open_tag, "flags": "i" }, "afterText": { "pattern": r"^<\/([_:\w][_:\w\-.\d]*)\s*>", "flags": "i" }, "action": { "indent": "indentOutdent" } },
            { "beforeText": { "pattern": open_tag, "flags": "i" }, "action": { "indent": "indent" } },
        ],
    })
}

fn visual_basic() -> Value {
    json!({
        "brackets": standard_brackets(),
        "indentationRules": {
            "decreaseIndentPattern": { "pattern": r"^\s*((End\s+(If|Sub|Function|Class|Module|Enum|Structure|Interface|Namespace|With|Select|Try|While|For|Property|Get|Set|SyncLock|Using|AddHandler|RaiseEvent|RemoveHandler|Event|Operator))|Else|ElseIf|Case|Catch|Finally|Loop|Next|Wend|Until)\b", "flags": "i" },
            "increaseIndentPattern": { "pattern": r"^\s*((If|ElseIf).*Then(?!.*End\s+If)\s*(('|REM).*)?|(Else|While|For|Do|Select\s+Case|Case|Sub|Function|Class|Module|Enum|Structure|Interface|Namespace|With|Try|Catch|Finally|SyncLock|Using|Property|Get|Set|AddHandler|RaiseEvent|RemoveHandler|Event|Operator)\b(?!.*\bEnd\s+(If|Sub|Function|Class|Module|Enum|Structure|Interface|Namespace|With|Select|Try|While|For|Property|Get|Set|SyncLock|Using|AddHandler|RaiseEvent|RemoveHandler|Event|Operator)\b).*(('|REM).*)?)$", "flags": "i" },
        },
        "onEnterRules": [
            { "beforeText": { "pattern": r"^\s*((End\s+(If|Sub|Function|Class|Module|Enum|Structure|Interface|Namespace|With|Select|Try|While|For|Property|Get|Set|SyncLock|Using|AddHandler|RaiseEvent|RemoveHandler|Event|Operator))|Loop|Next|Wend|Until)\b.*$", "flags": "i" }, "action": { "indent": "none" } },
        ],
    })
}

fn latex() -> Value {
    let mut brackets = vec![["{", "}"], ["[", "]"], ["(", ")"], ["[", ")"], ["(", "]"]];
    brackets.extend([
        ["\\left(", "\\right)"], ["\\left(", "\\right."], ["\\left.", "\\right)"], ["\\left[", "\\right]"], ["\\left[", "\\right."], ["\\left.", "\\right]"],
        ["\\left\\{", "\\right\\}"], ["\\left\\{", "\\right."], ["\\left.", "\\right\\}"], ["\\left<", "\\right>"],
        ["\\bigl(", "\\bigr)"], ["\\bigl[", "\\bigr]"], ["\\bigl\\{", "\\bigr\\}"], ["\\Bigl(", "\\Bigr)"], ["\\Bigl[", "\\Bigr]"], ["\\Bigl\\{", "\\Bigr\\}"],
        ["\\biggl(", "\\biggr)"], ["\\biggl[", "\\biggr]"], ["\\biggl\\{", "\\biggr\\}"], ["\\Biggl(", "\\Biggr)"], ["\\Biggl[", "\\Biggr]"], ["\\Biggl\\{", "\\Biggr\\}"],
        ["\\langle", "\\rangle"], ["\\lvert", "\\rvert"], ["\\lVert", "\\rVert"], ["\\left|", "\\right|"], ["\\left\\vert", "\\right\\vert"], ["\\left\\|", "\\right\\|"],
        ["\\left\\Vert", "\\right\\Vert"], ["\\left\\langle", "\\right\\rangle"], ["\\left\\lvert", "\\right\\rvert"], ["\\left\\lVert", "\\right\\rVert"],
        ["\\bigl\\langle", "\\bigr\\rangle"], ["\\bigl|", "\\bigr|"], ["\\bigl\\vert", "\\bigr\\vert"], ["\\bigl\\lvert", "\\bigr\\rvert"], ["\\bigl\\|", "\\bigr\\|"],
        ["\\bigl\\lVert", "\\bigr\\rVert"], ["\\bigl\\Vert", "\\bigr\\Vert"], ["\\Bigl\\langle", "\\Bigr\\rangle"], ["\\Bigl|", "\\Bigr|"], ["\\Bigl\\lvert", "\\Bigr\\rvert"],
        ["\\Bigl\\vert", "\\Bigr\\vert"], ["\\Bigl\\|", "\\Bigr\\|"], ["\\Bigl\\lVert", "\\Bigr\\rVert"], ["\\Bigl\\Vert", "\\Bigr\\Vert"], ["\\biggl\\langle", "\\biggr\\rangle"],
        ["\\biggl|", "\\biggr|"], ["\\biggl\\lvert", "\\biggr\\rvert"], ["\\biggl\\vert", "\\biggr\\vert"], ["\\biggl\\|", "\\biggr\\|"], ["\\biggl\\lVert", "\\biggr\\rVert"],
        ["\\biggl\\Vert", "\\biggr\\Vert"], ["\\Biggl\\langle", "\\Biggr\\rangle"], ["\\Biggl|", "\\Biggr|"], ["\\Biggl\\lvert", "\\Biggr\\rvert"], ["\\Biggl\\vert", "\\Biggr\\vert"],
        ["\\Biggl\\|", "\\Biggr\\|"], ["\\Biggl\\lVert", "\\Biggr\\rVert"], ["\\Biggl\\Vert", "\\Biggr\\Vert"],
    ]);
    let auto_closing = [
        ["\\left(", "\\right)"], ["\\left[", "\\right]"], ["\\left\\{", "\\right\\}"], ["\\bigl(", "\\bigr)"], ["\\bigl[", "\\bigr]"], ["\\bigl\\{", "\\bigr\\}"],
        ["\\Bigl(", "\\Bigr)"], ["\\Bigl[", "\\Bigr]"], ["\\Bigl\\{", "\\Bigr\\}"], ["\\biggl(", "\\biggr)"], ["\\biggl[", "\\biggr]"], ["\\biggl\\{", "\\biggr\\}"],
        ["\\Biggl(", "\\Biggr)"], ["\\Biggl[", "\\Biggr]"], ["\\Biggl\\{", "\\Biggr\\}"], ["\\(", "\\)"], ["\\[", "\\]"], ["\\{", "\\}"], ["{", "}"], ["[", "]"], ["(", ")"], ["`", "'"],
    ];
    json!({
        "brackets": brackets,
        "autoClosingPairs": auto_closing.map(|[open, close]| json!({ "open": open, "close": close })),
        "indentationRules": { "decreaseIndentPattern": r"^\s*\\end{(?!document)", "increaseIndentPattern": r"\\begin{(?!document)([^}]*)}(?!.*\\end{\1})" },
    })
}

fn lua() -> Value {
    json!({ "brackets": standard_brackets(), "indentationRules": {
        "decreaseIndentPattern": r"^\s*((\b(elseif|else|end|until)\b)|(\})|(\)))",
        "increaseIndentPattern": r"^((?!(\-\-)).)*((\b(else|function|then|do|repeat)\b((?!\b(end|until)\b).)*)|(\{\s*))$",
    } })
}

impl Cursor {
    /// `registerTokenizationSupport`: comments and strings tokenized by tests/grammar.json.
    fn tokenized(mut self) -> Self {
        let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/editor/auto_indent/tests");
        let package = json!({ "contributes": { "grammars": [{ "language": "testLanguage", "scopeName": "source.indentation-test", "path": "grammar.json" }] } });
        let registry = textmate::Registry::new([(folder.as_path(), &package)]);
        self.editor.document.tokens = Tokenization::new(registry.grammar("testLanguage"));
        self
    }

    /// `viewModel.type(text)` without the 'keyboard' source: no interceptors.
    fn type_plain(&mut self, text: &str) {
        self.editor.type_text(text);
    }

    /// `viewModel.paste(text, true)`, then `AutoIndentOnPaste.trigger(range)`.
    fn paste(&mut self, text: &str, (start_line, start_column): (usize, usize), (end_line, end_column): (usize, usize)) {
        self.editor.paste(crate::editor::Pasted { text, paste_on_new_line: true, multicursor_text: None, is_block: false }, true, false);
        let auto = AutoIndent { languages: &self.languages, strategy: self.strategy };
        self.editor.auto_indent_on_paste(Position::new(start_line - 1, start_column - 1), Position::new(end_line - 1, end_column - 1), auto, true);
    }
}

mod paste_typescript {
    use super::*;

    #[test]
    fn issue_119225_do_not_add_extra_leading_space_when_pasting_jsdoc() {
        let mut cursor = Cursor::new(typescript(), &[""]).tokenized();
        let text = "/**\n * JSDoc\n */\nfunction a() {}";
        cursor.paste(text, (1, 1), (4, 16));
        assert_eq!(cursor.value(), text);
    }

    #[test]
    fn issue_167299_blank_line_removes_indent() {
        let mut cursor = Cursor::new(typescript(), &[""]);
        let text = "\nexport type IncludeReference =\n\t| BaseReference\n\t| SelfReference\n\t| RelativeReference;\n\nexport const enum IncludeReferenceKind {\n\tBase,\n\tSelf,\n\tRelativeReference,\n}";
        cursor.paste(text, (1, 1), (11, 2));
        assert_eq!(cursor.value(), text);
    }

    #[test]
    fn issue_29803_do_not_indent_when_pasting_text_with_only_one_line() {
        let mut cursor = Cursor::new(typescript(), &["const linkHandler = new Class(a, b, c,", "    d)"]);
        cursor.move_to(2, 6);
        cursor.paste(", null", (2, 6), (2, 11));
        assert_eq!(cursor.value(), "const linkHandler = new Class(a, b, c,\n    d, null)");
    }

    #[test]
    fn issue_29753_incorrect_indentation_after_comment() {
        let mut cursor = Cursor::new(typescript(), &["class A {", "    /**", "     * used only for debug purposes.", "     */", "    private _codeInfo: KeyMapping[];", "}"]);
        cursor.select((5, 24), (5, 34));
        cursor.paste("IMacLinuxKeyMapping", (5, 24), (5, 43));
        assert_eq!(cursor.value(), "class A {\n    /**\n     * used only for debug purposes.\n     */\n    private _codeInfo: IMacLinuxKeyMapping[];\n}");
    }

    #[test]
    fn issue_29753_incorrect_indentation_of_header_comment() {
        let mut cursor = Cursor::new(typescript(), &[""]);
        let text = "/*----------------\n *  Copyright (c) \n *  Licensed under ...\n *-----------------*/";
        cursor.paste(text, (1, 1), (4, 22));
        assert_eq!(cursor.value(), text);
    }

    #[test]
    fn issue_209859_do_not_do_change_indentation_when_pasted_inside_of_a_string() {
        let text = "const foo = \"some text\n         which is strangely\n    indented\"";
        let mut cursor = Cursor::new(typescript(), &text.split('\n').collect::<Vec<_>>()).tokenized();
        cursor.select((2, 10), (2, 15));
        cursor.paste("which", (2, 1), (2, 28));
        assert_eq!(cursor.value(), text);
    }

    /// Not upstream: every upstream case above keeps the pasted indentation.
    #[test]
    fn pasted_lines_take_the_indentation_of_the_block() {
        let mut cursor = Cursor::new(typescript(), &["function f() {", "", "}"]);
        cursor.move_to(2, 1);
        cursor.paste("a();\n  b();", (2, 1), (3, 7));
        assert_eq!((cursor.value().as_str(), cursor.cursor()), ("function f() {\n    a();\n    b();\n}", (3, 9, 3, 9)));
    }

    #[test]
    #[ignore = "test.skip upstream"]
    fn issue_181065_incorrect_paste_of_object_within_comment() {
        let mut cursor = Cursor::new(typescript(), &[""]).tokenized();
        let text = "/**\n * @typedef {\n * }\n */";
        cursor.paste(text, (1, 1), (4, 4));
        assert_eq!(cursor.value(), text);
    }

    #[test]
    #[ignore = "test.skip upstream"]
    fn issue_86301_preserve_cursor_at_inserted_indentation_level() {
        let mut cursor = Cursor::new(typescript(), &["() => {", "", "}"]);
        cursor.move_to(2, 1);
        cursor.paste("() => {\n\n}\n", (2, 1), (5, 1));
        assert_eq!((cursor.value().as_str(), cursor.cursor()), ("() => {\n    () => {\n    \n    }\n    \n}", (5, 5, 5, 5)));
    }

    #[test]
    #[ignore = "test.skip upstream"]
    fn issue_85781_indent_line_with_extra_white_space() {
        let mut cursor = Cursor::new(typescript(), &["() => {", "    console.log(\"a\");", "}"]);
        cursor.move_to(2, 5);
        cursor.paste("() => {\n    console.log(\"b\")\n}\n ", (2, 5), (5, 6));
        assert_eq!(cursor.value(), "() => {\n    () => {\n        console.log(\"b\")\n    }\n    console.log(\"a\");\n}");
    }

    #[test]
    #[ignore = "test.skip upstream"]
    fn issue_29589_incorrect_indentation_of_closing_brace_on_paste() {
        let mut cursor = Cursor::new(typescript(), &[""]);
        let text = "function makeSub(a,b) {\nsubsent = sent.substring(a,b);\nreturn subsent;\n}";
        cursor.paste(text, (1, 1), (4, 2));
        assert_eq!(cursor.value(), "function makeSub(a,b) {\nsubsent = sent.substring(a,b);\nreturn subsent;\n}");
    }

    #[test]
    #[ignore = "test.skip upstream"]
    fn issue_201420_incorrect_indentation_when_first_line_is_comment() {
        let mut cursor = Cursor::new(typescript(), &["function bar() {", "", "}"]).tokenized();
        cursor.move_to(2, 1);
        cursor.paste("// comment\nconst foo = 42", (2, 1), (3, 15));
        assert_eq!(cursor.value(), "function bar() {\n    // comment\n    const foo = 42\n}");
    }
}

#[test]
fn ruby_issue_198350_in_or_when_incorrectly_match_non_keywords() {
    let mut cursor = Cursor::new(ruby(), &[""]);
    cursor.type_plain("def foo\n        i");
    cursor.type_text("n");
    assert_eq!(cursor.value(), "def foo\n        in");
    cursor.type_text(" ");
    assert_eq!(cursor.value(), "def foo\nin ");
    let mut cursor = Cursor::new(ruby(), &[""]);
    cursor.type_plain("  # in");
    cursor.type_text(" ");
    assert_eq!(cursor.value(), "  # in ");
}

#[test]
#[ignore = "test.skip upstream"]
fn ruby_issue_199846_in_or_when_incorrectly_match_non_keywords() {
    let mut cursor = Cursor::new(ruby(), &[""]);
    cursor.type_plain("method('#foo') do");
    cursor.type_text("\n");
    assert_eq!(cursor.value(), "method('#foo') do\n    ");
}

#[test]
fn php_issue_199050_should_not_indent_after_brace_detected_in_a_string() {
    let mut cursor = Cursor::new(php(), &["preg_replace('{');"]).tokenized();
    // Upstream's (1, 54) clamps to the line end.
    cursor.move_to(1, 19);
    cursor.type_text("\n");
    assert_eq!(cursor.value(), "preg_replace('{');\n");
}

#[test]
#[ignore = "test.skip upstream"]
fn go_issue_199050_should_not_indent_after_brace_detected_in_a_string() {
    let mut cursor = Cursor::new(go(), &["var s = `", "quick  brown", "fox", "`"]);
    cursor.move_to(3, 1);
    cursor.paste("  ", (3, 1), (3, 3));
    assert_eq!(cursor.value(), "var s = `\nquick  brown\n  fox\n`");
}

#[test]
#[ignore = "test.skip upstream"]
fn cpp_issue_178334_incorrect_outdent_of_brace_when_signature_spans_multiple_lines() {
    let mut cursor = Cursor::new(cpp(), &["int WINAPI WinMain(bool instance,", "    int nshowcmd) {}"]);
    cursor.move_to(2, 20);
    cursor.type_text("\n");
    assert_eq!(cursor.value(), "int WINAPI WinMain(bool instance,\n    int nshowcmd) {\n    \n}");
}

#[test]
#[ignore = "test.skip upstream"]
fn cpp_issue_118929_incorrect_indent_when_comment_follows_curly_brace() {
    let mut cursor = Cursor::new(cpp(), &["if (true) { // jaja", "}"]);
    cursor.move_to(1, 20);
    cursor.type_text("\n");
    assert_eq!(cursor.value(), "if (true) { // jaja\n    \n}");
}

#[test]
#[ignore = "test.skip upstream"]
fn cpp_issue_111265_auto_indentation_set_to_none_still_changes_the_indentation() {
    let mut cursor = Cursor::new(cpp(), &["int func() {", "\t\t"]).strategy(Strategy::None);
    cursor.move_to(2, 3);
    cursor.type_text("}");
    assert_eq!(cursor.value(), "int func() {\n\t\t}");
}

#[test]
#[ignore = "test.skip upstream"]
fn html_issue_61510_incorrect_indentation_after_comment_in_html_file() {
    let mut cursor = Cursor::new(html(), &["<pre>", "  foo //I press <Enter> at the end of this line", "</pre>"]);
    cursor.move_to(2, 48);
    cursor.type_text("\n");
    assert_eq!(cursor.value(), "<pre>\n  foo //I press <Enter> at the end of this line\n  \n</pre>");
}

#[test]
#[ignore = "test.skip upstream"]
fn latex_issue_178075_no_auto_closing_pair_when_indentation_done() {
    let mut cursor = Cursor::new(latex(), &["\\begin{theorem}", "    \\end"]);
    cursor.move_to(2, 9);
    cursor.type_text("{");
    assert_eq!(cursor.value(), "\\begin{theorem}\n\\end{}");
}

#[test]
#[ignore = "test.skip upstream"]
fn lua_issue_178075_no_auto_closing_pair_when_indentation_done() {
    let mut cursor = Cursor::new(lua(), &["print(\"asdf function asdf\")"]);
    cursor.move_to(1, 28);
    cursor.type_text("\n");
    assert_eq!(cursor.value(), "print(\"asdf function asdf\")\n");
}

/// 'Auto Indent On Type - Visual Basic', issue #118932.
mod visual_basic {
    use super::*;

    /// Typing `ch` at (line, column) of `lines`.
    fn type_char(lines: &[&str], (line, column): (usize, usize), ch: &str, expected: &[&str]) {
        let mut cursor = Cursor::new(visual_basic(), lines);
        cursor.move_to(line, column);
        cursor.type_text(ch);
        assert_eq!(cursor.editor.document.lines, expected);
    }

    /// Typing `text` without interceptors at (line, column) of `lines`, then Enter.
    fn enter_after(lines: &[&str], (line, column): (usize, usize), text: &str, expected: &[&str]) {
        let mut cursor = Cursor::new(visual_basic(), lines);
        cursor.move_to(line, column);
        cursor.type_plain(text);
        cursor.type_text("\n");
        assert_eq!(cursor.editor.document.lines, expected);
    }

    #[test]
    fn no_indentation_in_visual_basic_files() {
        type_char(&["If True Then", "    Some code", "    End I"], (3, 10), "f", &["If True Then", "    Some code", "End If"]);
    }

    #[test]
    fn indent_after_module_declaration() {
        enter_after(&[""], (1, 1), "Module Test", &["Module Test", "    "]);
    }

    #[test]
    fn indent_after_sub_declaration() {
        enter_after(&["Module Test", "    "], (2, 5), "Sub Main()", &["Module Test", "    Sub Main()", "        "]);
    }

    #[test]
    fn dedent_on_end_sub() {
        let lines = ["Module Test", "    Sub Main()", "        Console.WriteLine(\"Hello\")"];
        type_char(&[&lines[..], &["        End Su"]].concat(), (4, 15), "b", &[&lines[..], &["    End Sub"]].concat());
    }

    #[test]
    fn dedent_on_end_module() {
        type_char(&["Module Test", "    Private x As Integer", "    End Modul"], (3, 14), "e", &["Module Test", "    Private x As Integer", "End Module"]);
    }

    #[test]
    fn indent_after_function_declaration() {
        let function = "Function Add(a As Integer, b As Integer) As Integer";
        enter_after(&["Module Test", "    "], (2, 5), function, &["Module Test", &format!("    {function}"), "        "]);
    }

    #[test]
    fn dedent_on_end_function() {
        let lines = ["Module Test", "    Function Add(a, b)", "        Return a + b"];
        type_char(&[&lines[..], &["        End Functio"]].concat(), (4, 20), "n", &[&lines[..], &["    End Function"]].concat());
    }

    #[test]
    fn indent_after_if_then() {
        enter_after(&["Sub Test()", "    "], (2, 5), "If x > 0 Then", &["Sub Test()", "    If x > 0 Then", "        "]);
    }

    #[test]
    fn indent_after_else_if_then() {
        let lines = ["Sub Test()", "    If x > 0 Then", "        DoSomething()", "    ElseIf x < 0 Then"];
        enter_after(&lines, (4, 22), "", &[&lines[..], &["        "]].concat());
    }

    #[test]
    fn dedent_and_indent_on_else() {
        let lines = ["Sub Test()", "    If x > 0 Then", "        DoSomething()"];
        type_char(&[&lines[..], &["        Els"]].concat(), (4, 12), "e", &[&lines[..], &["    Else"]].concat());
    }

    #[test]
    fn indent_after_while() {
        enter_after(&["Sub Test()", "    "], (2, 5), "While x > 0", &["Sub Test()", "    While x > 0", "        "]);
    }

    #[test]
    fn dedent_on_end_while() {
        let lines = ["Sub Test()", "    While x > 0", "        x = x - 1"];
        type_char(&[&lines[..], &["        End Whil"]].concat(), (4, 17), "e", &[&lines[..], &["    End While"]].concat());
    }

    #[test]
    fn indent_after_for() {
        enter_after(&["Sub Test()", "    "], (2, 5), "For i = 1 To 10", &["Sub Test()", "    For i = 1 To 10", "        "]);
    }

    #[test]
    fn dedent_on_next() {
        let lines = ["Sub Test()", "    For i = 1 To 10", "        DoSomething(i)"];
        type_char(&[&lines[..], &["        Nex"]].concat(), (4, 12), "t", &[&lines[..], &["    Next"]].concat());
    }

    #[test]
    fn indent_after_do() {
        enter_after(&["Sub Test()", "    "], (2, 5), "Do", &["Sub Test()", "    Do", "        "]);
    }

    #[test]
    fn dedent_on_loop() {
        let lines = ["Sub Test()", "    Do", "        x = x + 1"];
        type_char(&[&lines[..], &["        Loo"]].concat(), (4, 12), "p", &[&lines[..], &["    Loop"]].concat());
    }

    #[test]
    fn indent_after_select_case() {
        enter_after(&["Sub Test()", "    "], (2, 5), "Select Case x", &["Sub Test()", "    Select Case x", "        "]);
    }

    #[test]
    fn dedent_on_end_select() {
        type_char(&["Sub Test()", "    Select Case x", "        End Selec"], (3, 18), "t", &["Sub Test()", "    Select Case x", "    End Select"]);
    }

    #[test]
    fn indent_after_try() {
        enter_after(&["Sub Test()", "    "], (2, 5), "Try", &["Sub Test()", "    Try", "        "]);
    }

    #[test]
    fn dedent_and_indent_on_catch() {
        let lines = ["Sub Test()", "    Try", "        DoSomething()"];
        type_char(&[&lines[..], &["        Catc"]].concat(), (4, 13), "h", &[&lines[..], &["    Catch"]].concat());
    }

    #[test]
    fn dedent_and_indent_on_finally() {
        let lines = ["Sub Test()", "    Try", "        DoSomething()", "    Catch", "        HandleError()"];
        type_char(&[&lines[..], &["        Finall"]].concat(), (6, 15), "y", &[&lines[..], &["    Finally"]].concat());
    }

    #[test]
    fn dedent_on_end_try() {
        let lines = ["Sub Test()", "    Try", "        DoSomething()", "    Catch", "        HandleError()", "    Finally", "        Cleanup()"];
        type_char(&[&lines[..], &["        End Tr"]].concat(), (8, 15), "y", &[&lines[..], &["    End Try"]].concat());
    }

    #[test]
    fn indent_after_class() {
        enter_after(&[""], (1, 1), "Class MyClass", &["Class MyClass", "    "]);
    }

    #[test]
    fn dedent_on_end_class() {
        type_char(&["Class MyClass", "    Private x As Integer", "    End Clas"], (3, 14), "s", &["Class MyClass", "    Private x As Integer", "End Class"]);
    }

    #[test]
    fn full_program_indentation_flow() {
        let mut cursor = Cursor::new(visual_basic(), &[""]);
        let steps = [
            ("Module Test", "\n", "Module Test\n    "),
            ("Sub Main()", "\n", "Module Test\n    Sub Main()\n        "),
            ("Console.WriteLine(\"Hello, World!\")", "\n", "Module Test\n    Sub Main()\n        Console.WriteLine(\"Hello, World!\")\n        "),
            ("End Su", "b", "Module Test\n    Sub Main()\n        Console.WriteLine(\"Hello, World!\")\n    End Sub"),
            ("", "\n", "Module Test\n    Sub Main()\n        Console.WriteLine(\"Hello, World!\")\n    End Sub\n    "),
        ];
        for (text, key, expected) in steps {
            cursor.type_plain(text);
            cursor.type_text(key);
            assert_eq!(cursor.value(), expected, "after {text:?} {key:?}");
        }
    }
}
