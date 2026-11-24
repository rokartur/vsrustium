//! Ported from platform/contextkey/test/common/{scanner,parser,contextkey}.test.ts.

use super::*;
use serde_json::json;

fn token_type(token: &Token) -> &'static str {
    match token.kind {
        TokenType::LParen => "(",
        TokenType::RParen => ")",
        TokenType::Neg => "!",
        TokenType::Eq => if token.is_triple_eq { "===" } else { "==" },
        TokenType::NotEq => if token.is_triple_eq { "!==" } else { "!=" },
        TokenType::Lt => "<",
        TokenType::LtEq => "<=",
        TokenType::Gt => ">",
        TokenType::GtEq => ">=",
        TokenType::RegexOp => "=~",
        TokenType::RegexStr => "RegexStr",
        TokenType::True => "true",
        TokenType::False => "false",
        TokenType::In => "in",
        TokenType::Not => "not",
        TokenType::And => "&&",
        TokenType::Or => "||",
        TokenType::Str => "Str",
        TokenType::QuotedStr => "QuotedStr",
        TokenType::Error => "ErrorToken",
        TokenType::Eof => "EOF",
    }
}

fn scan(input: &str) -> Vec<(&'static str, usize, Option<String>)> {
    let (tokens, _) = Scanner::scan(input);
    tokens
        .iter()
        .map(|token| {
            let has_lexeme = matches!(token.kind, TokenType::RegexStr | TokenType::Str | TokenType::QuotedStr | TokenType::Error);
            (token_type(token), token.offset, has_lexeme.then(|| token.lexeme.clone()))
        })
        .collect()
}

