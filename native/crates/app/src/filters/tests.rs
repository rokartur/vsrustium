//! Ported from src/vs/base/test/common/filters.test.ts and iconLabels.test.ts.

use super::*;

fn ok(result: Option<Matches>, expected: &[Range<usize>]) {
    assert_eq!(result.expect("matches"), expected);
}

#[test]
fn prefix_filter_ignore_case() {
    ok(matches_prefix("alpha", "alpha"), &[0..5]);
    ok(matches_prefix("alpha", "alphasomething"), &[0..5]);
    assert!(matches_prefix("alpha", "alp").is_none());
    ok(matches_prefix("a", "alpha"), &[0..1]);
    ok(matches_prefix("ä", "Älpha"), &[0..1]);
    assert!(matches_prefix("x", "alpha").is_none());
    ok(matches_prefix("A", "alpha"), &[0..1]);
    ok(matches_prefix("AlPh", "alPHA"), &[0..4]);
    assert!(matches_prefix("T", "4").is_none());
}

#[test]
fn camel_case_filter() {
    assert!(matches_camel_case("", "").is_none());
    ok(matches_camel_case("", "anything"), &[]);
    ok(matches_camel_case("alpha", "alpha"), &[0..5]);
    ok(matches_camel_case("AlPhA", "alpha"), &[0..5]);
    ok(matches_camel_case("alpha", "alphasomething"), &[0..5]);
    assert!(matches_camel_case("alpha", "alp").is_none());
    ok(matches_camel_case("c", "CamelCaseRocks"), &[0..1]);
    ok(matches_camel_case("cc", "CamelCaseRocks"), &[0..1, 5..6]);
    ok(matches_camel_case("ccr", "CamelCaseRocks"), &[0..1, 5..6, 9..10]);
    ok(matches_camel_case("cacr", "CamelCaseRocks"), &[0..2, 5..6, 9..10]);
    ok(matches_camel_case("cacar", "CamelCaseRocks"), &[0..2, 5..7, 9..10]);
    ok(matches_camel_case("ccarocks", "CamelCaseRocks"), &[0..1, 5..7, 9..14]);
    ok(matches_camel_case("cr", "CamelCaseRocks"), &[0..1, 9..10]);
    ok(matches_camel_case("fba", "FooBarAbe"), &[0..1, 3..5]);
    ok(matches_camel_case("fbar", "FooBarAbe"), &[0..1, 3..6]);
    ok(matches_camel_case("fbara", "FooBarAbe"), &[0..1, 3..7]);
    ok(matches_camel_case("fbaa", "FooBarAbe"), &[0..1, 3..5, 6..7]);
    ok(matches_camel_case("fbaab", "FooBarAbe"), &[0..1, 3..5, 6..8]);
    ok(matches_camel_case("c2d", "canvasCreation2D"), &[0..1, 14..16]);
    ok(matches_camel_case("cce", "_canvasCreationEvent"), &[1..2, 7..8, 15..16]);
}

#[test]
fn camel_case_filter_19256() {
    assert!(matches_camel_case("Debug Console", "Open: Debug Console").is_some());
    assert!(matches_camel_case("Debug console", "Open: Debug Console").is_some());
    assert!(matches_camel_case("debug console", "Open: Debug Console").is_some());
}

#[test]
fn contiguous_sub_string() {
    ok(matches_contiguous_sub_string("cela", "cancelAnimationFrame()"), &[3..7]);
}

#[test]
fn empty_queries_match_empty() {
    for label in ["", "Hello", " Hello", "$(zap)", "$(zap) ", "$(zap) Hello", "$(zap) $(zap) Hello"] {
        ok(matches_fuzzy_icon_aware("", &parse_label_with_icons(label)), &[0..0]);
    }
    ok(matches_fuzzy("", "first"), &[0..0]);
    ok(matches_contiguous_sub_string("", ""), &[0..0]);
}

#[test]
fn fuzzy_wildcards_match_greedily() {
    ok(matches_fuzzy("a*b", "xaxbxb"), &[1..6]);
    ok(matches_fuzzy("A.B", "xa.b"), &[1..4]);
    assert!(matches_fuzzy("a*c", "ab\nc").is_none());
}

