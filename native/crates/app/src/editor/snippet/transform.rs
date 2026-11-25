//! snippetParser.ts `Transform` and `FormatString`: `${1/regex/format/options}` and `${NAME/regex/format/options}`.

use onig::{Regex, RegexOptions, Syntax};

#[derive(Clone, Debug, PartialEq)]
pub struct Transform {
    source: String,
    ignore_case: bool,
    global: bool,
    format: Vec<Format>,
}

#[derive(Clone, Debug, PartialEq)]
enum Format {
    Text(String),
    /// `$1`, `${1:/upcase}`, `${1:+if}`, `${1:?if:else}`, `${1:-else}`, `${1:else}`.
    Group { index: usize, shorthand: Option<String>, if_value: Option<String>, else_value: Option<String> },
}

/// `_parseTransform` after the first `/`: the regex, the format and the options up to `}`; `None` when it is cut off
/// or the regex does not compile.
pub(super) fn parse(chars: &[char], position: &mut usize) -> Option<Transform> {
    let mut source = String::new();
    loop {
        match *chars.get(*position)? {
            '/' => break,
            '\\' if chars.get(*position + 1) == Some(&'/') => {
                source.push('/');
                *position += 1;
            }
            c => source.push(c),
        }
        *position += 1;
    }
    *position += 1;
    let mut format = Vec::new();
    loop {
        match *chars.get(*position)? {
            '/' => break,
            '\\' => match chars.get(*position + 1) {
                Some(&next @ ('\\' | '/')) => {
                    format.push(Format::Text(next.to_string()));
                    *position += 1;
                }
                _ => format.push(Format::Text("\\".into())),
            },
            '$' => match parse_format_string(chars, position) {
                Some(group) => {
                    format.push(group);
                    continue;
                }
                None => format.push(Format::Text("$".into())),
            },
            c => format.push(Format::Text(c.to_string())),
        }
        *position += 1;
    }
    *position += 1;
    let mut options = String::new();
    loop {
        match *chars.get(*position)? {
            '}' => break,
            c => options.push(c),
        }
        *position += 1;
    }
    *position += 1;
    let transform = Transform { source, ignore_case: options.contains('i'), global: options.contains('g'), format };
    transform.regex().is_some().then_some(transform)
}

/// `_parseFormatString` at a `$`: a group reference, else `None` with the position unchanged.
fn parse_format_string(chars: &[char], position: &mut usize) -> Option<Format> {
    let start = *position;
    let group = parse_group(chars, position);
    if group.is_none() {
        *position = start;
    }
    group
}

fn parse_group(chars: &[char], position: &mut usize) -> Option<Format> {
    *position += 1;
    let braced = chars.get(*position) == Some(&'{');
    if braced {
        *position += 1;
    }
    let digits = take_while(chars, position, |c| c.is_ascii_digit());
    let index: usize = digits.parse().ok()?;
    let group = |shorthand, if_value, else_value| Some(Format::Group { index, shorthand, if_value, else_value });
    if !braced {
        return group(None, None, None);
    }
    match chars.get(*position)? {
        '}' => {
            *position += 1;
            return group(None, None, None);
        }
        ':' => *position += 1,
        _ => return None,
    }
    match chars.get(*position)? {
        '/' => {
            *position += 1;
            let name = take_while(chars, position, |c| c == '_' || c.is_ascii_alphanumeric());
            if name.is_empty() || chars.get(*position) != Some(&'}') {
                return None;
            }
            *position += 1;
            group(Some(name), None, None)
        }
        '+' => {
            *position += 1;
            group(None, Some(until(chars, position, '}')?), None)
        }
        '-' => {
            *position += 1;
            group(None, None, Some(until(chars, position, '}')?))
        }
        '?' => {
            *position += 1;
            let if_value = until(chars, position, ':')?;
            group(None, Some(if_value), Some(until(chars, position, '}')?))
        }
        _ => group(None, None, Some(until(chars, position, '}')?)),
    }
}