#[test]
fn scanner() {
    let cases: Vec<(&str, Vec<(&str, usize, Option<&str>)>)> = vec![
        ("foo.bar<C-shift+2>", vec![("Str", 0, Some("foo.bar<C-shift+2>")), ("EOF", 18, None)]),
        ("!foo", vec![("!", 0, None), ("Str", 1, Some("foo")), ("EOF", 4, None)]),
        ("foo === bar", vec![("Str", 0, Some("foo")), ("===", 4, None), ("Str", 8, Some("bar")), ("EOF", 11, None)]),
        ("foo  !== bar", vec![("Str", 0, Some("foo")), ("!==", 5, None), ("Str", 9, Some("bar")), ("EOF", 12, None)]),
        ("!(foo && bar)", vec![("!", 0, None), ("(", 1, None), ("Str", 2, Some("foo")), ("&&", 6, None), ("Str", 9, Some("bar")), (")", 12, None), ("EOF", 13, None)]),
        ("=~ ", vec![("=~", 0, None), ("EOF", 3, None)]),
        ("foo =~ /bar/", vec![("Str", 0, Some("foo")), ("=~", 4, None), ("RegexStr", 7, Some("/bar/")), ("EOF", 12, None)]),
        ("foo =~ /zee/i", vec![("Str", 0, Some("foo")), ("=~", 4, None), ("RegexStr", 7, Some("/zee/i")), ("EOF", 13, None)]),
        ("foo =~ /zee/gm", vec![("Str", 0, Some("foo")), ("=~", 4, None), ("RegexStr", 7, Some("/zee/gm")), ("EOF", 14, None)]),
        ("foo in barrr  ", vec![("Str", 0, Some("foo")), ("in", 4, None), ("Str", 7, Some("barrr")), ("EOF", 14, None)]),
        ("resource =~ //FileCabinet/(SuiteScripts|Templates/(E-mail%20Templates|Marketing%20Templates)|Web%20Site%20Hosting%20Files)(/.*)*$/", vec![("Str", 0, Some("resource")), ("=~", 9, None), ("RegexStr", 12, Some("//")), ("Str", 14, Some("FileCabinet/")), ("(", 26, None), ("Str", 27, Some("SuiteScripts")), ("ErrorToken", 39, Some("|")), ("Str", 40, Some("Templates/")), ("(", 50, None), ("Str", 51, Some("E-mail%20Templates")), ("ErrorToken", 69, Some("|")), ("Str", 70, Some("Marketing%20Templates")), (")", 91, None), ("ErrorToken", 92, Some("|")), ("Str", 93, Some("Web%20Site%20Hosting%20Files")), (")", 121, None), ("(", 122, None), ("RegexStr", 123, Some("/.*)*$/")), ("EOF", 130, None)]),
        ("editorLangId in testely.supportedLangIds && resourceFilename =~ /^.+(.test.(w+))$/gm", vec![("Str", 0, Some("editorLangId")), ("in", 13, None), ("Str", 16, Some("testely.supportedLangIds")), ("&&", 41, None), ("Str", 44, Some("resourceFilename")), ("=~", 61, None), ("RegexStr", 64, Some("/^.+(.test.(w+))$/gm")), ("EOF", 84, None)]),
        ("!(foo && bar) && baz", vec![("!", 0, None), ("(", 1, None), ("Str", 2, Some("foo")), ("&&", 6, None), ("Str", 9, Some("bar")), (")", 12, None), ("&&", 14, None), ("Str", 17, Some("baz")), ("EOF", 20, None)]),
        ("foo.bar:zed==completed", vec![("Str", 0, Some("foo.bar:zed")), ("==", 11, None), ("Str", 13, Some("completed")), ("EOF", 22, None)]),
        ("a && b || c", vec![("Str", 0, Some("a")), ("&&", 2, None), ("Str", 5, Some("b")), ("||", 7, None), ("Str", 10, Some("c")), ("EOF", 11, None)]),
        ("fooBar && baz.jar && fee.bee<K-loo+1>", vec![("Str", 0, Some("fooBar")), ("&&", 7, None), ("Str", 10, Some("baz.jar")), ("&&", 18, None), ("Str", 21, Some("fee.bee<K-loo+1>")), ("EOF", 37, None)]),
        ("foo.barBaz<C-r> < 2", vec![("Str", 0, Some("foo.barBaz<C-r>")), ("<", 16, None), ("Str", 18, Some("2")), ("EOF", 19, None)]),
        ("foo.bar >= -1", vec![("Str", 0, Some("foo.bar")), (">=", 8, None), ("Str", 11, Some("-1")), ("EOF", 13, None)]),
        ("foo.bar <= -1", vec![("Str", 0, Some("foo.bar")), ("<=", 8, None), ("Str", 11, Some("-1")), ("EOF", 13, None)]),
        ("resource =~ /\\/Objects\\/.+\\.xml$/", vec![("Str", 0, Some("resource")), ("=~", 9, None), ("RegexStr", 12, Some("/\\/Objects\\/.+\\.xml$/")), ("EOF", 33, None)]),
        ("view == vsc-packages-activitybar-folders && vsc-packages-folders-loaded", vec![("Str", 0, Some("view")), ("==", 5, None), ("Str", 8, Some("vsc-packages-activitybar-folders")), ("&&", 41, None), ("Str", 44, Some("vsc-packages-folders-loaded")), ("EOF", 71, None)]),
        ("sfdx:project_opened && resource =~ /.*\\/functions\\/.*\\/[^\\/]+(\\/[^\\/]+.(ts|js|java|json|toml))?$/ && resourceFilename != package.json && resourceFilename != package-lock.json && resourceFilename != tsconfig.json", vec![("Str", 0, Some("sfdx:project_opened")), ("&&", 20, None), ("Str", 23, Some("resource")), ("=~", 32, None), ("RegexStr", 35, Some("/.*\\/functions\\/.*\\/[^\\/]+(\\/[^\\/]+.(ts|js|java|json|toml))?$/")), ("&&", 98, None), ("Str", 101, Some("resourceFilename")), ("!=", 118, None), ("Str", 121, Some("package.json")), ("&&", 134, None), ("Str", 137, Some("resourceFilename")), ("!=", 154, None), ("Str", 157, Some("package-lock.json")), ("&&", 175, None), ("Str", 178, Some("resourceFilename")), ("!=", 195, None), ("Str", 198, Some("tsconfig.json")), ("EOF", 211, None)]),
        ("view =~ '/(servers)/' && viewItem =~ '/^(Starting|Started|Debugging|Stopping|Stopped)/'", vec![("Str", 0, Some("view")), ("=~", 5, None), ("QuotedStr", 9, Some("/(servers)/")), ("&&", 22, None), ("Str", 25, Some("viewItem")), ("=~", 34, None), ("QuotedStr", 38, Some("/^(Starting|Started|Debugging|Stopping|Stopped)/")), ("EOF", 87, None)]),
        ("resourcePath =~ /.md(.yml|.txt)*$/gim", vec![("Str", 0, Some("resourcePath")), ("=~", 13, None), ("RegexStr", 16, Some("/.md(.yml|.txt)*$/gim")), ("EOF", 37, None)]),
        ("foo === bar'", vec![("Str", 0, Some("foo")), ("===", 4, None), ("Str", 8, Some("bar")), ("ErrorToken", 11, Some("'")), ("EOF", 12, None)]),
        ("foo === '", vec![("Str", 0, Some("foo")), ("===", 4, None), ("ErrorToken", 8, Some("'")), ("EOF", 9, None)]),
        ("foo && 'bar", vec![("Str", 0, Some("foo")), ("&&", 4, None), ("ErrorToken", 7, Some("'bar")), ("EOF", 11, None)]),
        ("vim<c-r> == 1 && vim<2 <= 3", vec![("Str", 0, Some("vim<c-r>")), ("==", 9, None), ("Str", 12, Some("1")), ("&&", 14, None), ("Str", 17, Some("vim<2")), ("<=", 23, None), ("Str", 26, Some("3")), ("EOF", 27, None)]),
        ("vim<c-r>==1 && vim<2<=3", vec![("Str", 0, Some("vim<c-r>")), ("==", 8, None), ("Str", 10, Some("1")), ("&&", 12, None), ("Str", 15, Some("vim<2<")), ("ErrorToken", 21, Some("=")), ("Str", 22, Some("3")), ("EOF", 23, None)]),
        ("foo|bar", vec![("Str", 0, Some("foo")), ("ErrorToken", 3, Some("|")), ("Str", 4, Some("bar")), ("EOF", 7, None)]),
        ("resource =~ //foo/(barr|door/(Foo-Bar%20Templates|Soo%20Looo)|Web%20Site%Jjj%20Llll)(/.*)*$/", vec![("Str", 0, Some("resource")), ("=~", 9, None), ("RegexStr", 12, Some("//")), ("Str", 14, Some("foo/")), ("(", 18, None), ("Str", 19, Some("barr")), ("ErrorToken", 23, Some("|")), ("Str", 24, Some("door/")), ("(", 29, None), ("Str", 30, Some("Foo-Bar%20Templates")), ("ErrorToken", 49, Some("|")), ("Str", 50, Some("Soo%20Looo")), (")", 60, None), ("ErrorToken", 61, Some("|")), ("Str", 62, Some("Web%20Site%Jjj%20Llll")), (")", 83, None), ("(", 84, None), ("RegexStr", 85, Some("/.*)*$/")), ("EOF", 92, None)]),
        ("/((/foo/(?!bar)(.*)/)|((/src/).*/)).*$/", vec![("RegexStr", 0, Some("/((/")), ("Str", 4, Some("foo/")), ("(", 8, None), ("Str", 9, Some("?")), ("!", 10, None), ("Str", 11, Some("bar")), (")", 14, None), ("(", 15, None), ("Str", 16, Some(".*")), (")", 18, None), ("RegexStr", 19, Some("/)|((/s")), ("Str", 26, Some("rc/")), (")", 29, None), ("Str", 30, Some(".*/")), (")", 33, None), (")", 34, None), ("Str", 35, Some(".*$/")), ("EOF", 39, None)]),
        ("resourcePath =~ //foo/barr// || resourcePath =~ //view/(jarrr|doooor|bees)/(web|templates)// && resourceExtname in foo.Bar", vec![("Str", 0, Some("resourcePath")), ("=~", 13, None), ("RegexStr", 16, Some("//")), ("Str", 18, Some("foo/barr//")), ("||", 29, None), ("Str", 32, Some("resourcePath")), ("=~", 45, None), ("RegexStr", 48, Some("//")), ("Str", 50, Some("view/")), ("(", 55, None), ("Str", 56, Some("jarrr")), ("ErrorToken", 61, Some("|")), ("Str", 62, Some("doooor")), ("ErrorToken", 68, Some("|")), ("Str", 69, Some("bees")), (")", 73, None), ("RegexStr", 74, Some("/(web|templates)/")), ("ErrorToken", 91, Some("/ && resourceExtname in foo.Bar")), ("EOF", 122, None)]),
        ("foo =~ /file:// || bar", vec![("Str", 0, Some("foo")), ("=~", 4, None), ("RegexStr", 7, Some("/file:/")), ("ErrorToken", 14, Some("/ || bar")), ("EOF", 22, None)]),
    ];
    for (input, expected) in cases {
        let expected: Vec<_> = expected.into_iter().map(|(kind, offset, lexeme)| (kind, offset, lexeme.map(String::from))).collect();
        assert_eq!(scan(input), expected, "{input}");
    }
}