#[test]
fn fuzzy_icon_aware() {
    ok(matches_fuzzy_icon_aware("ccr", &parse_label_with_icons("$(codicon)CamelCaseRocks$(codicon)")), &[10..11, 15..16, 19..20]);
    ok(matches_fuzzy_icon_aware("ccr", &parse_label_with_icons("$(codicon) CamelCaseRocks $(codicon)")), &[11..12, 16..17, 20..21]);
    ok(matches_fuzzy_icon_aware("iut", &parse_label_with_icons("$(codicon) Indent $(octico) Using $(octic) Tpaces")), &[11..12, 28..29, 43..44]);
    ok(matches_fuzzy_icon_aware("using", &parse_label_with_icons("$(codicon) Indent Using Spaces")), &[18..23]);
    ok(matches_fuzzy_icon_aware("codicon", &parse_label_with_icons("This $(codicon Indent Using Spaces")), &[7..14]);
    ok(matches_fuzzy_icon_aware("indent", &parse_label_with_icons("This $codicon Indent Using Spaces")), &[14..20]);
    ok(matches_fuzzy_icon_aware("unt", &parse_label_with_icons("$(primitive-dot) $(file-text) Untitled-1")), &[30..33]);
    ok(matches_fuzzy_icon_aware("s", &parse_label_with_icons("$(loading~spin) start")), &[16..17]);
}

#[test]
fn compare_anything_puts_prefix_then_suffix_matches_first() {
    let mut labels = vec!["xfoo", "foobar", "foo", "barfoo2", "bar"];
    labels.sort_by(|a, b| compare_anything(a, b, "foo"));
    assert_eq!(labels, ["foo", "foobar", "xfoo", "bar", "barfoo2"]);
    assert_eq!(compare_file_names("file2", "file10"), Ordering::Less);
}

#[test]
fn matches_base_contiguous_sub_string_filter() {
    ok(matches_base_contiguous_sub_string("cela", "cancelAnimationFrame()"), &[3..7]);
    ok(matches_base_contiguous_sub_string("cafe", "café"), &[0..4]);
    ok(matches_base_contiguous_sub_string("cafe", "caféBar"), &[0..4]);
    ok(matches_base_contiguous_sub_string("resume", "résumé"), &[0..6]);
    ok(matches_base_contiguous_sub_string("naïve", "naïve"), &[0..5]);
    ok(matches_base_contiguous_sub_string("naive", "naïve"), &[0..5]);
    ok(matches_base_contiguous_sub_string("aeou", "àéöü"), &[0..4]);
}

#[test]
fn word_filter() {
    ok(matches_words("alpha", "alpha"), &[0..5]);
    ok(matches_words("alpha", "alphasomething"), &[0..5]);
    assert!(matches_words("alpha", "alp").is_none());
    ok(matches_words("a", "alpha"), &[0..1]);
    assert!(matches_words("x", "alpha").is_none());
    ok(matches_words("A", "alpha"), &[0..1]);
    ok(matches_words("AlPh", "alPHA"), &[0..4]);
    assert!(matches_words("Debug Console", "Open: Debug Console").is_some());

    ok(matches_words("gp", "Git: Pull"), &[0..1, 5..6]);
    ok(matches_words("g p", "Git: Pull"), &[0..1, 5..6]);
    ok(matches_words("gipu", "Git: Pull"), &[0..2, 5..7]);

    ok(matches_words("gp", "Category: Git: Pull"), &[10..11, 15..16]);
    ok(matches_words("g p", "Category: Git: Pull"), &[10..11, 15..16]);
    ok(matches_words("gipu", "Category: Git: Pull"), &[10..12, 15..17]);

    assert!(matches_words("it", "Git: Pull").is_none());
    assert!(matches_words("ll", "Git: Pull").is_none());

    ok(matches_words("git: プル", "git: プル"), &[0..7]);
    ok(matches_words("git プル", "git: プル"), &[0..3, 5..7]);

    ok(matches_words("öäk", "Öhm: Älles Klar"), &[0..1, 5..6, 11..12]);

    ok(matches_words("C++", "C/C++: command"), &[2..5]);

    ok(matches_words(".", ":"), &[]);
    ok(matches_words(".", "."), &[0..1]);

    assert!(matches_words("bar", "foo-bar").is_some());
    assert!(matches_words("bar test", "foo-bar test").is_some());
    assert!(matches_words("fbt", "foo-bar test").is_some());
    assert!(matches_words("bar test", "foo-bar (test)").is_some());
    assert!(matches_words("foo bar", "foo (bar)").is_some());

    assert!(matches_words("bar est", "foo-bar test").is_none());
    assert!(matches_words("fo ar", "foo-bar test").is_none());
    assert!(matches_words("for", "foo-bar test").is_none());

    assert!(matches_words("foo bar", "foo-bar").is_some());
    assert!(matches_words("foo bar", "123 foo-bar 456").is_some());
    assert!(matches_words("foo-bar", "foo bar").is_some());
    assert!(matches_words("foo:bar", "foo:bar").is_some());
}