/// `_until`: the non-empty text up to `stop`, which is consumed, with `\$`, `\}` and `\\` unescaped.
fn until(chars: &[char], position: &mut usize, stop: char) -> Option<String> {
    let mut value = String::new();
    loop {
        match *chars.get(*position)? {
            c if c == stop => break,
            '\\' if matches!(chars.get(*position + 1), Some('$' | '}' | '\\')) => {
                value.push(chars[*position + 1]);
                *position += 1;
            }
            '\\' => {
                value.push('\\');
                if let Some(&next) = chars.get(*position + 1) {
                    value.push(next);
                    *position += 1;
                }
            }
            c => value.push(c),
        }
        *position += 1;
    }
    *position += 1;
    (!value.is_empty()).then_some(value)
}

fn take_while(chars: &[char], position: &mut usize, accept: impl Fn(char) -> bool) -> String {
    let start = *position;
    while chars.get(*position).is_some_and(|c| accept(*c)) {
        *position += 1;
    }
    chars[start..*position].iter().collect()
}

impl Transform {
    fn regex(&self) -> Option<Regex> {
        let options = if self.ignore_case { RegexOptions::REGEX_OPTION_IGNORECASE } else { RegexOptions::REGEX_OPTION_NONE };
        Regex::with_options(&self.source, options, Syntax::default()).ok()
    }

    /// `Transform.resolve`: `value.replace(regexp, ...)`, every match with `g`; no match runs the else branches.
    pub fn resolve(&self, value: &str) -> String {
        let regex = self.regex().expect("checked when parsed");
        let mut result = String::new();
        let mut last = 0;
        let mut matched = false;
        for captures in regex.captures_iter(value) {
            let (start, end) = captures.pos(0).expect("group 0 is the match");
            matched = true;
            result.push_str(&value[last..start]);
            let groups: Vec<&str> = (0..captures.len()).map(|index| captures.at(index).unwrap_or_default()).collect();
            result.push_str(&self.replace(&groups));
            last = end;
            if !self.global {
                break;
            }
        }
        result.push_str(&value[last..]);
        let has_else = self.format.iter().any(|format| matches!(format, Format::Group { else_value: Some(_), .. }));
        if !matched && has_else {
            return self.replace(&[]);
        }
        result
    }

    fn replace(&self, groups: &[&str]) -> String {
        let mut result = String::new();
        for format in &self.format {
            match format {
                Format::Text(text) => result.push_str(text),
                Format::Group { index, shorthand, if_value, else_value } => {
                    let value = groups.get(*index).copied().unwrap_or_default();
                    result.push_str(&resolve_format(value, shorthand.as_deref(), if_value.as_deref(), else_value.as_deref()));
                }
            }
        }
        result
    }
}

/// `FormatString.resolve`.
fn resolve_format(value: &str, shorthand: Option<&str>, if_value: Option<&str>, else_value: Option<&str>) -> String {
    match (shorthand, if_value, else_value) {
        (Some(_), _, _) if value.is_empty() => String::new(),
        (Some("upcase"), _, _) => value.to_uppercase(),
        (Some("downcase"), _, _) => value.to_lowercase(),
        (Some("capitalize"), _, _) => capitalize(value),
        (Some("pascalcase"), _, _) => words(value).map_or(value.to_owned(), |words| words.iter().map(|word| capitalize(word)).collect()),
        (Some("camelcase"), _, _) => words(value).map_or(value.to_owned(), |words| {
            words.iter().enumerate().map(|(index, word)| if index == 0 { lower_first(word) } else { capitalize(word) }).collect()
        }),
        (Some("kebabcase"), _, _) => kebab_case(value),
        (Some("snakecase"), _, _) => snake_case(value),
        (_, Some(if_value), _) if !value.is_empty() => if_value.to_owned(),
        (_, _, Some(else_value)) if value.is_empty() => else_value.to_owned(),
        _ => value.to_owned(),
    }
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map_or(String::new(), |first| first.to_uppercase().chain(chars).collect())
}

fn lower_first(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map_or(String::new(), |first| first.to_lowercase().chain(chars).collect())
}

/// `value.match(/[\p{L}0-9]+/gu)`.
fn words(value: &str) -> Option<Vec<&str>> {
    let words: Vec<&str> = value.split(|c: char| !(c.is_alphabetic() || c.is_ascii_digit())).filter(|word| !word.is_empty()).collect();
    (!words.is_empty()).then_some(words)
}

