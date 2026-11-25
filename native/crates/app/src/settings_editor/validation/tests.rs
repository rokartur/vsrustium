// Port of services/preferences/test/common/preferencesValidation.test.ts.

use serde_json::{Value, json};

use super::{invalid_type_error, validate};

struct Tester(Value);

impl Tester {
    fn accepts(&self, input: Value) {
        assert_eq!(validate(&self.0, &input), "", "{} should accept {input}", self.0);
    }

    fn rejects(&self, input: Value) -> String {
        let error = validate(&self.0, &input);
        assert_ne!(error, "", "{} should reject {input}", self.0);
        error
    }

    fn rejects_with(&self, input: Value, message: &str) {
        let error = self.rejects(input);
        assert!(error.contains(message), "expected {message:?} in {error:?}");
    }

    fn validates_numeric(&self) {
        for input in ["3", "3.", ".0", "3.0", " 3.0", " 3.0  "] {
            self.accepts(json!(input));
        }
        self.rejects(json!("3f"));
        self.accepts(json!(3));
        self.rejects(json!("test"));
    }

    fn validates_non_nullable_numeric(&self) {
        self.validates_numeric();
        self.accepts(json!(0));
        self.rejects(json!(""));
        self.rejects(Value::Null);
    }
}

fn tester(schema: Value) -> Tester {
    Tester(schema)
}

#[test]
fn exclusive_max_and_max_work_together_properly() {
    let just_max = tester(json!({ "maximum": 5, "type": "number" }));
    just_max.validates_non_nullable_numeric();
    just_max.rejects(json!("5.1"));
    just_max.accepts(json!("5.0"));

    let just_exclusive_max = tester(json!({ "exclusiveMaximum": 5, "type": "number" }));
    just_exclusive_max.validates_non_nullable_numeric();
    just_exclusive_max.rejects(json!("5.1"));
    just_exclusive_max.rejects(json!("5.0"));
    just_exclusive_max.accepts(json!("4.999"));

    let both = tester(json!({ "exclusiveMaximum": 5, "maximum": 4, "type": "number" }));
    both.validates_non_nullable_numeric();
    both.rejects(json!("5.1"));
    both.rejects(json!("5.0"));
    both.rejects(json!("4.999"));
    both.accepts(json!("4"));

    let both = tester(json!({ "exclusiveMaximum": 5, "maximum": 6, "type": "number" }));
    both.validates_non_nullable_numeric();
    both.rejects(json!("5.1"));
    both.rejects(json!("5.0"));
    both.accepts(json!("4.999"));
}

#[test]
fn exclusive_min_and_min_work_together_properly() {
    let just_min = tester(json!({ "minimum": -5, "type": "number" }));
    just_min.validates_non_nullable_numeric();
    just_min.rejects(json!("-5.1"));
    just_min.accepts(json!("-5.0"));

    let just_exclusive_min = tester(json!({ "exclusiveMinimum": -5, "type": "number" }));
    just_exclusive_min.validates_non_nullable_numeric();
    just_exclusive_min.rejects(json!("-5.1"));
    just_exclusive_min.rejects(json!("-5.0"));
    just_exclusive_min.accepts(json!("-4.999"));

    let both = tester(json!({ "exclusiveMinimum": -5, "minimum": -4, "type": "number" }));
    both.validates_non_nullable_numeric();
    both.rejects(json!("-5.1"));
    both.rejects(json!("-5.0"));
    both.rejects(json!("-4.999"));
    both.accepts(json!("-4"));

    let both = tester(json!({ "exclusiveMinimum": -5, "minimum": -6, "type": "number" }));
    both.validates_non_nullable_numeric();
    both.rejects(json!("-5.1"));
    both.rejects(json!("-5.0"));
    both.accepts(json!("-4.999"));
}

#[test]
fn multiple_of_works_for_both_integers_and_fractions() {
    let only_evens = tester(json!({ "multipleOf": 2, "type": "number" }));
    for input in ["2.0", "2", "-4", "0", "100"] {
        only_evens.accepts(json!(input));
    }
    only_evens.rejects(json!("100.1"));
    only_evens.rejects(json!(""));
    only_evens.rejects(json!("we"));

    let hacky_integers = tester(json!({ "multipleOf": 1, "type": "number" }));
    hacky_integers.accepts(json!("2.0"));
    hacky_integers.rejects(json!(".5"));

    let half_integers = tester(json!({ "multipleOf": 0.5, "type": "number" }));
    half_integers.accepts(json!("0.5"));
    half_integers.accepts(json!("1.5"));
    half_integers.rejects(json!("1.51"));
}