fn parse_to_str(input: &str) -> String {
    let mut parser = Parser::new(true);
    let Some(expr) = parser.parse(input) else {
        let mut prints = String::new();
        if !parser.lexing_errors.is_empty() {
            prints.push_str("Lexing errors:\n\n");
            for error in &parser.lexing_errors {
                let info = error.additional_info.as_deref().unwrap_or("undefined");
                prints.push_str(&format!("Unexpected token '{}' at offset {}. {info}\n", error.lexeme, error.offset));
            }
        }
        if !parser.parsing_errors.is_empty() {
            if !parser.lexing_errors.is_empty() {
                prints.push_str("\n --- \n");
            }
            prints.push_str("Parsing errors:\n\n");
            for error in &parser.parsing_errors {
                prints.push_str(&format!("Unexpected '{}' at offset {}.\n", error.lexeme, error.offset));
            }
        }
        return prints;
    };
    expr.serialize()
}

#[test]
fn parser() {
    let cases = [
        (" foo", "foo"),
        ("!foo", "!foo"),
        ("foo =~ /bar/", "foo =~ /bar/"),
        ("foo || (foo =~ /bar/ && baz)", "foo || baz && foo =~ /bar/"),
        ("foo || (foo =~ /bar/ || baz)", "baz || foo || foo =~ /bar/"),
        ("(foo || bar) && (jee || jar)", "bar && jar || bar && jee || foo && jar || foo && jee"),
        ("foo && foo =~ /zee/i", "foo && foo =~ /zee/i"),
        ("foo.bar==enabled", "foo.bar == 'enabled'"),
        ("foo.bar == 'enabled'", "foo.bar == 'enabled'"),
        ("foo.bar:zed==completed", "foo.bar:zed == 'completed'"),
        ("a && b || c", "c || a && b"),
        ("fooBar && baz.jar && fee.bee<K-loo+1>", "baz.jar && fee.bee<K-loo+1> && fooBar"),
        ("foo.barBaz<C-r> < 2", "foo.barBaz<C-r> < 2"),
        ("foo.bar >= -1", "foo.bar >= -1"),
        ("view == vsc-packages-activitybar-folders && vsc-packages-folders-loaded", "vsc-packages-folders-loaded && view == 'vsc-packages-activitybar-folders'"),
        ("foo.bar <= -1", "foo.bar <= -1"),
        ("!cmake:hideBuildCommand && cmake:enableFullFeatureSet", "cmake:enableFullFeatureSet && !cmake:hideBuildCommand"),
        ("!(foo && bar)", "!bar || !foo"),
        ("!(foo && bar || boar) || deer", "deer || !bar && !boar || !boar && !foo"),
        ("!(!foo)", "foo"),
        ("debugState == \"stopped\"", "debugState == '\"stopped\"'"),
        (" viewItem == VSCode WorkSpace", "Parsing errors:\n\nUnexpected 'WorkSpace' at offset 20.\n"),
        ("resource =~ //foo/(barr|door/(Foo-Bar%20Templates|Soo%20Looo)|Web%20Site%Jjj%20Llll)(/.*)*$/", "resource =~ /\\/foo\\/(barr|door\\/(Foo-Bar%20Templates|Soo%20Looo)|Web%20Site%Jjj%20Llll)(\\/.*)*$/"),
        ("resource =~ /((/scratch/(?!update)(.*)/)|((/src/).*/)).*$/", "resource =~ /((\\/scratch\\/(?!update)(.*)\\/)|((\\/src\\/).*\\/)).*$/"),
        ("resourcePath =~ /.md(.yml|.txt)*$/giym", "resourcePath =~ /.md(.yml|.txt)*$/im"),
        ("/foo", "Lexing errors:\n\nUnexpected token '/foo' at offset 0. Did you forget to escape the '/' (slash) character? Put two backslashes before it to escape, e.g., '\\\\/'.\n\n --- \nParsing errors:\n\nUnexpected '/foo' at offset 0.\n"),
        ("!b == 'true'", "Parsing errors:\n\nUnexpected '==' at offset 3.\n"),
        ("!foo &&  in bar", "Parsing errors:\n\nUnexpected 'in' at offset 9.\n"),
        ("vim<c-r> == 1 && vim<2<=3", "Lexing errors:\n\nUnexpected token '=' at offset 23. Did you mean == or =~?\n\n --- \nParsing errors:\n\nUnexpected '=' at offset 23.\n"),
        ("foo && 'bar", "Lexing errors:\n\nUnexpected token ''bar' at offset 7. Did you forget to open or close the quote?\n\n --- \nParsing errors:\n\nUnexpected ''bar' at offset 7.\n"),
        ("config.foo &&  &&bar =~ /^foo$|^bar-foo$|^joo$|^jar$/ && !foo", "Parsing errors:\n\nUnexpected '&&' at offset 15.\n"),
        ("!foo == 'test'", "Parsing errors:\n\nUnexpected '==' at offset 5.\n"),
        ("!!foo", "Parsing errors:\n\nUnexpected '!' at offset 1.\n"),
    ];
    for (input, expected) in cases {
        assert_eq!(parse_to_str(input), expected, "{input}");
    }
}