fn all_matches(pattern: &str, value: &str) -> Vec<String> {
    let regex = Regex::new(pattern).expect("constant pattern");
    regex.find_iter(value).map(|(start, end)| value[start..end].to_owned()).collect()
}

/// `_toKebabCase`.
fn kebab_case(value: &str) -> String {
    if words(value).is_none() {
        return value.to_owned();
    }
    let cleaned = value.trim().trim_matches('_');
    let parts = all_matches(r"\p{Lu}{2,}(?=\p{Lu}\p{Ll}+[0-9]*|[\s_-]|$)|\p{Lu}?\p{Ll}+[0-9]*|\p{Lu}(?=\p{Lu}\p{Ll})|\p{Lu}(?=[\s_-]|$)|[0-9]+", cleaned);
    if parts.is_empty() {
        let words: Vec<String> = cleaned.split(|c: char| c.is_whitespace() || c == '_' || c == '-').filter(|word| !word.is_empty()).map(str::to_lowercase).collect();
        return words.join("-");
    }
    parts.iter().map(|part| part.to_lowercase()).collect::<Vec<_>>().join("-")
}

/// `_toSnakeCase`.
fn snake_case(value: &str) -> String {
    let split = Regex::new(r"(\p{Ll})(\p{Lu})").expect("constant pattern").replace_all(value, |captures: &onig::Captures| format!("{}_{}", captures.at(1).unwrap_or_default(), captures.at(2).unwrap_or_default()));
    Regex::new(r"[\s\-]+").expect("constant pattern").replace_all(&split, "_").to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transform(text: &str) -> Transform {
        let chars: Vec<char> = text.chars().collect();
        let mut position = 0;
        let transform = parse(&chars, &mut position).expect("parses");
        assert_eq!(position, chars.len(), "consumed up to and with the closing brace");
        transform
    }

    #[test]
    fn format_strings_resolve_like_upstream() {
        // snippetParser.test.ts `Parser, TM text`, `FormatString` and `Transform -> FormatString#resolve`.
        assert_eq!(transform("(.*)/${1:/upcase}/}").resolve("foo"), "FOO");
        assert_eq!(transform("(.*)/${1:/downcase}/}").resolve("FOO"), "foo");
        assert_eq!(transform("(.*)/${1:/capitalize}/}").resolve("bar"), "Bar");
        assert_eq!(transform("(.*)/${1:/pascalcase}/}").resolve("bar-foo"), "BarFoo");
        assert_eq!(transform("(.*)/${1:/camelcase}/}").resolve("bar-foo"), "barFoo");
        assert_eq!(transform("(.*)/${1:/kebabcase}/}").resolve("barFooBaz"), "bar-foo-baz");
        assert_eq!(transform("(.*)/${1:/kebabcase}/}").resolve("XMLHttpRequest"), "xml-http-request");
        assert_eq!(transform("(.*)/${1:/snakecase}/}").resolve("barFoo baz"), "bar_foo_baz");
        assert_eq!(transform("(.*)/${1:+yes}/}").resolve("x"), "yes");
        assert_eq!(transform("(.*)/${1:?yes:no}/}").resolve(""), "no");
        assert_eq!(transform("(x)?/${1:-else}/}").resolve(""), "else");
    }

    #[test]
    fn the_regex_replaces_the_first_match_or_every_one_with_g() {
        assert_eq!(transform("a/b/}").resolve("aaa"), "baa");
        assert_eq!(transform("a/b/g}").resolve("aaa"), "bbb");
        assert_eq!(transform("A/b/gi}").resolve("aXa"), "bXb");
        assert_eq!(transform(r"\//\\/g}").resolve("a/b"), r"a\b");
        assert_eq!(transform(r"(.*)\..+$/$1/}").resolve("main.test.ts"), "main.test");
        let chars: Vec<char> = "(/x/}".chars().collect();
        assert_eq!(parse(&chars, &mut 0), None, "an invalid regex leaves the text as is");
    }
}
