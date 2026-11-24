//! Port of the fuzzyScore cases in base/test/common/filters.test.ts. fuzzyScoreGraceful and anyScore
//! have no port yet, so their assertions are left out.

use super::*;

const OPTIONS: FuzzyScoreOptions = FuzzyScoreOptions { first_match_can_be_weak: false, boost_full_match: true };

fn assert_fuzzy_matches(pattern: &str, word: &str, decorated_word: Option<&str>, pattern_pos: usize, word_pos: usize, first_match_can_be_weak: bool) {
    let options = FuzzyScoreOptions { first_match_can_be_weak, boost_full_match: true };
    let result = fuzzy_score(pattern, &pattern.to_lowercase(), pattern_pos, word, &word.to_lowercase(), word_pos, options);
    assert_eq!(result.is_some(), decorated_word.is_some(), "{pattern:?} on {word:?}");
    let Some(decorated_word) = decorated_word else { return };
    let chars: Vec<char> = word.chars().collect();
    let mut actual_word = String::new();
    let mut pos = 0;
    for range in create_matches(result.as_ref()) {
        actual_word.extend(&chars[pos..range.start]);
        for ch in &chars[range.clone()] {
            actual_word.push('^');
            actual_word.push(*ch);
        }
        pos = range.end;
    }
    actual_word.extend(&chars[pos..]);
    assert_eq!(actual_word, decorated_word);
}

fn assert_top_score(pattern: &str, expected: usize, words: &[&str]) {
    let mut top_score = -(100 * 10);
    let mut top_index = 0;
    for (index, word) in words.iter().enumerate() {
        if let Some(found) = fuzzy_score(pattern, &pattern.to_lowercase(), 0, word, &word.to_lowercase(), 0, OPTIONS) {
            if found.score > top_score {
                top_score = found.score;
                top_index = index;
            }
        }
    }
    assert_eq!(top_index, expected, "{pattern} -> actual={} <> expected={}", words[top_index], words[expected]);
}

// fuzzyScore, #23215
#[test]
fn fuzzyscore_23215() {
    assert_fuzzy_matches("tit", "win.tit", Some("win.^t^i^t"), 0, 0, false);
    assert_fuzzy_matches("title", "win.title", Some("win.^t^i^t^l^e"), 0, 0, false);
    assert_fuzzy_matches("WordCla", "WordCharacterClassifier", Some("^W^o^r^dCharacter^C^l^assifier"), 0, 0, false);
    assert_fuzzy_matches("WordCCla", "WordCharacterClassifier", Some("^W^o^r^d^Character^C^l^assifier"), 0, 0, false);
}

// fuzzyScore, #23332
#[test]
fn fuzzyscore_23332() {
    assert_fuzzy_matches("dete", "\"editor.quickSuggestionsDelay\"", None, 0, 0, false);
}

// fuzzyScore, #23190
#[test]
fn fuzzyscore_23190() {
    assert_fuzzy_matches("c:\\do", "& 'C:\\Documents and Settings'", Some("& '^C^:^\\^D^ocuments and Settings'"), 0, 0, false);
    assert_fuzzy_matches("c:\\do", "& 'c:\\Documents and Settings'", Some("& '^c^:^\\^D^ocuments and Settings'"), 0, 0, false);
}

// fuzzyScore, #23581
#[test]
fn fuzzyscore_23581() {
    assert_fuzzy_matches("close", "css.lint.importStatement", Some("^css.^lint.imp^ort^Stat^ement"), 0, 0, false);
    assert_fuzzy_matches("close", "css.colorDecorators.enable", Some("^css.co^l^orDecorator^s.^enable"), 0, 0, false);
    assert_fuzzy_matches("close", "workbench.quickOpen.closeOnFocusOut", Some("workbench.quickOpen.^c^l^o^s^eOnFocusOut"), 0, 0, false);
    assert_top_score("close", 2, &["css.lint.importStatement", "css.colorDecorators.enable", "workbench.quickOpen.closeOnFocusOut"]);
}