fn has(key: &str) -> Option<Expr> {
    Some(Expr::has(key))
}

fn not(key: &str) -> Option<Expr> {
    Some(Expr::not(key))
}

fn regex(key: &str, pattern: &str) -> Option<Expr> {
    Some(Expr::regex(key, JsRegex::new(pattern, "").unwrap()))
}

fn deserialize(input: &str) -> Expr {
    Expr::deserialize(input).unwrap_or_else(|| panic!("{input} parses"))
}

fn context(value: Value) -> HashMap<String, Value> {
    serde_json::from_value(value).unwrap()
}

#[test]
fn expr_equals() {
    let a = Expr::and(vec![
        has("a1"),
        Expr::and(vec![has("and.a")]),
        has("a2"),
        regex("d3", "d.*"),
        regex("d4", r"\*\*3*"),
        Some(Expr::key_equals("b1", json!("bb1"))),
        Some(Expr::key_equals("b2", json!("bb2"))),
        Some(Expr::key_not_equals("c1", json!("cc1"))),
        Some(Expr::key_not_equals("c2", json!("cc2"))),
        not("d1"),
        not("d2"),
    ])
    .unwrap();
    let b = Expr::and(vec![
        Some(Expr::key_equals("b2", json!("bb2"))),
        Some(Expr::key_not_equals("c1", json!("cc1"))),
        not("d1"),
        regex("d4", r"\*\*3*"),
        Some(Expr::key_not_equals("c2", json!("cc2"))),
        has("a2"),
        Some(Expr::key_equals("b1", json!("bb1"))),
        regex("d3", "d.*"),
        has("a1"),
        Expr::and(vec![Some(Expr::key_equals("and.a", json!(true)))]),
        not("d2"),
    ])
    .unwrap();
    assert!(a.equals(&b), "expressions should be equal");
}

