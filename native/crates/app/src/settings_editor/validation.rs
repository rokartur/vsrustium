//! Port of services/preferences/common/preferencesValidation.ts.

#[cfg(test)]
mod tests;

use std::collections::HashSet;

use regex::Regex;
use serde_json::{Map, Value};

use crate::theme::parse_hex;

/// `createValidator(prop)(value)`: the error messages for `value`, or "" when it is valid.
pub fn validate(schema: &Value, value: &Value) -> String {
    let types = types(schema);
    let nullable = types.contains(&"null");
    let numeric = (types.contains(&"number") || types.contains(&"integer")) && (types.len() == 1 || types.len() == 2 && nullable);
    if nullable && is_null_or_empty(value) {
        return String::new();
    }

    let mut errors = Vec::new();
    if let Some(error) = validate_array(schema, value).filter(|error| !error.is_empty()) {
        errors.push(error);
    }
    if let Some(error) = validate_object(schema, value).filter(|error| !error.is_empty()) {
        errors.push(error);
    }
    if schema["type"] == "boolean" && !value.is_boolean() {
        errors.push("Incorrect type. Expected \"boolean\".".to_string());
    }
    if numeric {
        let number = match value {
            Value::Bool(_) | Value::Array(_) => None,
            _ if is_null_or_empty(value) => None,
            _ => js_number(value),
        };
        match number {
            Some(number) => errors.extend(numeric_errors(schema, number)),
            None => errors.push("Value must be a number.".to_string()),
        }
    }
    if schema["type"] == "string" {
        let string_enum = schema["enum"].as_array().is_none_or(|values| values.iter().all(Value::is_string));
        match value {
            _ if !string_enum => errors.push("The enum options should be strings, but there is a non-string option. Please file an issue with the extension author.".to_string()),
            Value::String(text) => errors.extend(string_errors(schema, text)),
            _ => errors.push("Incorrect type. Expected \"string\".".to_string()),
        }
    }

    join_errors(schema, errors)
}

/// `getInvalidTypeError`: set when the value cannot be shown in the settings UI for the schema's type.
pub fn invalid_type_error(value: &Value, schema_type: &Value) -> Option<String> {
    let valid = match schema_type {
        Value::Null => return None,
        Value::Array(types) => types.iter().any(|type_| validates_as_type(value, type_.as_str().unwrap_or_default())),
        type_ => validates_as_type(value, type_.as_str().unwrap_or_default()),
    };
    (!valid).then(|| format!("Setting has an invalid type, expected {schema_type}. Fix in JSON."))
}

/// `validatePropertyName`.
pub fn validate_property_name(property_names: &Value, key: &str) -> bool {
    property_names["pattern"].as_str().is_none_or(|pattern| to_regex(pattern).is_match(key))
}

fn validates_as_type(value: &Value, type_: &str) -> bool {
    match type_ {
        "boolean" => value.is_boolean(),
        "object" => value.is_object(),
        "null" => value.is_null(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "number" | "integer" => value.is_number(),
        _ => true,
    }
}

fn types(schema: &Value) -> Vec<&str> {
    match &schema["type"] {
        Value::Array(types) => types.iter().filter_map(Value::as_str).collect(),
        Value::String(type_) => vec![type_.as_str()],
        _ => vec![""],
    }
}

fn is_null_or_empty(value: &Value) -> bool {
    value.is_null() || value == ""
}

fn join_errors(schema: &Value, errors: Vec<String>) -> String {
    if errors.is_empty() {
        return String::new();
    }
    match schema["errorMessage"].as_str() {
        Some(message) => [message.to_string()].into_iter().chain(errors).collect::<Vec<_>>().join(" "),
        None => errors.join(" "),
    }
}

/// JavaScript's unary `+` on a JSON value; `None` where it gives NaN.
fn js_number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => js_string_to_number(text),
        Value::Null | Value::Bool(_) | Value::Array(_) | Value::Object(_) => None,
    }
}

/// `Number(text)`: trimmed, empty is 0, `Infinity` and 0x/0o/0b literals accepted.
pub fn js_string_to_number(text: &str) -> Option<f64> {
    let text = text.trim();
    if text.is_empty() {
        return Some(0.0);
    }
    for (prefix, radix) in [("0x", 16), ("0X", 16), ("0o", 8), ("0O", 8), ("0b", 2), ("0B", 2)] {
        if let Some(digits) = text.strip_prefix(prefix) {
            return u64::from_str_radix(digits, radix).ok().map(|number| number as f64);
        }
    }
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    if unsigned == "Infinity" {
        return Some(if text.starts_with('-') { f64::NEG_INFINITY } else { f64::INFINITY });
    }
    // Rust also reads "inf" and "nan", which JavaScript does not.
    if !unsigned.starts_with(|char: char| char.is_ascii_digit() || char == '.') {
        return None;
    }
    text.parse().ok()
}