#[test]
fn integer_type_correctly_adds_a_validation() {
    let integers = tester(json!({ "multipleOf": 1, "type": "integer" }));
    integers.accepts(json!("02"));
    integers.accepts(json!("2"));
    integers.accepts(json!("20"));
    integers.rejects(json!(".5"));
    integers.rejects(json!("2j"));
    integers.rejects(json!(""));
}

#[test]
fn null_is_allowed_only_when_expected() {
    let nullable_integers = tester(json!({ "type": ["integer", "null"] }));
    nullable_integers.accepts(json!("2"));
    nullable_integers.rejects(json!(".5"));
    nullable_integers.accepts(json!("2.0"));
    nullable_integers.rejects(json!("2j"));
    nullable_integers.accepts(json!(""));

    let non_nullable_integers = tester(json!({ "type": ["integer"] }));
    non_nullable_integers.accepts(json!("2"));
    non_nullable_integers.rejects(json!(".5"));
    non_nullable_integers.accepts(json!("2.0"));
    non_nullable_integers.rejects(json!("2j"));
    non_nullable_integers.rejects(json!(""));

    let nullable_numbers = tester(json!({ "type": ["number", "null"] }));
    nullable_numbers.accepts(json!("2"));
    nullable_numbers.accepts(json!(".5"));
    nullable_numbers.accepts(json!("2.0"));
    nullable_numbers.rejects(json!("2j"));
    nullable_numbers.accepts(json!(""));

    let non_nullable_numbers = tester(json!({ "type": ["number"] }));
    non_nullable_numbers.accepts(json!("2"));
    non_nullable_numbers.accepts(json!(".5"));
    non_nullable_numbers.accepts(json!("2.0"));
    non_nullable_numbers.rejects(json!("2j"));
    non_nullable_numbers.rejects(json!(""));
}

#[test]
fn string_max_min_length_work() {
    let min = tester(json!({ "minLength": 4, "type": "string" }));
    min.rejects(json!("123"));
    min.accepts(json!("1234"));
    min.accepts(json!("12345"));

    let max = tester(json!({ "maxLength": 6, "type": "string" }));
    max.accepts(json!("12345"));
    max.accepts(json!("123456"));
    max.rejects(json!("1234567"));

    let min_max = tester(json!({ "minLength": 4, "maxLength": 6, "type": "string" }));
    min_max.rejects(json!("123"));
    min_max.accepts(json!("1234"));
    min_max.accepts(json!("12345"));
    min_max.accepts(json!("123456"));
    min_max.rejects(json!("1234567"));
}

#[test]
fn objects_work() {
    let object = tester(json!({ "type": "object", "properties": { "a": { "type": "string", "maxLength": 2 } }, "additionalProperties": false }));
    object.rejects(json!({ "a": "string" }));
    object.accepts(json!({ "a": "st" }));
    object.rejects(json!({ "a": null }));
    object.rejects(json!({ "a": 7 }));
    object.accepts(json!({}));
    object.rejects(json!("test"));
    object.rejects(json!(7));
    object.rejects(json!([1, 2, 3]));

    let pattern = tester(json!({ "type": "object", "patternProperties": { "^a[a-z]$": { "type": "string", "minLength": 2 } }, "additionalProperties": false }));
    pattern.accepts(json!({ "ab": "string" }));
    pattern.accepts(json!({ "ab": "string", "ac": "hmm" }));
    pattern.rejects(json!({ "ab": "string", "ac": "h" }));
    pattern.rejects(json!({ "ab": "string", "ac": 99999 }));
    pattern.rejects(json!({ "abc": "string" }));
    pattern.rejects(json!({ "a0": "string" }));
    pattern.rejects(json!({ "ab": "string", "bc": "hmm" }));
    pattern.rejects(json!({ "be": "string" }));
    pattern.rejects(json!({ "be": "a" }));
    pattern.accepts(json!({}));

    let pattern = tester(json!({ "type": "object", "patternProperties": { "^#": { "type": "string", "minLength": 3 } }, "additionalProperties": { "type": "string", "maxLength": 3 } }));
    pattern.accepts(json!({ "#ab": "string" }));
    pattern.accepts(json!({ "ab": "str" }));
    pattern.rejects(json!({ "#ab": "s" }));
    pattern.rejects(json!({ "ab": 99999 }));
    pattern.rejects(json!({ "#ab": 99999 }));
    pattern.accepts(json!({}));

    let pattern = tester(json!({ "type": "object", "properties": { "hello": { "type": "string" } }, "additionalProperties": { "type": "boolean" } }));
    pattern.accepts(json!({ "hello": "world" }));
    pattern.accepts(json!({ "hello": "world", "bye": false }));
    pattern.rejects(json!({ "hello": "world", "bye": "false" }));
    pattern.rejects(json!({ "hello": "world", "bye": 1 }));
    pattern.rejects(json!({ "hello": "world", "bye": "world" }));
    pattern.accepts(json!({ "hello": "test" }));
    pattern.accepts(json!({}));
}