#[test]
fn issue_134942_equals_in_comparator_expressions() {
    let cases = [
        (Expr::greater("value", json!(0)), "value > 0"),
        (Expr::greater_equals("value", json!(0)), "value >= 0"),
        (Expr::smaller("value", json!(0)), "value < 0"),
        (Expr::smaller_equals("value", json!(0)), "value <= 0"),
    ];
    for (expr, input) in cases {
        assert!(expr.equals(&deserialize(input)), "{input}");
    }
}

#[test]
fn normalize() {
    assert!(Expr::key_equals("key1", json!(true)).equals(&Expr::has("key1")));
    assert!(Expr::key_not_equals("key1", json!(false)).equals(&Expr::has("key1")));
    assert!(Expr::key_equals("key1", json!(false)).equals(&Expr::not("key1")));
    assert!(Expr::key_not_equals("key1", json!(true)).equals(&Expr::not("key1")));
}

#[test]
fn evaluate() {
    let context = context(json!({ "a": true, "b": false, "c": "5", "d": "d" }));
    let test_expression = |expr: &str, expected: bool| assert_eq!(deserialize(expr).evaluate(&context), expected, "{expr}");
    let d_star = JsRegex::new("d.*", "").unwrap();
    let d_ignore_case = JsRegex::new("D", "i").unwrap();
    let test_batch = |expr: &str, value: Option<Value>| {
        let value = value.as_ref();
        let truthy = super::truthy(value);
        test_expression(expr, truthy);
        test_expression(&format!("{expr} == true"), truthy);
        test_expression(&format!("{expr} != true"), !truthy);
        test_expression(&format!("{expr} == false"), !truthy);
        test_expression(&format!("{expr} != false"), truthy);
        test_expression(&format!("{expr} == 5"), loose_equals(value, &json!("5")));
        test_expression(&format!("{expr} != 5"), !loose_equals(value, &json!("5")));
        test_expression(&format!("!{expr}"), !truthy);
        test_expression(&format!("{expr} =~ /d.*/"), d_star.test(value));
        test_expression(&format!("{expr} =~ /D/i"), d_ignore_case.test(value));
    };
    test_batch("a", Some(json!(true)));
    test_batch("b", Some(json!(false)));
    test_batch("c", Some(json!("5")));
    test_batch("d", Some(json!("d")));
    test_batch("z", None);

    test_expression("true", true);
    test_expression("false", false);
    test_expression("a && !b", true);
    test_expression("a && b", false);
    test_expression("a && !b && c == 5", true);
    test_expression("d =~ /e.*/", false);
    // false && true || true === true because && is evaluated first
    test_expression("b && a || a", true);
    test_expression("a || b", true);
    test_expression("b || b", false);
    test_expression("b && a || a && b", false);
}

