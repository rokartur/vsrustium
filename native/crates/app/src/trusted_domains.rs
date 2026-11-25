//! platform/url/common/trustedDomains.ts with the glob matching of urlGlob.ts: whether a link opens without the
//! link protection prompt. `new URL()` steps of upstream are the `url` crate, which implements the same WHATWG parser.
//! Where upstream would throw on a malformed URI or pattern, the link here is not trusted.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;

use crate::uri::{Uri, encode_uri};

#[cfg(test)]
mod tests;

/// `TRUSTED_DOMAINS_STORAGE_KEY`: the user's list, JSON in application storage.
pub const STORAGE_KEY: &str = "http.linkProtectionTrustedDomains";
/// `TRUSTED_DOMAINS_CONTENT_STORAGE_KEY`: the text of the Trusted Domains editor as last saved.
pub const CONTENT_STORAGE_KEY: &str = "http.linkProtectionTrustedDomainsContent";

static LOCALHOST: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^(.+\.)?localhost(:\d+)?$").expect("valid"));
static LOOPBACK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^127\.0\.0\.1(:\d+)?$").expect("valid"));
static IPV6_LOOPBACK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\[::1\]|\[0:0:0:0:0:0:0:1\])(:\d+)?$").expect("valid"));
static HAS_SCHEME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[^./:]*://").expect("valid"));
static ESCAPE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)%[0-9a-f]{2}").expect("valid"));
static NON_ASCII_ESCAPE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)%[89a-f][0-9a-f]").expect("valid"));
static ASCII_ESCAPE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)%[0-7][0-9a-f]").expect("valid"));

/// `isURLDomainTrusted`: schemes must match, `*.` matches subdomains, a path scopes to itself and below; localhost
/// is always trusted and `*` trusts everything.
pub fn is_url_domain_trusted(url: &Uri, trusted_domains: &[String]) -> bool {
    if trusted_domains.iter().any(|domain| domain == "*") {
        return true;
    }
    if !is_url_safe_for_trust(url) {
        return false;
    }
    let Some(url) = normalize_url_for_trust(url) else { return false };
    if !is_url_safe_for_trust(&url) {
        return false;
    }
    if is_localhost_authority(&url.authority) {
        return true;
    }
    trusted_domains.iter().any(|domain| normalize_url_pattern(&domain.replace('\\', "/")).is_some_and(|pattern| test_url_matches_glob(&url, &pattern)))
}

fn is_url_safe_for_trust(url: &Uri) -> bool {
    !url.authority.is_empty() && !url.authority.contains('@')
}

pub fn is_localhost_authority(authority: &str) -> bool {
    LOCALHOST.is_match(authority) || LOOPBACK.is_match(authority) || IPV6_LOOPBACK.is_match(authority)
}

fn is_http(url: &Uri) -> bool {
    url.scheme.eq_ignore_ascii_case("http") || url.scheme.eq_ignore_ascii_case("https")
}

/// `normalizeURLForTrust`: browser path separators and dot segments, a lowercase host, and GitHub's
/// case-insensitive paths lowercased.
fn normalize_url_for_trust(url: &Uri) -> Option<Uri> {
    let parsed = normalize_url_path_separators(url)?;
    let host_offset = parsed.authority.find('@').map_or(0, |at| at + 1);
    let authority = format!("{}{}", &parsed.authority[..host_offset], parsed.authority[host_offset..].to_lowercase());
    let mut path = parsed.path.clone();
    if authority == "github.com" {
        path = if is_http(&parsed) { lowercase_url_path(&path)? } else { path.to_lowercase() };
    }
    parsed
        .with(|uri| {
            uri.authority = authority;
            uri.path = path;
        })
        .ok()
}

fn lowercase_url_path(path: &str) -> Option<String> {
    let unreserved = decode_unreserved_url_path_characters(path);
    if !NON_ASCII_ESCAPE.is_match(&unreserved) {
        return Some(unreserved.to_lowercase());
    }
    let protected = ASCII_ESCAPE.replace_all(&unreserved, |escape: &regex::Captures| format!("%25{}", &escape[0][1..]));
    Some(Uri::parse(&format!("https://url.invalid{protected}")).ok()?.path.to_lowercase())
}

/// `normalizeURLPattern`: a configured pattern normalized like the URLs it is tested against; one without a
/// scheme stays without one.
fn normalize_url_pattern(pattern: &str) -> Option<String> {
    let has_scheme = HAS_SCHEME.is_match(pattern);
    let prefix = "https://";
    let uri = Uri::parse(&if has_scheme { pattern.to_owned() } else { format!("{prefix}{pattern}") }).ok()?;
    let normalized = normalize_url_for_trust(&uri)?;
    let serialized = normalized
        .with(|uri| {
            uri.authority = uri.authority.replace('%', "%25");
            uri.path = uri.path.replace('%', "%25");
        })
        .ok()?
        .to_string(true);
    Some(if has_scheme { serialized } else { serialized[prefix.len()..].to_owned() })
}