#[test]
fn numerical_objects_work() {
    let object = tester(json!({ "type": "object", "properties": { "b": { "type": "number" } } }));
    object.accepts(json!({ "b": 2.5 }));
    object.accepts(json!({ "b": -2.5 }));
    object.accepts(json!({ "b": 0 }));
    object.accepts(json!({ "b": "0.12" }));
    object.rejects(json!({ "b": "abc" }));
    object.rejects(json!({ "b": [] }));
    object.rejects(json!({ "b": false }));
    object.rejects(json!({ "b": null }));

    let object = tester(json!({ "type": "object", "properties": { "b": { "type": "integer", "minimum": 2, "maximum": 5.5 } } }));
    object.accepts(json!({ "b": 2 }));
    object.accepts(json!({ "b": 3 }));
    object.accepts(json!({ "b": "3.0" }));
    object.accepts(json!({ "b": 5 }));
    object.rejects(json!({ "b": 1 }));
    object.rejects(json!({ "b": 6 }));
    object.rejects(json!({ "b": 5.5 }));
}

#[test]
fn patterns_work() {
    let urls = tester(json!({ "pattern": "^(hello)*$", "type": "string" }));
    urls.accepts(json!(""));
    urls.rejects(json!("hel"));
    urls.accepts(json!("hello"));
    urls.rejects(json!("hellohel"));
    urls.accepts(json!("hellohello"));

    let urls = tester(json!({ "pattern": "^(hello)*$", "type": "string", "patternErrorMessage": "err: must be friendly" }));
    urls.accepts(json!(""));
    urls.rejects_with(json!("hel"), "err: must be friendly");
    urls.accepts(json!("hello"));
    urls.rejects_with(json!("hellohel"), "err: must be friendly");
    urls.accepts(json!("hellohello"));

    let unicode_pattern = tester(json!({ "type": "string", "pattern": "^[\\p{L}\\d_. -]*$", "minLength": 3 }));
    unicode_pattern.accepts(json!("_autoload"));
    unicode_pattern.rejects(json!("#hash"));
    unicode_pattern.rejects(json!(""));
}

#[test]
fn custom_error_messages_are_shown() {
    let with_message = tester(json!({ "minLength": 1, "maxLength": 0, "type": "string", "errorMessage": "always error!" }));
    with_message.rejects_with(json!(""), "always error!");
    with_message.rejects_with(json!(" "), "always error!");
    with_message.rejects_with(json!("1"), "always error!");
}

#[test]
fn simple_array() {
    let array = tester(json!({ "type": "array", "items": { "type": "string" } }));
    array.accepts(json!([]));
    array.accepts(json!(["foo"]));
    array.accepts(json!(["foo", "bar"]));
    array.rejects(json!(76));
    array.rejects(json!([6, "3", 7]));
}

#[test]
fn min_max_items_array() {
    let array = tester(json!({ "type": "array", "items": { "type": "string" }, "minItems": 1, "maxItems": 2 }));
    array.rejects_with(json!([]), "Array must have at least 1 items");
    array.accepts(json!(["a"]));
    array.accepts(json!(["a", "a"]));
    array.rejects_with(json!(["a", "a", "a"]), "Array must have at most 2 items");
}

#[test]
fn array_of_enums() {
    let array = tester(json!({ "type": "array", "items": { "type": "string", "enum": ["a", "b"] } }));
    array.accepts(json!(["a"]));
    array.accepts(json!(["a", "b"]));
    array.rejects_with(json!(["c"]), "Value 'c' is not one of");
    array.rejects_with(json!(["a", "c"]), "Value 'c' is not one of");
    array.rejects_with(json!(["c", "d"]), "Value 'c' is not one of");
    array.rejects_with(json!(["c", "d"]), "Value 'd' is not one of");
}

