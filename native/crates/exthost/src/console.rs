//! Host console entries (`IRemoteConsoleLog`) as text: the JSON argument array that
//! `safeStringifyArgumentsToArray` sends, formatted like Node's `util.format`.

use serde_json::Value;

/// One line of output for a `$logExtensionHostMessage` entry, without the `__$stack` argument.
pub fn format_console_entry(entry: &Value) -> String {
    let Some(Ok(Value::Array(mut args))) = entry["arguments"].as_str().map(serde_json::from_str::<Value>) else {
        return format!("Unable to log remote console arguments {}", entry["arguments"]);
    };
    if args.last().is_some_and(|last| last.get("__$stack").is_some()) {
        args.pop();
    }
    format_args(&args)
}

fn format_args(args: &[Value]) -> String {
    let mut rest = args.iter();
    let mut out = String::new();
    if let Some(Value::String(format)) = args.first() {
        rest.next();
        let mut chars = format.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '%' {
                out.push(c);
                continue;
            }
            match chars.peek().copied() {
                Some('%') => {
                    chars.next();
                    out.push('%');
                }
                // %c takes a CSS argument and prints nothing.
                Some('c') => {
                    chars.next();
                    rest.next();
                }
                Some(spec @ ('s' | 'd' | 'i' | 'f' | 'j' | 'o' | 'O')) => match rest.next() {
                    Some(arg) => {
                        chars.next();
                        out.push_str(&format_value(arg, spec));
                    }
                    None => out.push('%'),
                },
                _ => out.push('%'),
            }
        }
    }
    for arg in rest {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&format_value(arg, 's'));
    }
    out
}

fn format_value(value: &Value, spec: char) -> String {
    match (value, spec) {
        (Value::Number(number), 'i') => number.as_f64().map(|n| n.trunc().to_string()).unwrap_or_else(|| number.to_string()),
        (Value::String(text), _) => text.clone(),
        _ => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(args: Value) -> Value {
        json!({ "type": "__$console", "severity": "log", "arguments": args.to_string() })
    }

    #[test]
    fn formats_like_util_format() {
        assert_eq!(format_console_entry(&entry(json!(["%s  %d passing", "\u{1b}[32m", 12, { "a": 1 }]))), "\u{1b}[32m  12 passing {\"a\":1}");
        assert_eq!(format_console_entry(&entry(json!(["100%% %c done %s", "color: blue"]))), "100%  done %s");
        assert_eq!(format_console_entry(&entry(json!(["%i of %d", 2.7, 2.7]))), "2 of 2.7");
        assert_eq!(format_console_entry(&entry(json!([3, "x", null]))), "3 x null");
    }

    #[test]
    fn drops_the_stack_argument() {
        assert_eq!(format_console_entry(&entry(json!(["boom", { "__$stack": "at x" }]))), "boom");
    }

    #[test]
    fn reports_unparsable_arguments() {
        assert_eq!(format_console_entry(&json!({ "arguments": "{" })), "Unable to log remote console arguments \"{\"");
    }
}