/// urlGlob.ts `normalizeURLAuthorityAndPath`: an HTTP authority with a slash, backslash, tab or newline ends where the
/// browser ends it.
fn normalize_url_authority_and_path(url: &Uri) -> Option<Uri> {
    if !is_http(url) || !url.authority.contains(['\\', '/', '\t', '\r', '\n']) {
        return Some(url.clone());
    }
    let serialized = url
        .with(|uri| {
            uri.query.clear();
            uri.fragment.clear();
        })
        .ok()?
        .to_string(true)
        .replace(['\t', '\r', '\n'], "");
    let authority_and_path = serialized[url.scheme.len() + 3..].replace('\\', "/");
    let authority_and_path = authority_and_path.trim_start_matches('/');
    let (authority, path) = authority_and_path.split_at(authority_and_path.find('/').unwrap_or(authority_and_path.len()));
    url.with(|uri| {
        uri.authority = authority.to_owned();
        uri.path = path.to_owned();
    })
    .ok()
}

/// `normalizeURLPathSeparators`: the path the browser requests, dot segments resolved.
fn normalize_url_path_separators(url: &Uri) -> Option<Uri> {
    if url.authority.is_empty() || !is_http(url) {
        return Some(url.clone());
    }
    let normalized = normalize_url_authority_and_path(url)?;
    if normalized.authority.is_empty() {
        return Some(normalized);
    }
    // A fixed authority preserves glob syntax such as wildcard hosts and ports.
    let fixed = normalized.with(|uri| uri.authority = "url.invalid".into()).ok()?.to_string(true);
    let path = url::Url::parse(&fixed).ok()?.path().to_owned();
    normalized.with(|uri| uri.path = path).ok()
}

/// urlGlob.ts `normalizeURL`: no trailing slashes, query or fragment; `resolve_path` resolves an HTTP path's dot
/// segments as the browser does.
fn normalize_url(url: &Uri, resolve_path: bool) -> Option<Uri> {
    let uri = normalize_url_authority_and_path(url)?;
    let mut path = uri.path.clone();
    if resolve_path && is_http(&uri) {
        let mut path_url = url::Url::parse("http://localhost").expect("valid");
        path_url.set_path(&encode_uri(&path.replace(['\t', '\n', '\r'], "").replace('\\', "/")));
        path = Uri::parse(path_url.as_str()).ok()?.path;
    }
    uri.with(|uri| {
        uri.path = path.trim_end_matches('/').to_owned();
        uri.query.clear();
        uri.fragment.clear();
    })
    .ok()
}

fn encode_url_path_for_matching(url: &Uri) -> String {
    if !is_http(url) {
        return url.path.clone();
    }
    // Encode decoded path characters without decoding or double-encoding existing escapes.
    decode_unreserved_url_path_characters(&encode_uri(&url.path).replace("%25", "%").replace('?', "%3F").replace('#', "%23"))
}

/// `decodeUnreservedURLPathCharacters`: unreserved characters decoded once, other escapes uppercased.
fn decode_unreserved_url_path_characters(path: &str) -> String {
    ESCAPE
        .replace_all(path, |escape: &regex::Captures| {
            let character = u8::from_str_radix(&escape[0][1..], 16).expect("two hex digits") as char;
            match character.is_ascii_alphanumeric() || "._~-".contains(character) {
                true => character.to_string(),
                false => escape[0].to_uppercase(),
            }
        })
        .into_owned()
}

/// `testUrlMatchesGlob`: a glob with `*` wildcards, `*.` subdomains and `:*` ports; HTTP paths must match both
/// before and after dot-segment resolution.
pub fn test_url_matches_glob(url: &Uri, glob_url: &str) -> bool {
    let Some(url) = normalize_url(url, false) else { return false };
    let glob = match HAS_SCHEME.is_match(glob_url) {
        true => glob_url.to_owned(),
        // A glob without a scheme assumes http or https.
        false if is_http(&url) => format!("{}://{glob_url}", url.scheme),
        false => return false,
    };
    let Some(glob) = Uri::parse(&glob).ok().and_then(|glob| normalize_url(&glob, false)) else { return false };
    if !do_url_part_match(&url.scheme, &glob.scheme, false) || !do_url_authority_match(&url.authority, &glob.authority, is_http(&url)) {
        return false;
    }
    if glob.path == "/" {
        return true;
    }
    let path = encode_url_path_for_matching(&url);
    let glob_path = encode_url_path_for_matching(&glob);
    if !do_url_part_match(&path, &glob_path, false) {
        return false;
    }
    let Some(resolved) = normalize_url(&url, true) else { return false };
    let resolved_path = encode_url_path_for_matching(&resolved);
    resolved_path == path || do_url_part_match(&resolved_path, &glob_path, false)
}