#[test]
fn array_of_numbers() {
    let array = tester(json!({ "type": "array", "items": { "type": "number" } }));
    array.accepts(json!([]));
    array.accepts(json!([2]));
    array.accepts(json!([2, 3]));
    array.accepts(json!(["2", "3"]));
    array.accepts(json!([6.6, "3", 7]));
    array.rejects(json!(76));
    array.rejects(json!(7.6));
    array.rejects(json!([6, "a", 7]));

    let array = tester(json!({ "type": "array", "items": { "type": "integer", "minimum": -2, "maximum": 3 }, "maxItems": 4 }));
    array.accepts(json!([]));
    array.accepts(json!([-2, 3]));
    array.accepts(json!([2, 3]));
    array.accepts(json!(["2", "3"]));
    array.accepts(json!(["-2", "0", "3"]));
    array.accepts(json!(["-2", 0.0, "3"]));
    array.rejects(json!(2));
    array.rejects(json!(76));
    array.rejects(json!([6, "3", 7]));
    array.rejects(json!([2, "a", 3]));
    array.rejects(json!([-2, 4]));
    array.rejects(json!([-1.2, 2.1]));
    array.rejects(json!([-3, 3]));
    array.rejects(json!([-3, 4]));
    array.rejects(json!([2, 2, 2, 2, 2]));
}

#[test]
fn min_max_and_enum() {
    let array = tester(json!({ "type": "array", "items": { "type": "string", "enum": ["a", "b"] }, "minItems": 1, "maxItems": 2 }));
    array.rejects_with(json!(["a", "b", "c"]), "Array must have at most 2 items");
    array.rejects_with(json!(["a", "b", "c"]), "Value 'c' is not one of");
}

#[test]
fn array_pattern() {
    let array = tester(json!({ "type": "array", "items": { "type": "string", "pattern": "^(hello)*$" } }));
    array.accepts(json!(["hello"]));
    array.rejects_with(json!(["a"]), "Value 'a' must match regex");
}

#[test]
fn array_unicode_pattern() {
    let array = tester(json!({ "type": "array", "items": { "type": "string", "pattern": "^[\\p{L}\\d_. -]*$" } }));
    array.accepts(json!(["hello", "world"]));
    array.rejects_with(json!(["hello", "#world"]), "Value '#world' must match regex");
}

#[test]
fn array_pattern_with_error_message() {
    let array = tester(json!({ "type": "array", "items": { "type": "string", "pattern": "^(hello)*$", "patternErrorMessage": "err: must be friendly" } }));
    array.rejects_with(json!(["a"]), "err: must be friendly");
}

#[test]
fn unique_items() {
    let array = tester(json!({ "type": "array", "items": { "type": "string" }, "uniqueItems": true }));
    array.rejects_with(json!(["a", "a"]), "Array has duplicate items");
}

#[test]
fn get_invalid_type_error() {
    let cases = [
        (json!(1), json!("number"), true),
        (json!(1.5), json!("number"), true),
        (json!([1]), json!("number"), false),
        (json!("1"), json!("number"), false),
        (json!({ "a": 1 }), json!("number"), false),
        (json!(null), json!("number"), false),
        (json!("a"), json!("string"), true),
        (json!("1"), json!("string"), true),
        (json!([]), json!("string"), false),
        (json!({}), json!("string"), false),
        (json!([1]), json!("array"), true),
        (json!([]), json!("array"), true),
        (json!([{}, [[]]]), json!("array"), true),
        (json!({ "a": ["a"] }), json!("array"), false),
        (json!("hello"), json!("array"), false),
        (json!(true), json!("boolean"), true),
        (json!("hello"), json!("boolean"), false),
        (json!(null), json!("boolean"), false),
        (json!([true]), json!("boolean"), false),
        (json!(null), json!("null"), true),
        (json!(false), json!("null"), false),
        (json!([null]), json!("null"), false),
        (json!("null"), json!("null"), false),
    ];
    for (value, type_, valid) in cases {
        assert_eq!(invalid_type_error(&value, &type_).is_none(), valid, "value {value}, type {type_}");
    }
}

#[test]
fn uri_checks_work() {
    let uri = tester(json!({ "type": "string", "format": "uri" }));
    for input in ["example.com", "example.com/example", "example/example.html", "www.example.com", "", " ", "example"] {
        uri.rejects(json!(input));
    }
    for input in ["https:", "https://", "https://example.com", "https://www.example.com"] {
        uri.accepts(json!(input));
    }
}
