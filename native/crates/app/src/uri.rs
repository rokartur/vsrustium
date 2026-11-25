//! base/common/uri.ts: `URI.parse`, `with` and `toString` with the same decoding, validation and encoding.
//! Rust strings hold no unpaired UTF-16 surrogates, so the cases built on them have no counterpart here.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Uri {
    pub scheme: String,
    pub authority: String,
    pub path: String,
    pub query: String,
    pub fragment: String,
}

static URI_PARTS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(([^:/?#]+?):)?(//([^/?#]*))?([^?#]*)(\?([^#]*))?(#(.*))?").expect("valid"));
static SCHEME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\w[\w\d+.-]*$").expect("valid"));
static ENCODED_AS_HEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(%[0-9A-Za-z][0-9A-Za-z])+").expect("valid"));

impl Uri {
    /// The `Uri` constructor (not strict): an empty scheme becomes `file`, then `_referenceResolution` and `_validateUri`.
    pub fn new(scheme: &str, authority: &str, path: &str, query: &str, fragment: &str) -> Result<Uri, String> {
        let scheme = if scheme.is_empty() { "file" } else { scheme };
        let path = match scheme {
            "https" | "http" | "file" if !path.starts_with('/') => format!("/{path}"),
            _ => path.to_owned(),
        };
        if !SCHEME.is_match(scheme) {
            return Err(format!("[UriError]: Scheme contains illegal characters. (len:{})", scheme.chars().count()));
        }
        if !path.is_empty() {
            if !authority.is_empty() && !path.starts_with('/') {
                return Err(r#"[UriError]: If a URI contains an authority component, then the path component must either be empty or begin with a slash ("/") character"#.into());
            }
            if authority.is_empty() && path.starts_with("//") {
                return Err(r#"[UriError]: If a URI does not contain an authority component, then the path cannot begin with two slash characters ("//")"#.into());
            }
        }
        Ok(Uri { scheme: scheme.into(), authority: authority.into(), path, query: query.into(), fragment: fragment.into() })
    }

    pub fn parse(value: &str) -> Result<Uri, String> {
        let Some(parts) = URI_PARTS.captures(value) else { return Uri::new("", "", "", "", "") };
        let part = |index: usize| parts.get(index).map_or("", |part| part.as_str());
        Uri::new(part(2), &percent_decode(part(4)), &percent_decode(part(5)), &percent_decode(part(7)), &percent_decode(part(9)))
    }

    /// `UriComponents` as the extension host sends them.
    pub fn from_json(uri: &Value) -> Result<Uri, String> {
        let part = |key: &str| uri[key].as_str().unwrap_or_default();
        Uri::new(part("scheme"), part("authority"), part("path"), part("query"), part("fragment"))
    }

    pub fn to_json(&self) -> Value {
        json!({ "$mid": 1, "scheme": self.scheme, "authority": self.authority, "path": self.path, "query": self.query, "fragment": self.fragment })
    }

    /// `with`: the changed parts, validated again.
    pub fn with(&self, change: impl FnOnce(&mut Uri)) -> Result<Uri, String> {
        let mut changed = self.clone();
        change(&mut changed);
        Uri::new(&changed.scheme, &changed.authority, &changed.path, &changed.query, &changed.fragment)
    }

    /// `toString(skipEncoding)`: `_asFormatted`.
    pub fn to_string(&self, skip_encoding: bool) -> String {
        let encode = |component: &str, is_path: bool, is_authority: bool| match skip_encoding {
            false => encode_component(component, is_path, is_authority),
            true => encode_component_minimal(component),
        };
        let mut formatted = String::new();
        if !self.scheme.is_empty() {
            formatted.push_str(&self.scheme);
            formatted.push(':');
        }
        if !self.authority.is_empty() || self.scheme == "file" {
            formatted.push_str("//");
        }
        if !self.authority.is_empty() {
            let mut authority = self.authority.as_str();
            if let Some((userinfo, host)) = authority.split_once('@') {
                match userinfo.split_once(':') {
                    None => formatted.push_str(&encode(userinfo, false, false)),
                    Some((user, password)) => {
                        formatted.push_str(&encode(user, false, false));
                        formatted.push(':');
                        formatted.push_str(&encode(password, false, false));
                    }
                }
                formatted.push('@');
                authority = host;
            }
            let authority = authority.to_lowercase();
            match authority.rfind(':') {
                None => formatted.push_str(&encode(&authority, false, true)),
                Some(port) => {
                    formatted.push_str(&encode(&authority[..port], false, true));
                    formatted.push_str(&authority[port..]);
                }
            }
        }
        if !self.path.is_empty() {
            let mut path = self.path.clone();
            // HTTP paths are case-sensitive, even when their first segment resembles a drive letter.
            let is_http = matches!(self.scheme.to_lowercase().as_str(), "http" | "https");
            let bytes = path.as_bytes();
            if bytes.len() >= 3 && bytes[0] == b'/' && bytes[2] == b':' {
                if bytes[1].is_ascii_uppercase() && !is_http {
                    path = format!("/{}:{}", bytes[1].to_ascii_lowercase() as char, &path[3..]);
                }
            } else if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_uppercase() && !is_http {
                path = format!("{}:{}", bytes[0].to_ascii_lowercase() as char, &path[2..]);
            }
            formatted.push_str(&encode(&path, true, false));
        }
        if !self.query.is_empty() {
            formatted.push('?');
            formatted.push_str(&encode(&self.query, false, false));
        }
        if !self.fragment.is_empty() {
            formatted.push('#');
            match skip_encoding {
                false => formatted.push_str(&encode_component(&self.fragment, false, false)),
                true => formatted.push_str(&self.fragment),
            }
        }
        formatted
    }
}

/// `encodeURIComponentFast`: everything but the unreserved characters (and `/` in a path, `[]:` in an authority)
/// as `%XX` of its UTF-8 bytes, which is what both its table and `encodeURIComponent` give.
fn encode_component(component: &str, is_path: bool, is_authority: bool) -> String {
    let mut encoded = String::with_capacity(component.len());
    for character in component.chars() {
        let keep = character.is_ascii_alphanumeric()
            || matches!(character, '-' | '.' | '_' | '~')
            || (is_path && character == '/')
            || (is_authority && matches!(character, '[' | ']' | ':'));
        if keep {
            encoded.push(character);
            continue;
        }
        for byte in character.encode_utf8(&mut [0; 4]).bytes() {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// JavaScript's `encodeURI`.
pub fn encode_uri(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for character in text.chars() {
        if character.is_ascii_alphanumeric() || ";,/?:@&=+$-_.!~*'()#".contains(character) {
            encoded.push(character);
            continue;
        }
        for byte in character.encode_utf8(&mut [0; 4]).bytes() {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// `encodeURIComponentMinimal`: only `?` and `#`.
fn encode_component_minimal(component: &str) -> String {
    component.replace('?', "%3F").replace('#', "%23")
}

/// `percentDecode`: each run of `%XX` decoded as `decodeURIComponent` would, a malformed run kept as written.
pub fn percent_decode(text: &str) -> String {
    ENCODED_AS_HEX.replace_all(text, |run: &regex::Captures| decode_graceful(&run[0])).into_owned()
}

/// `decodeURIComponentGraceful`: on a malformed run, the first `%XX` stays as written and the rest is tried again.
fn decode_graceful(run: &str) -> String {
    let bytes: Option<Vec<u8>> = run.as_bytes().chunks(3).map(|escape| std::str::from_utf8(&escape[1..]).ok().and_then(|hex| u8::from_str_radix(hex, 16).ok())).collect();
    if let Some(text) = bytes.and_then(|bytes| String::from_utf8(bytes).ok()) {
        return text;
    }
    match run.len() > 3 {
        true => format!("{}{}", &run[..3], decode_graceful(&run[3..])),
        false => run.to_owned(),
    }
}