// fuzzyScore, #23458
#[test]
fn fuzzyscore_23458() {
    assert_fuzzy_matches("highlight", "editorHoverHighlight", Some("editorHover^H^i^g^h^l^i^g^h^t"), 0, 0, false);
    assert_fuzzy_matches("hhighlight", "editorHoverHighlight", Some("editor^Hover^H^i^g^h^l^i^g^h^t"), 0, 0, false);
    assert_fuzzy_matches("dhhighlight", "editorHoverHighlight", None, 0, 0, false);
}

// fuzzyScore, #23746
#[test]
fn fuzzyscore_23746() {
    assert_fuzzy_matches("-moz", "-moz-foo", Some("^-^m^o^z-foo"), 0, 0, false);
    assert_fuzzy_matches("moz", "-moz-foo", Some("-^m^o^z-foo"), 0, 0, false);
    assert_fuzzy_matches("moz", "-moz-animation", Some("-^m^o^z-animation"), 0, 0, false);
    assert_fuzzy_matches("moza", "-moz-animation", Some("-^m^o^z-^animation"), 0, 0, false);
}

// fuzzyScore
#[test]
fn fuzzyscore() {
    assert_fuzzy_matches("ab", "abA", Some("^a^bA"), 0, 0, false);
    assert_fuzzy_matches("ccm", "cacmelCase", Some("^ca^c^melCase"), 0, 0, false);
    assert_fuzzy_matches("bti", "the_black_knight", None, 0, 0, false);
    assert_fuzzy_matches("ccm", "camelCase", None, 0, 0, false);
    assert_fuzzy_matches("cmcm", "camelCase", None, 0, 0, false);
    assert_fuzzy_matches("BK", "the_black_knight", Some("the_^black_^knight"), 0, 0, false);
    assert_fuzzy_matches("KeyboardLayout=", "KeyboardLayout", None, 0, 0, false);
    assert_fuzzy_matches("LLL", "SVisualLoggerLogsList", Some("SVisual^Logger^Logs^List"), 0, 0, false);
    assert_fuzzy_matches("LLLL", "SVilLoLosLi", None, 0, 0, false);
    assert_fuzzy_matches("LLLL", "SVisualLoggerLogsList", None, 0, 0, false);
    assert_fuzzy_matches("TEdit", "TextEdit", Some("^Text^E^d^i^t"), 0, 0, false);
    assert_fuzzy_matches("TEdit", "TextEditor", Some("^Text^E^d^i^tor"), 0, 0, false);
    assert_fuzzy_matches("TEdit", "Textedit", Some("^Text^e^d^i^t"), 0, 0, false);
    assert_fuzzy_matches("TEdit", "text_edit", Some("^text_^e^d^i^t"), 0, 0, false);
    assert_fuzzy_matches("TEditDit", "TextEditorDecorationType", Some("^Text^E^d^i^tor^Decorat^ion^Type"), 0, 0, false);
    assert_fuzzy_matches("TEdit", "TextEditorDecorationType", Some("^Text^E^d^i^torDecorationType"), 0, 0, false);
    assert_fuzzy_matches("Tedit", "TextEdit", Some("^Text^E^d^i^t"), 0, 0, false);
    assert_fuzzy_matches("ba", "?AB?", None, 0, 0, false);
    assert_fuzzy_matches("bkn", "the_black_knight", Some("the_^black_^k^night"), 0, 0, false);
    assert_fuzzy_matches("bt", "the_black_knight", Some("the_^black_knigh^t"), 0, 0, false);
    assert_fuzzy_matches("ccm", "camelCasecm", Some("^camel^Casec^m"), 0, 0, false);
    assert_fuzzy_matches("fdm", "findModel", Some("^fin^d^Model"), 0, 0, false);
    assert_fuzzy_matches("fob", "foobar", Some("^f^oo^bar"), 0, 0, false);
    assert_fuzzy_matches("fobz", "foobar", None, 0, 0, false);
    assert_fuzzy_matches("foobar", "foobar", Some("^f^o^o^b^a^r"), 0, 0, false);
    assert_fuzzy_matches("form", "editor.formatOnSave", Some("editor.^f^o^r^matOnSave"), 0, 0, false);
    assert_fuzzy_matches("g p", "Git: Pull", Some("^Git:^ ^Pull"), 0, 0, false);
    assert_fuzzy_matches("g p", "Git: Pull", Some("^Git:^ ^Pull"), 0, 0, false);
    assert_fuzzy_matches("gip", "Git: Pull", Some("^G^it: ^Pull"), 0, 0, false);
    assert_fuzzy_matches("gip", "Git: Pull", Some("^G^it: ^Pull"), 0, 0, false);
    assert_fuzzy_matches("gp", "Git: Pull", Some("^Git: ^Pull"), 0, 0, false);
    assert_fuzzy_matches("gp", "Git_Git_Pull", Some("^Git_Git_^Pull"), 0, 0, false);
    assert_fuzzy_matches("is", "ImportStatement", Some("^Import^Statement"), 0, 0, false);
    assert_fuzzy_matches("is", "isValid", Some("^i^sValid"), 0, 0, false);
    assert_fuzzy_matches("lowrd", "lowWord", Some("^l^o^wWo^r^d"), 0, 0, false);
    assert_fuzzy_matches("myvable", "myvariable", Some("^m^y^v^aria^b^l^e"), 0, 0, false);
    assert_fuzzy_matches("no", "", None, 0, 0, false);
    assert_fuzzy_matches("no", "match", None, 0, 0, false);
    assert_fuzzy_matches("ob", "foobar", None, 0, 0, false);
    assert_fuzzy_matches("sl", "SVisualLoggerLogsList", Some("^SVisual^LoggerLogsList"), 0, 0, false);
    assert_fuzzy_matches("sllll", "SVisualLoggerLogsList", Some("^SVisua^l^Logger^Logs^List"), 0, 0, false);
    assert_fuzzy_matches("Three", "HTMLHRElement", None, 0, 0, false);
    assert_fuzzy_matches("Three", "Three", Some("^T^h^r^e^e"), 0, 0, false);
    assert_fuzzy_matches("fo", "barfoo", None, 0, 0, false);
    assert_fuzzy_matches("fo", "bar_foo", Some("bar_^f^oo"), 0, 0, false);
    assert_fuzzy_matches("fo", "bar_Foo", Some("bar_^F^oo"), 0, 0, false);
    assert_fuzzy_matches("fo", "bar foo", Some("bar ^f^oo"), 0, 0, false);
    assert_fuzzy_matches("fo", "bar.foo", Some("bar.^f^oo"), 0, 0, false);
    assert_fuzzy_matches("fo", "bar/foo", Some("bar/^f^oo"), 0, 0, false);
    assert_fuzzy_matches("fo", "bar\\foo", Some("bar\\^f^oo"), 0, 0, false);
}