#[test]
fn evaluate_js_values_the_batch_relies_on() {
    // Pins the JavaScript results evaluate() compares against, so a wrong helper cannot pass both sides.
    assert!(!loose_equals(Some(&json!(true)), &json!("5")));
    assert!(loose_equals(Some(&json!(5)), &json!("5")));
    assert!(!loose_equals(None, &json!("5")));
    assert!(JsRegex::new("d.*", "").unwrap().test(None), "/d.*/.test(undefined) matches 'undefined'");
    assert!(!JsRegex::new("d.*", "").unwrap().test(Some(&json!(true))));
}

#[test]
fn negate() {
    let cases = [
        ("true", "false"),
        ("false", "true"),
        ("a", "!a"),
        ("a && b || c", "!a && !c || !b && !c"),
        ("a && b || c || d", "!a && !c && !d || !b && !c && !d"),
        ("!a && !b || !c && !d", "a && c || a && d || b && c || b && d"),
        ("!a && !b || !c && !d || !e && !f", "a && c && e || a && c && f || a && d && e || a && d && f || b && c && e || b && c && f || b && d && e || b && d && f"),
    ];
    for (expr, expected) in cases {
        assert_eq!(deserialize(expr).negate().serialize(), expected, "{expr}");
    }
}

#[test]
fn false_true() {
    let cases = [
        ("true", "true"),
        ("!true", "false"),
        ("false", "false"),
        ("!false", "true"),
        ("a && true", "a"),
        ("a && false", "false"),
        ("a || true", "true"),
        ("a || false", "a"),
        ("isMac", "true"),
        ("isLinux", "false"),
        ("isWindows", "false"),
        ("isChromeOS", "false"),
    ];
    for (expr, expected) in cases {
        assert_eq!(deserialize(expr).serialize(), expected, "{expr}");
    }
}

#[test]
fn issue_101015_distribute_or() {
    let t = |expr1: &str, expr2: &str, expected: &str| {
        let actual = Expr::and(vec![Expr::deserialize(expr1), Expr::deserialize(expr2)]).map(|expr| expr.serialize());
        assert_eq!(actual.as_deref(), Some(expected), "{expr1} and {expr2}");
    };
    t("a", "b", "a && b");
    t("a || b", "c", "a && c || b && c");
    t("a || b", "c || d", "a && c || a && d || b && c || b && d");
    t("a || b", "c && d", "a && c && d || b && c && d");
    t("a || b", "c && d || e", "a && e || b && e || a && c && d || b && c && d");
}

#[test]
fn context_key_in_expr() {
    let a_in_b = deserialize("a in b");
    let cases = [
        (json!({ "a": 3, "b": [3, 2, 1] }), true),
        (json!({ "a": 3, "b": [1, 2, 3] }), true),
        (json!({ "a": 3, "b": [1, 2] }), false),
        (json!({ "a": 3 }), false),
        (json!({ "a": 3, "b": null }), false),
        (json!({ "a": "x", "b": ["x"] }), true),
        (json!({ "a": "x", "b": ["y"] }), false),
        (json!({ "a": "x", "b": {} }), false),
        (json!({ "a": "x", "b": { "x": false } }), true),
        (json!({ "a": "x", "b": { "x": true } }), true),
        (json!({ "a": "prototype", "b": {} }), false),
    ];
    for (values, expected) in cases {
        assert_eq!(a_in_b.evaluate(&context(values.clone())), expected, "{values}");
    }
}