fn number_of(schema: &Value, key: &str) -> Option<f64> {
    schema[key].as_f64()
}

fn numeric_errors(schema: &Value, value: f64) -> Vec<String> {
    let types = types(schema);
    let nullable = types.contains(&"null");
    let integral = types.contains(&"integer") && (types.len() == 1 || types.len() == 2 && nullable);
    let maximum = number_of(schema, "maximum");
    let minimum = number_of(schema, "minimum");
    let exclusive_max = match &schema["exclusiveMaximum"] {
        Value::Bool(exclusive) => maximum.filter(|_| *exclusive),
        other => other.as_f64(),
    };
    let exclusive_min = match &schema["exclusiveMinimum"] {
        Value::Bool(exclusive) => minimum.filter(|_| *exclusive),
        other => other.as_f64(),
    };

    let mut errors = Vec::new();
    if let Some(max) = exclusive_max.filter(|max| maximum.is_none_or(|maximum| *max <= maximum))
        && value >= max
    {
        errors.push(format!("Value must be strictly less than {}.", js_number_text(max)));
    }
    if let Some(min) = exclusive_min.filter(|min| minimum.is_none_or(|minimum| *min >= minimum))
        && value <= min
    {
        errors.push(format!("Value must be strictly greater than {}.", js_number_text(min)));
    }
    if let Some(maximum) = maximum.filter(|maximum| exclusive_max.is_none_or(|max| max > *maximum))
        && value > maximum
    {
        errors.push(format!("Value must be less than or equal to {}.", js_number_text(maximum)));
    }
    if let Some(minimum) = minimum.filter(|minimum| exclusive_min.is_none_or(|min| min < *minimum))
        && value < minimum
    {
        errors.push(format!("Value must be greater than or equal to {}.", js_number_text(minimum)));
    }
    if let Some(multiple_of) = number_of(schema, "multipleOf")
        && value % multiple_of != 0.0
    {
        errors.push(format!("Value must be a multiple of {}.", js_number_text(multiple_of)));
    }
    if integral && value % 1.0 != 0.0 {
        errors.push("Value must be an integer.".to_string());
    }
    errors
}

/// A number as JavaScript prints it in a template: `5`, not `5.0`.
fn js_number_text(number: f64) -> String {
    if number.fract() == 0.0 && number.abs() < 1e21 { format!("{}", number as i64) } else { number.to_string() }
}

fn string_errors(schema: &Value, value: &str) -> Vec<String> {
    let uri = schema["format"] == "uri";
    let uri_reference = uri || schema["format"] == "uri-reference";
    let length = value.encode_utf16().count() as f64;

    let mut errors = Vec::new();
    if let Some(max_length) = number_of(schema, "maxLength")
        && length > max_length
    {
        errors.push(format!("Value must be {} or fewer characters long.", js_number_text(max_length)));
    }
    if let Some(min_length) = number_of(schema, "minLength")
        && length < min_length
    {
        errors.push(format!("Value must be {} or more characters long.", js_number_text(min_length)));
    }
    if let Some(pattern) = schema["pattern"].as_str()
        && !to_regex(pattern).is_match(value)
    {
        errors.push(match schema["patternErrorMessage"].as_str() {
            Some(message) => message.to_string(),
            None => format!("Value must match regex `{pattern}`."),
        });
    }
    if schema["format"] == "color-hex" && parse_hex(value).is_none() {
        errors.push("Invalid color format. Use #RGB, #RGBA, #RRGGBB or #RRGGBBAA.".to_string());
    }
    // The upstream `uriRegex` matches every string, so "URI is expected." never shows.
    if uri_reference && value.is_empty() {
        errors.push("URI expected.".to_string());
    }
    if uri && uri_scheme(value).is_none() {
        errors.push("URI with a scheme is expected.".to_string());
    }
    if let Some(values) = schema["enum"].as_array()
        && !values.iter().any(|item| item == value)
    {
        let quoted: Vec<_> = values.iter().map(|item| format!("\"{}\"", item.as_str().unwrap_or_default())).collect();
        errors.push(format!("Value is not accepted. Valid values: {}.", quoted.join(", ")));
    }
    errors
}

/// Group 2 of upstream's `^(([^:/?#]+?):)?(\/\/([^/?#]*))?...` URI regex.
fn uri_scheme(value: &str) -> Option<&str> {
    let end = value.find([':', '/', '?', '#'])?;
    (end > 0 && value[end..].starts_with(':')).then(|| &value[..end])
}

/// `toRegExp`; a pattern that does not compile matches everything, as upstream's `/.*/` fallback.
fn to_regex(pattern: &str) -> Regex {
    Regex::new(pattern).unwrap_or_else(|_| Regex::new("").expect("the empty pattern compiles"))
}