#[test]
fn matches_words_with_korean_keys() {
    ok(matches_words("랴ㅣㄷㄴ", "files"), &[0..5]);
    ok(matches_words("ㅎㅑㅅ", "Git: Pull"), &[0..3]);
    assert!(matches_words("ㅎㅑㅅ", "Pull").is_none());
}

#[test]
fn matches_words_performance_309582() {
    let targets = [
        "workbench.action.terminal.focusNextLine",
        "editor.action.clipboardCopyAction",
        "workbench.action.editor.changeLanguageMode",
        "editor.action.smartSelect.expand",
        "workbench.action.files.saveAll",
    ];
    let start = std::time::Instant::now();
    for _ in 0..1000 {
        for target in targets {
            matches_words("editor.action", target);
        }
    }
    // Mocha's default timeout.
    assert!(start.elapsed() < std::time::Duration::from_secs(2));
}

/// Ported from src/vs/base/test/common/naturalLanguage/korean.test.ts.
#[test]
fn korean_alt_chars() {
    let jamo = [
        ("ᄀ", "r"),
        ("ᄁ", "R"),
        ("ᄂ", "s"),
        ("ᄃ", "e"),
        ("ᄄ", "E"),
        ("ᄅ", "f"),
        ("ᄆ", "a"),
        ("ᄇ", "q"),
        ("ᄈ", "Q"),
        ("ᄉ", "t"),
        ("ᄊ", "T"),
        ("ᄋ", "d"),
        ("ᄌ", "w"),
        ("ᄍ", "W"),
        ("ᄎ", "c"),
        ("ᄏ", "z"),
        ("ᄐ", "x"),
        ("ᄑ", "v"),
        ("ᄒ", "g"),
        ("ᆨ", "r"),
        ("ᆩ", "R"),
        ("ᆪ", "rt"),
        ("ᆫ", "s"),
        ("ᆬ", "sw"),
        ("ᆭ", "sg"),
        ("ᆮ", "e"),
        ("ᆯ", "f"),
        ("ᆰ", "fr"),
        ("ᆱ", "fa"),
        ("ᆲ", "fq"),
        ("ᆳ", "ft"),
        ("ᆴ", "fx"),
        ("ᆵ", "fv"),
        ("ᆶ", "fg"),
        ("ᆷ", "a"),
        ("ᆸ", "q"),
        ("ᆹ", "qt"),
        ("ᆺ", "t"),
        ("ᆻ", "T"),
        ("ᆼ", "d"),
        ("ᆽ", "w"),
        ("ᆾ", "c"),
        ("ᆿ", "z"),
        ("ᇀ", "x"),
        ("ᇁ", "v"),
        ("ᇂ", "g"),
        ("ᅡ", "k"),
        ("ᅢ", "o"),
        ("ᅣ", "i"),
        ("ᅤ", "O"),
        ("ᅥ", "j"),
        ("ᅦ", "p"),
        ("ᅧ", "u"),
        ("ᅨ", "P"),
        ("ᅩ", "h"),
        ("ᅪ", "hk"),
        ("ᅫ", "ho"),
        ("ᅬ", "hl"),
        ("ᅭ", "y"),
        ("ᅮ", "n"),
        ("ᅯ", "nj"),
        ("ᅰ", "np"),
        ("ᅱ", "nl"),
        ("ᅲ", "b"),
        ("ᅳ", "m"),
        ("ᅴ", "ml"),
        ("ᅵ", "l"),
        ("ㄱ", "r"),
        ("ㄲ", "R"),
        ("ㄳ", "rt"),
        ("ㄴ", "s"),
        ("ㄵ", "sw"),
        ("ㄶ", "sg"),
        ("ㄷ", "e"),
        ("ㄸ", "E"),
        ("ㄹ", "f"),
        ("ㄺ", "fr"),
        ("ㄻ", "fa"),
        ("ㄼ", "fq"),
        ("ㄽ", "ft"),
        ("ㄾ", "fx"),
        ("ㄿ", "fv"),
        ("ㅀ", "fg"),
        ("ㅁ", "a"),
        ("ㅂ", "q"),
        ("ㅃ", "Q"),
        ("ㅄ", "qt"),
        ("ㅅ", "t"),
        ("ㅆ", "T"),
        ("ㅇ", "d"),
        ("ㅈ", "w"),
        ("ㅉ", "W"),
        ("ㅊ", "c"),
        ("ㅋ", "z"),
        ("ㅌ", "x"),
        ("ㅍ", "v"),
        ("ㅎ", "g"),
        ("ㅏ", "k"),
        ("ㅐ", "o"),
        ("ㅑ", "i"),
        ("ㅒ", "O"),
        ("ㅓ", "j"),
        ("ㅔ", "p"),
        ("ㅕ", "u"),
        ("ㅖ", "P"),
        ("ㅗ", "h"),
        ("ㅘ", "hk"),
        ("ㅙ", "ho"),
        ("ㅚ", "hl"),
        ("ㅛ", "y"),
        ("ㅜ", "n"),
        ("ㅝ", "nj"),
        ("ㅞ", "np"),
        ("ㅟ", "nl"),
        ("ㅠ", "b"),
        ("ㅡ", "m"),
        ("ㅢ", "ml"),
        ("ㅣ", "l"),
    ];
    for (hangul, alt) in jamo {
        assert_eq!(korean_keys(hangul), alt, "{hangul:?}");
    }
    // Compared in lower case: some upper case letters have no Hangul input.
    let composed = [
        ("ㅁㅊㅊㄷㄴ냐ㅠㅑㅣㅑ쇼", "accessibility"),
        ("ㅁㅊ채ㅕㅜㅅ뚜샤시드둣ㄴ", "accountEntitlements"),
        ("며야ㅐ쳗ㄴ", "audioCues"),
        ("ㅠㄱㅁ찯셰먁채ㅣㅐ걐ㄷㄱ2ㅆ디듣ㅅ교", "bracketPairColorizer2Telemetry"),
        ("ㅠㅕㅣㅏㄸ얏", "bulkEdit"),
        ("ㅊ미ㅣㅗㅑㄷㄱㅁㄱ초ㅛ", "callHierarchy"),
        ("촘ㅅ", "chat"),
        ("챙ㄷㅁㅊ샤ㅐㅜㄴ", "codeActions"),
        ("챙ㄷㄸ야색", "codeEditor"),
        ("채ㅡㅡ뭉ㄴ", "commands"),
        ("채ㅡㅡ둣ㄴ", "comments"),
        ("채ㅜ럏ㄸ테ㅐㄳㄷㄱ", "configExporter"),
        ("채ㅜㅅㄷㅌ스두ㅕ", "contextmenu"),
        ("쳔새ㅡㄸ야색", "customEditor"),
        ("ㅇ듀ㅕㅎ", "debug"),
        ("ㅇ덱ㄷㅊㅁㅅㄷㅇㄸㅌㅅ두냐ㅐㅜㅡㅑㅎㄱㅁ색", "deprecatedExtensionMigrator"),
        ("ㄷ얏ㄴㄷㄴ냐ㅐㅜㄴ", "editSessions"),
        ("드ㅡㄷㅅ", "emmet"),
        ("ㄷㅌㅅ두냐ㅐㅜㄴ", "extensions"),
        ("ㄷㅌㅅㄷ구밌ㄷ그ㅑㅜ미", "externalTerminal"),
        ("ㄷㅌㅅㄷ구미ㅕ갸ㅒㅔ둗ㄱ", "externalUriOpener"),
        ("랴ㅣㄷㄴ", "files"),
        ("래ㅣ야ㅜㅎ", "folding"),
        ("래금ㅅ", "format"),
        ("ㅑㅟ묘ㅗㅑㅜㅅㄴ", "inlayHints"),
        ("ㅑㅟㅑㅜㄷ촘ㅅ", "inlineChat"),
        ("ㅑㅜㅅㄷㄱㅁㅊ샾ㄷ", "interactive"),
        ("ㅑㄴ녇", "issue"),
        ("ㅏ됴ㅠㅑㅜ야ㅜㅎㄴ", "keybindings"),
        ("ㅣ무혐ㅎㄷㅇㄷㅅㄷㅊ샤ㅐㅜ", "languageDetection"),
        ("ㅣ무혐ㅎㄷㄴㅅㅁ션", "languageStatus"),
        ("ㅣㅑㅡㅑ샤ㅜ얓ㅁ색", "limitIndicator"),
        ("ㅣㅑㄴㅅ", "list"),
        ("ㅣㅐㅊ미ㅗㅑㄴ새교", "localHistory"),
        ("ㅣㅐㅊ미ㅑㅋㅁ샤ㅐㅜ", "localization"),
        ("ㅣㅐㅎㄴ", "logs"),
        ("ㅡ메ㅔㄷㅇㄸ얏ㄴ", "mappedEdits"),
        ("ㅡㅁ가애주", "markdown"),
        ("ㅡㅁ갇ㄱㄴ", "markers"),
        ("ㅡㄷㄱㅎㄷㄸ야색", "mergeEditor"),
        ("ㅡㅕㅣ샤얄ㄹㄸ야색", "multiDiffEditor"),
        ("ㅜㅐㅅ듀ㅐㅐㅏ", "notebook"),
        ("ㅐㅕ시ㅑㅜㄷ", "outline"),
        ("ㅐㅕ세ㅕㅅ", "output"),
        ("ㅔㄷㄱ래그뭋ㄷ", "performance"),
        ("ㅔㄱㄷㄹㄷㄱ둧ㄷㄴ", "preferences"),
        ("벼ㅑ참ㅊㅊㄷㄴㄴ", "quickaccess"),
        ("ㄱ디며ㅜ촏ㄱ", "relauncher"),
        ("ㄱ드ㅐㅅㄷ", "remote"),
        ("ㄱ드ㅐㅅㄷ쎠ㅜㅜ디", "remoteTunnel"),
        ("ㄴㅁ노", "sash"),
        ("ㄴ츠", "scm"),
        ("ㄴㄷㅁㄱ초", "search"),
        ("ㄴㄷㅁㄱ초ㄸ야색", "searchEditor"),
        ("놈ㄱㄷ", "share"),
        ("누ㅑㅔㅔㄷㅅㄴ", "snippets"),
        ("넫ㄷ초", "speech"),
        ("네ㅣㅁ노", "splash"),
        ("녁ㅍ됸", "surveys"),
        ("ㅅㅁㅎㄴ", "tags"),
        ("ㅅㅁ난", "tasks"),
        ("ㅅ디듣ㅅ교", "telemetry"),
        ("ㅅㄷ그ㅑㅜ미", "terminal"),
        ("ㅅㄷ그ㅑㅜ미채ㅜㅅ갸ㅠ", "terminalContrib"),
        ("ㅅㄷㄴ샤ㅜㅎ", "testing"),
        ("소듣ㄴ", "themes"),
        ("샤ㅡ디ㅑㅜㄷ", "timeline"),
        ("쇼ㅔ도ㅑㄷㄱㅁㄱ초ㅛ", "typeHierarchy"),
        ("ㅕㅔㅇㅁㅅㄷ", "update"),
        ("ㅕ기", "url"),
        ("ㅕㄴㄷㄱㅇㅁㅅ몌개랴ㅣㄷ", "userDataProfile"),
        ("ㅕㄴㄷㄱㅇㅁㅅㅁ뇨ㅜㅊ", "userDataSync"),
        ("ㅈ듀퍋ㅈ", "webview"),
        ("ㅈ듀퍋졔무디", "webviewPanel"),
        ("ㅈ듀퍋ㅈ퍋ㅈ", "webviewView"),
        ("ㅈ디채ㅡ듀무ㅜㄷㄱ", "welcomeBanner"),
        ("ㅈ디채ㅡㄷ야미ㅐㅎ", "welcomeDialog"),
        ("ㅈ디채ㅡㄷㅎㄷㅅ샤ㅜㅎㄴㅅㅁㄳㄷㅇ", "welcomeGettingStarted"),
        ("ㅈ디채ㅡㄷ퍋ㅈㄴ", "welcomeViews"),
        ("ㅈ디채ㅡㄷㅉ미ㅏ소개ㅕ호", "welcomeWalkthrough"),
        ("재가넴ㅊㄷ", "workspace"),
        ("재가넴ㅊㄷㄴ", "workspaces"),
    ];
    for (hangul, alt) in composed {
        assert_eq!(korean_keys(hangul).to_lowercase(), alt.to_lowercase(), "{hangul:?}");
    }
}

fn korean_keys(hangul: &str) -> String {
    hangul.chars().map(|c| korean::korean_alt_chars(c).unwrap_or_else(|| c.to_string())).collect()
}