#[test]
fn context_key_not_in_expr() {
    let a_not_in_b = deserialize("a not in b");
    let cases = [
        (json!({ "a": 3, "b": [3, 2, 1] }), false),
        (json!({ "a": 3, "b": [1, 2, 3] }), false),
        (json!({ "a": 3, "b": [1, 2] }), true),
        (json!({ "a": 3 }), true),
        (json!({ "a": 3, "b": null }), true),
        (json!({ "a": "x", "b": ["x"] }), false),
        (json!({ "a": "x", "b": ["y"] }), true),
        (json!({ "a": "x", "b": {} }), true),
        (json!({ "a": "x", "b": { "x": false } }), false),
        (json!({ "a": "x", "b": { "x": true } }), false),
        (json!({ "a": "prototype", "b": {} }), true),
    ];
    for (values, expected) in cases {
        assert_eq!(a_not_in_b.evaluate(&context(values.clone())), expected, "{values}");
    }
}

#[test]
fn issue_338834_distributing_an_or_that_collapses_to_a_single_and_produces_a_flat_and() {
    let actual = Expr::and(vec![has("a"), has("b"), Expr::or(vec![not("b"), Expr::and(vec![has("a"), has("c")])])]).unwrap();
    assert_eq!(actual.serialize(), "a && b && c");
    assert!(actual.equals(&Expr::and(vec![has("a"), has("b"), has("c")]).unwrap()));
    assert!(actual.equals(&deserialize(&actual.serialize())));
}

#[test]
fn distributing_and_over_or_is_not_affected_by_in_and_comparison_terms() {
    let a_or_b = || Expr::or(vec![has("a"), has("b")]);
    assert_eq!(Expr::and(vec![a_or_b(), Some(Expr::greater("c", json!(1)))]).unwrap().serialize(), "a && c > 1 || b && c > 1");
    assert_eq!(Expr::and(vec![a_or_b(), Some(Expr::is_in("c", "d"))]).unwrap().serialize(), "a && c in 'd' || b && c in 'd'");
    assert_eq!(Expr::and(vec![has("a"), has("b"), Expr::or(vec![not("b"), Expr::and(vec![not("b"), has("c")])])]).unwrap().serialize(), "false");
}

#[test]
fn issue_106524_distributing_and_should_normalize() {
    let actual = Expr::and(vec![Expr::or(vec![has("a"), has("b")]), has("c")]).unwrap();
    let expected = Expr::or(vec![Expr::and(vec![has("a"), has("c")]), Expr::and(vec![has("b"), has("c")])]).unwrap();
    assert!(actual.equals(&expected));
}

#[test]
fn issue_129625_removes_duplicated_terms_in_or_expressions() {
    assert_eq!(Expr::or(vec![has("A"), has("B"), has("A")]).unwrap().serialize(), "A || B");
}

#[test]
fn resolves_true_constant_or_expressions() {
    assert_eq!(Expr::or(vec![has("A"), not("A")]).unwrap().serialize(), "true");
}

#[test]
fn resolves_false_constant_and_expressions() {
    assert_eq!(Expr::and(vec![has("A"), not("A")]).unwrap().serialize(), "false");
}

#[test]
fn issue_129625_removes_duplicated_terms_in_and_expressions() {
    assert_eq!(Expr::and(vec![has("A"), has("B"), has("A")]).unwrap().serialize(), "A && B");
}

#[test]
fn issue_129625_remove_duplicated_terms_when_negating() {
    let expr = Expr::and(vec![has("A"), Expr::or(vec![has("B1"), has("B2")])]).unwrap();
    assert_eq!(expr.serialize(), "A && B1 || A && B2");
    assert_eq!(expr.negate().serialize(), "!A || !A && !B1 || !A && !B2 || !B1 && !B2");
    assert_eq!(expr.negate().negate().serialize(), "A && B1 || A && B2");
    assert_eq!(expr.negate().negate().negate().serialize(), "!A || !A && !B1 || !A && !B2 || !B1 && !B2");
}

fn str_implies(p: &str, q: &str) -> bool {
    implies(&deserialize(p), &deserialize(q))
}

#[test]
fn issue_129625_remove_redundant_terms_in_or_expressions() {
    assert!(str_implies("a && b", "a"));
    assert!(!str_implies("a", "a && b"));
}