fn do_url_authority_match(authority: &str, glob_authority: &str, decode_percent_encoding: bool) -> bool {
    if do_url_part_match(authority, glob_authority, true) {
        return true;
    }
    let normalized = normalize_authority_for_matching(authority, decode_percent_encoding);
    let normalized_glob = normalize_authority_for_matching(glob_authority, decode_percent_encoding);
    (normalized != authority || normalized_glob != glob_authority) && do_url_part_match(&normalized, &normalized_glob, true)
}

/// `normalizeAuthorityForMatching`: literal DNS labels as the browser spells them (lowercase, Punycode), wildcard
/// labels, ports and user information as written.
fn normalize_authority_for_matching(authority: &str, decode_percent_encoding: bool) -> String {
    let hostname_start = authority.rfind('@').map_or(0, |at| at + 1);
    if authority[hostname_start..].starts_with('[') {
        return authority.to_owned();
    }
    let hostname_end = authority[hostname_start..].find(':').map_or(authority.len(), |port| hostname_start + port);
    let labels: Vec<String> = authority[hostname_start..hostname_end]
        .split('.')
        .map(|label| {
            let literal = label.contains('*') || label.contains(['/', '\\', '?', '#']) || label.contains(char::is_whitespace) || (!decode_percent_encoding && label.contains('%'));
            if literal {
                return label.to_owned();
            }
            if label.chars().all(|character| character.is_ascii_alphanumeric() || character == '_' || character == '-') {
                return label.to_lowercase();
            }
            // A suffix prevents numeric labels from being interpreted as IPv4 addresses.
            let suffix = ".invalid";
            let parsed = url::Url::parse(&format!("http://{label}{suffix}")).ok();
            match parsed.as_ref().and_then(|parsed| parsed.host_str()).and_then(|host| host.strip_suffix(suffix)) {
                Some(normalized) if !normalized.contains('*') => normalized.to_owned(),
                _ => label.to_owned(),
            }
        })
        .collect();
    format!("{}{}{}", &authority[..hostname_start], labels.join("."), &authority[hostname_end..])
}

/// `doUrlPartMatch`: a breadth-first walk over (url offset, glob offset) states, so long parts cannot recurse deeply.
fn do_url_part_match(url_part: &str, glob_part: &str, include_port_logic: bool) -> bool {
    if url_part == glob_part {
        return true;
    }
    if !glob_part.contains('*') {
        return url_part.starts_with(&format!("{glob_part}/"));
    }
    let url: Vec<char> = url_part.chars().collect();
    let glob: Vec<char> = glob_part.chars().collect();
    // A port scan can end one past the end of the url.
    let mut pending: Vec<Vec<usize>> = vec![Vec::new(); url.len() + 2];
    let mut seen = HashSet::new();
    let mut add_state = |pending: &mut Vec<Vec<usize>>, url_offset: usize, glob_offset: usize| {
        if seen.insert((url_offset, glob_offset)) {
            pending[url_offset].push(glob_offset);
        }
    };
    add_state(&mut pending, 0, 0);
    for url_offset in 0..=url.len() {
        let mut index = 0;
        // States added at this offset while walking it are walked too, as a JavaScript Set iterates its additions.
        while index < pending[url_offset].len() {
            let glob_offset = pending[url_offset][index];
            index += 1;
            let any_port = include_port_logic && glob.get(glob_offset) == Some(&':') && glob.get(glob_offset + 1) == Some(&'*');
            if url_offset == url.len() {
                if glob_offset == glob.len() || (any_port && glob_offset + 2 == glob.len()) {
                    return true;
                }
                continue;
            }
            if glob_offset == glob.len() {
                if url[url_offset] == '/' {
                    return true;
                }
                continue;
            }
            if url[url_offset] == glob[glob_offset] {
                add_state(&mut pending, url_offset + 1, glob_offset + 1);
            }
            if glob[glob_offset] == '*' {
                if glob_offset + 1 == glob.len() {
                    return true;
                }
                if glob[glob_offset + 1] == '.' {
                    if !['/', ':'].contains(&url[url_offset]) {
                        add_state(&mut pending, url_offset + 1, glob_offset);
                    }
                    if url_offset == 0 || url[url_offset - 1] == '.' {
                        add_state(&mut pending, url_offset, glob_offset + 2);
                    }
                }
                let next_glob = if url_offset + 1 == url.len() { glob_offset + 1 } else { glob_offset };
                add_state(&mut pending, url_offset + 1, next_glob);
                add_state(&mut pending, url_offset, glob_offset + 1);
            }
            if any_port {
                let mut end_port = url_offset;
                if url[url_offset] == ':' {
                    end_port += 1;
                    loop {
                        end_port += 1;
                        if !url.get(end_port).is_some_and(char::is_ascii_digit) {
                            break;
                        }
                    }
                }
                add_state(&mut pending, end_port, glob_offset + 2);
            }
        }
    }
    false
}