fn validate_array(schema: &Value, value: &Value) -> Option<String> {
    let items = &schema["items"];
    if schema["type"] != "array" || !items.is_object() || items["type"].is_array() {
        return None;
    }
    if is_falsy(value) {
        return Some(String::new());
    }
    let Value::Array(array) = value else {
        return Some("Incorrect type. Expected an array.\n".to_string());
    };

    let mut message = String::new();
    if schema["uniqueItems"] == true && has_duplicates(array) {
        message += "Array has duplicate items\n";
    }
    if let Some(min_items) = number_of(schema, "minItems").filter(|min| *min != 0.0)
        && (array.len() as f64) < min_items
    {
        message += &format!("Array must have at least {} items\n", js_number_text(min_items));
    }
    if let Some(max_items) = number_of(schema, "maxItems").filter(|max| *max != 0.0)
        && array.len() as f64 > max_items
    {
        message += &format!("Array must have at most {} items\n", js_number_text(max_items));
    }

    match items["type"].as_str() {
        Some("string") => {
            let Some(strings) = array.iter().map(Value::as_str).collect::<Option<Vec<_>>>() else {
                message += "Incorrect type. Expected a string array.\n";
                return Some(message);
            };
            if let Some(pattern) = items["pattern"].as_str() {
                let regex = to_regex(pattern);
                for item in strings.iter().filter(|item| !regex.is_match(item)) {
                    message += &match items["patternErrorMessage"].as_str() {
                        Some(error) => error.to_string(),
                        None => format!("Value '{item}' must match regex '{pattern}'."),
                    };
                }
            }
            if let Some(values) = items["enum"].as_array() {
                let quoted: Vec<_> = values.iter().map(|value| format!("'{}'", value.as_str().unwrap_or_default())).collect();
                for item in strings.iter().filter(|item| !values.iter().any(|value| value == **item)) {
                    message += &format!("Value '{item}' is not one of [{}]\n", quoted.join(", "));
                }
            }
        }
        Some("integer" | "number") => {
            for item in array {
                let error = validate(items, item);
                if !error.is_empty() {
                    message += &format!("{}: {error}\n", js_display(item));
                }
            }
        }
        _ => {}
    }
    Some(message)
}

fn validate_object(schema: &Value, value: &Value) -> Option<String> {
    if schema["type"] != "object" {
        return None;
    }
    if is_falsy(value) {
        return Some(String::new());
    }
    let Value::Object(object) = value else {
        return Some(join_errors(schema, vec!["Incorrect type. Expected an object.".to_string()]));
    };
    Some(join_errors(schema, object_errors(schema, object)))
}

fn object_errors(schema: &Value, object: &Map<String, Value>) -> Vec<String> {
    let mut errors = Vec::new();
    let mut property_names_error_shown = false;
    for (key, data) in object {
        if !property_names_error_shown && !validate_property_name(&schema["propertyNames"], key) {
            let message = match schema["propertyNames"]["patternErrorMessage"].as_str() {
                Some(message) => message.to_string(),
                None => format!("Property name must match pattern `{}`.", schema["propertyNames"]["pattern"].as_str().unwrap_or_default()),
            };
            errors.push(format!("{message}\n"));
            property_names_error_shown = true;
        }

        let property_schema = schema["properties"].get(key).or_else(|| {
            let patterns = schema["patternProperties"].as_object()?;
            patterns.iter().find(|(pattern, _)| to_regex(pattern).is_match(key)).map(|(_, schema)| schema)
        });
        if let Some(property_schema) = property_schema {
            let error = validate(property_schema, data);
            if !error.is_empty() {
                errors.push(format!("{key}: {error}\n"));
            }
            continue;
        }

        match &schema["additionalProperties"] {
            Value::Bool(false) => errors.push(format!("Property {key} is not allowed.\n")),
            additional @ Value::Object(_) => {
                let error = validate(additional, data);
                if !error.is_empty() {
                    errors.push(format!("{key}: {error}\n"));
                }
            }
            _ => {}
        }
    }
    errors
}

/// JavaScript's `!value` on a JSON value.
fn is_falsy(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Bool(flag) => !flag,
        Value::Number(number) => number.as_f64() == Some(0.0),
        Value::String(text) => text.is_empty(),
        Value::Array(_) | Value::Object(_) => false,
    }
}

/// `new Set(array).size < array.length`: primitives compare by value, arrays and objects by identity.
fn has_duplicates(array: &[Value]) -> bool {
    let mut seen = HashSet::new();
    array.iter().filter(|item| !item.is_array() && !item.is_object()).any(|item| !seen.insert(item.to_string()))
}

/// `${value}` in a template string.
fn js_display(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.as_f64().map(js_number_text).unwrap_or_else(|| number.to_string()),
        other => other.to_string(),
    }
}