#[test]
fn implies_cases() {
    assert!(str_implies("a", "a"));
    assert!(str_implies("a", "a || b"));
    assert!(!str_implies("a", "a && b"));
    assert!(!str_implies("a", "a && b || a && c"));
    assert!(str_implies("a && b", "a"));
    assert!(str_implies("a && b", "b"));
    assert!(str_implies("a && b", "a && b || c"));
    assert!(!str_implies("a || b", "a || c"));
    assert!(str_implies("a || b", "a || b"));
    assert!(str_implies("a && b", "a && b"));
    assert!(str_implies("a || b", "a || b || c"));
    assert!(str_implies("c && a && b", "c && a"));
}

#[test]
fn greater_greater_equals_smaller_smaller_equals_evaluate() {
    let cases = [
        ("a > 1", json!({}), false),
        ("a > 1", json!({ "a": 0 }), false),
        ("a > 1", json!({ "a": 1 }), false),
        ("a > 1", json!({ "a": 2 }), true),
        ("a > 1", json!({ "a": "0" }), false),
        ("a > 1", json!({ "a": "1" }), false),
        ("a > 1", json!({ "a": "2" }), true),
        ("a > 1", json!({ "a": "a" }), false),
        ("a > 10", json!({ "a": 2 }), false),
        ("a > 10", json!({ "a": 11 }), true),
        ("a > 10", json!({ "a": "11" }), true),
        ("a > 10", json!({ "a": "2" }), false),
        ("a > 10", json!({ "a": "11" }), true),
        ("a > 1.1", json!({ "a": 1 }), false),
        ("a > 1.1", json!({ "a": 2 }), true),
        ("a > 1.1", json!({ "a": 11 }), true),
        ("a > 1.1", json!({ "a": "1.1" }), false),
        ("a > 1.1", json!({ "a": "2" }), true),
        ("a > 1.1", json!({ "a": "11" }), true),
        ("a > b", json!({ "a": "b" }), false),
        ("a > b", json!({ "a": "c" }), false),
        ("a > b", json!({ "a": 1000 }), false),
        ("a >= 2", json!({ "a": "1" }), false),
        ("a >= 2", json!({ "a": "2" }), true),
        ("a >= 2", json!({ "a": "3" }), true),
        ("a < 2", json!({ "a": "1" }), true),
        ("a < 2", json!({ "a": "2" }), false),
        ("a < 2", json!({ "a": "3" }), false),
        ("a <= 2", json!({ "a": "1" }), true),
        ("a <= 2", json!({ "a": "2" }), true),
        ("a <= 2", json!({ "a": "3" }), false),
    ];
    for (expr, values, expected) in cases {
        assert_eq!(deserialize(expr).evaluate(&context(values.clone())), expected, "{expr} with {values}");
    }
}

#[test]
fn greater_greater_equals_smaller_smaller_equals_negate() {
    let cases = [
        ("a > 1", "a <= 1"),
        ("a > 1.1", "a <= 1.1"),
        ("a > b", "a <= b"),
        ("a >= 1", "a < 1"),
        ("a >= 1.1", "a < 1.1"),
        ("a >= b", "a < b"),
        ("a < 1", "a >= 1"),
        ("a < 1.1", "a >= 1.1"),
        ("a < b", "a >= b"),
        ("a <= 1", "a > 1"),
        ("a <= 1.1", "a > 1.1"),
        ("a <= b", "a > b"),
    ];
    for (expr, expected) in cases {
        assert_eq!(deserialize(expr).negate().serialize(), expected, "{expr}");
    }
}

#[test]
fn issue_111899_context_keys_can_use_lt_or_gt() {
    let actual = deserialize("editorTextFocus && vim.active && vim.use<C-r>");
    assert!(actual.equals(&Expr::and(vec![has("editorTextFocus"), has("vim.active"), has("vim.use<C-r>")]).unwrap()));
}

#[test]
fn js_regex_anchors_and_flags_follow_javascript() {
    let value = Some(json!("one\ntwo"));
    assert!(!JsRegex::new("^two$", "").unwrap().test(value.as_ref()));
    assert!(JsRegex::new("^two$", "m").unwrap().test(value.as_ref()));
    assert!(!JsRegex::new("one.two", "").unwrap().test(value.as_ref()));
    assert!(JsRegex::new("one.two", "s").unwrap().test(value.as_ref()));
    assert!(JsRegex::new("ab", "gg").is_none());
}