// fuzzyScore (first match can be weak)
#[test]
fn fuzzyscore_first_match_can_be_weak() {
    assert_fuzzy_matches("Three", "HTMLHRElement", Some("H^TML^H^R^El^ement"), 0, 0, true);
    assert_fuzzy_matches("tor", "constructor", Some("construc^t^o^r"), 0, 0, true);
    assert_fuzzy_matches("ur", "constructor", Some("constr^ucto^r"), 0, 0, true);
    assert_top_score("tor", 2, &["constructor", "Thor", "cTor"]);
}

// fuzzyScore, many matches
#[test]
fn fuzzyscore_many_matches() {
    assert_fuzzy_matches("aaaaaa", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", Some("^a^a^a^a^a^aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"), 0, 0, false);
}

// Freeze when fjfj -> jfjf, https://github.com/microsoft/vscode/issues/91807
#[test]
fn freeze_when_fjfj_jfjf_https_github_com_microsoft_vscode_issues_91807() {
    assert_fuzzy_matches("jfjfj", "fjfjfjfjfjfjfjfjfjfjfj", None, 0, 0, false);
    assert_fuzzy_matches("jfjfjfjfjfjfjfjfjfj", "fjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfj", None, 0, 0, false);
    assert_fuzzy_matches("jfjfjfjfjfjfjfjfjfjjfjfjfjfjfjfjfjfjfjjfjfjfjfjfjfjfjfjfjjfjfjfjfjfjfjfjfjfjjfjfjfjfjfjfjfjfjfjjfjfjfjfjfjfjfjfjfj", "fjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfj", None, 0, 0, false);
    assert_fuzzy_matches("jfjfjfjfjfjfjfjfjfj", "fJfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfj", Some("f^J^f^j^f^j^f^j^f^j^f^j^f^j^f^j^f^j^f^jfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfj"), 0, 0, false);
    assert_fuzzy_matches("jfjfjfjfjfjfjfjfjfj", "fjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfj", Some("f^j^f^j^f^j^f^j^f^j^f^j^f^j^f^j^f^j^f^jfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfjfj"), 0, 0, true);
}

// fuzzyScore, issue #26423
#[test]
fn fuzzyscore_issue_26423() {
    assert_fuzzy_matches("baba", "abababab", None, 0, 0, false);
    assert_fuzzy_matches("fsfsfs", "dsafdsafdsafdsafdsafdsafdsafasdfdsa", None, 0, 0, false);
    assert_fuzzy_matches("fsfsfsfsfsfsfsf", "dsafdsafdsafdsafdsafdsafdsafasdfdsafdsafdsafdsafdsfdsafdsfdfdfasdnfdsajfndsjnafjndsajlknfdsa", None, 0, 0, false);
}

// Fuzzy IntelliSense matching vs Haxe metadata completion, #26995
#[test]
fn fuzzy_intellisense_matching_vs_haxe_metadata_completion_26995() {
    assert_fuzzy_matches("f", ":Foo", Some(":^Foo"), 0, 0, false);
    assert_fuzzy_matches("f", ":foo", Some(":^foo"), 0, 0, false);
}

// Separator only match should not be weak #79558
#[test]
fn separator_only_match_should_not_be_weak_79558() {
    assert_fuzzy_matches(".", "foo.bar", Some("foo^.bar"), 0, 0, false);
}

// Cannot set property '1' of undefined, #26511
#[test]
fn cannot_set_property_1_of_undefined_26511() {
    let word = "a".repeat(122);
    let pattern = "a".repeat(119);
    fuzzy_score(&pattern, &pattern.to_lowercase(), 0, &word, &word.to_lowercase(), 0, OPTIONS); // must not explode
}

// Vscode 1.12 no longer obeys 'sortText' in completion items (from language server), #26096
#[test]
fn vscode_1_12_no_longer_obeys_sorttext_in_completion_items_from_language_server_26096() {
    assert_fuzzy_matches("  ", "  group", None, 2, 0, false);
    assert_fuzzy_matches("  g", "  group", Some("  ^group"), 2, 0, false);
    assert_fuzzy_matches("g", "  group", Some("  ^group"), 0, 0, false);
    assert_fuzzy_matches("g g", "  groupGroup", None, 0, 0, false);
    assert_fuzzy_matches("g g", "  group Group", Some("  ^group^ ^Group"), 0, 0, false);
    assert_fuzzy_matches(" g g", "  group Group", Some("  ^group^ ^Group"), 1, 0, false);
    assert_fuzzy_matches("zz", "zzGroup", Some("^z^zGroup"), 0, 0, false);
    assert_fuzzy_matches("zzg", "zzGroup", Some("^z^z^Group"), 0, 0, false);
    assert_fuzzy_matches("g", "zzGroup", Some("zz^Group"), 0, 0, false);
}

// patternPos isn't working correctly #79815
#[test]
fn patternpos_isn_t_working_correctly_79815() {
    assert_fuzzy_matches("p", "prop", Some("^prop"), 0, 0, false);
    assert_fuzzy_matches(":p", "prop", Some("^prop"), 1, 0, false);
    assert_fuzzy_matches(":p", "prop", None, 2, 0, false);
    assert_fuzzy_matches(":p", "proP", Some("pro^P"), 1, 1, false);
    assert_fuzzy_matches(":p", "aprop", Some("a^prop"), 1, 0, true);
    assert_fuzzy_matches(":p", "aprop", None, 1, 0, false);
}

// topScore - fuzzyScore
#[test]
fn topscore_fuzzyscore() {
    assert_top_score("cons", 2, &["ArrayBufferConstructor", "Console", "console"]);
    assert_top_score("Foo", 1, &["foo", "Foo", "foo"]);
    assert_top_score("onMess", 1, &["onmessage", "onMessage", "onThisMegaEscape"]);
    assert_top_score("CC", 1, &["camelCase", "CamelCase"]);
    assert_top_score("cC", 0, &["camelCase", "CamelCase"]);
    assert_top_score("p", 4, &["parse", "posix", "pafdsa", "path", "p"]);
    assert_top_score("pa", 0, &["parse", "pafdsa", "path"]);
    assert_top_score("log", 3, &["HTMLOptGroupElement", "ScrollLogicalPosition", "SVGFEMorphologyElement", "log", "logger"]);
    assert_top_score("e", 2, &["AbstractWorker", "ActiveXObject", "else"]);
    assert_top_score("workbench.sideb", 1, &["workbench.editor.defaultSideBySideLayout", "workbench.sideBar.location"]);
    assert_top_score("editor.r", 2, &["diffEditor.renderSideBySide", "editor.overviewRulerlanes", "editor.renderControlCharacter", "editor.renderWhitespace"]);
    assert_top_score("-mo", 1, &["-ms-ime-mode", "-moz-columns"]);
    assert_top_score("convertModelPosition", 0, &["convertModelPositionToViewPosition", "convertViewToModelPosition"]);
    assert_top_score("is", 0, &["isValidViewletId", "import statement"]);
    assert_top_score("title", 1, &["files.trimTrailingWhitespace", "window.title"]);
    assert_top_score("const", 1, &["constructor", "const", "cuOnstrul"]);
}

// Unexpected suggestion scoring, #28791
#[test]
fn unexpected_suggestion_scoring_28791() {
    assert_top_score("_lines", 1, &["_lineStarts", "_lines"]);
    assert_top_score("_lines", 1, &["_lineS", "_lines"]);
    assert_top_score("_lineS", 0, &["_lineS", "_lines"]);
}

// HTML closing tag proposal filtered out #38880
#[test]
fn html_closing_tag_proposal_filtered_out_38880() {
    assert_fuzzy_matches("\t\t<", "\t\t</body>", Some("^\t^\t^</body>"), 0, 0, false);
    assert_fuzzy_matches("\t\t<", "\t\t</body>", Some("\t\t^</body>"), 2, 0, false);
    assert_fuzzy_matches("\t<", "\t</body>", Some("\t^</body>"), 1, 0, false);
}

// fuzzyScoreGraceful
#[test]
fn fuzzyscoregraceful() {
    assert_fuzzy_matches("rlut", "result", None, 0, 0, false);
    assert_fuzzy_matches("cno", "console", Some("^co^ns^ole"), 0, 0, false);
}

// List highlight filter: Not all characters from match are highlighterd #66923
#[test]
fn list_highlight_filter_not_all_characters_from_match_are_highlighterd_66923() {
    assert_fuzzy_matches("foo", "barbarbarbarbarbarbarbarbarbarbarbarbarbarbarbar_foo", Some("barbarbarbarbarbarbarbarbarbarbarbarbarbarbarbar_^f^o^o"), 0, 0, false);
}

// Autocompletion is matched against truncated filterText to 54 characters #74133
#[test]
fn autocompletion_is_matched_against_truncated_filtertext_to_54_characters_74133() {
    assert_fuzzy_matches("foo", "ffffffffffffffffffffffffffffbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbar_foo", Some("ffffffffffffffffffffffffffffbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbar_^f^o^o"), 0, 0, false);
    assert_fuzzy_matches("Aoo", "Affffffffffffffffffffffffffffbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbar_foo", Some("^Affffffffffffffffffffffffffffbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbar_f^o^o"), 0, 0, false);
    assert_fuzzy_matches("foo", "Gffffffffffffffffffffffffffffbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbarbar_foo", None, 0, 0, false);
}

// "Go to Symbol" with the exact method name doesn't work as expected #84787
#[test]
fn go_to_symbol_with_the_exact_method_name_doesn_t_work_as_expected_84787() {
    let found = fuzzy_score(":get", ":get", 1, "get", "get", 0, FuzzyScoreOptions { first_match_can_be_weak: true, boost_full_match: true });
    assert!(found.is_some());
}

// Wrong highlight after emoji #113404
#[test]
fn wrong_highlight_after_emoji_113404() {
    assert_fuzzy_matches("di", "✨div classname=\"\"></div>", Some("✨^d^iv classname=\"\"></div>"), 0, 0, false);
    assert_fuzzy_matches("di", "adiv classname=\"\"></div>", Some("adiv classname=\"\"></^d^iv>"), 0, 0, false);
}

// Suggestion is not highlighted #85826
#[test]
fn suggestion_is_not_highlighted_85826() {
    assert_fuzzy_matches("SemanticTokens", "SemanticTokensEdits", Some("^S^e^m^a^n^t^i^c^T^o^k^e^n^sEdits"), 0, 0, false);
}

// IntelliSense completion not correctly highlighting text in front of cursor #115250
#[test]
fn intellisense_completion_not_correctly_highlighting_text_in_front_of_cursor_115250() {
    assert_fuzzy_matches("lo", "log", Some("^l^og"), 0, 0, false);
}

// configurable full match boost
#[test]
fn configurable_full_match_boost() {
    let prefix = "create";
    let a = "createModelServices";
    let b = "create";
    let boost = FuzzyScoreOptions { first_match_can_be_weak: true, boost_full_match: true };
    let score = |word: &str, word_start: usize, options| fuzzy_score(prefix, prefix, 0, word, &word.to_lowercase(), word_start, options).unwrap().score;

    assert!(score(a, 0, boost) < score(b, 0, boost));

    // also works with wordStart > 0 (https://github.com/microsoft/vscode/issues/187921)
    let word_prefix = "$(symbol-function) ";
    let start = word_prefix.chars().count();
    assert!(score(&format!("{word_prefix}{a}"), start, boost) < score(&format!("{word_prefix}{b}"), start, boost));

    let no_boost = FuzzyScoreOptions { first_match_can_be_weak: true, boost_full_match: false };
    assert_eq!(score(a, 0, no_boost), score(b, 0, no_boost));
}

// Unexpected suggest highlighting ignores whole word match in favor of matching first letter#147423
#[test]
fn unexpected_suggest_highlighting_ignores_whole_word_match_in_favor_of_matching_first_letter_147423() {
    assert_fuzzy_matches("i", "machine/{id}", Some("machine/{^id}"), 0, 0, false);
    assert_fuzzy_matches("ok", "obobobf{ok}/user", Some("^obobobf{o^k}/user"), 0, 0, false);
}

