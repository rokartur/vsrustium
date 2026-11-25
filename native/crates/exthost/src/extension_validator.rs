//! extensionValidator.ts: `engines.vscode` ranges and the manifest checks an installed extension must pass.

use serde_json::Value;

#[derive(Debug, PartialEq)]
struct ParsedVersion {
    has_caret: bool,
    has_greater_equals: bool,
    major_base: u64,
    major_must_equal: bool,
    minor_base: u64,
    minor_must_equal: bool,
    patch_base: u64,
    patch_must_equal: bool,
    pre_release: Option<String>,
}

#[derive(Debug, PartialEq)]
struct NormalizedVersion {
    major_base: u64,
    major_must_equal: bool,
    minor_base: u64,
    minor_must_equal: bool,
    patch_base: u64,
    patch_must_equal: bool,
    /// Milliseconds since the epoch, or 0.
    not_before: i64,
    is_minimum: bool,
}

/// `VERSION_REGEXP`: `^(\^|>=)?((\d+)|x)\.((\d+)|x)\.((\d+)|x)(\-.*)?$` after trimming.
fn parse_version(version: &str) -> Option<ParsedVersion> {
    let version = version.trim();
    if version == "*" {
        return Some(ParsedVersion { has_caret: false, has_greater_equals: false, major_base: 0, major_must_equal: false, minor_base: 0, minor_must_equal: false, patch_base: 0, patch_must_equal: false, pre_release: None });
    }
    let (prefix, rest) = match (version.strip_prefix('^'), version.strip_prefix(">=")) {
        (Some(rest), _) => ("^", rest),
        (_, Some(rest)) => (">=", rest),
        _ => ("", version),
    };
    let (numbers, pre_release) = match rest.find('-') {
        Some(dash) => (&rest[..dash], Some(rest[dash..].to_string())),
        None => (rest, None),
    };
    let parts: Vec<&str> = numbers.split('.').collect();
    let [major, minor, patch] = parts.as_slice() else { return None };
    let part = |text: &str| match text {
        "x" => Some((0, false)),
        _ if !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()) => text.parse().ok().map(|number| (number, true)),
        _ => None,
    };
    let ((major_base, major_must_equal), (minor_base, minor_must_equal), (patch_base, patch_must_equal)) = (part(major)?, part(minor)?, part(patch)?);
    Some(ParsedVersion { has_caret: prefix == "^", has_greater_equals: prefix == ">=", major_base, major_must_equal, minor_base, minor_must_equal, patch_base, patch_must_equal, pre_release })
}

fn normalize_version(version: ParsedVersion) -> NormalizedVersion {
    let (mut minor_must_equal, mut patch_must_equal) = (version.minor_must_equal, version.patch_must_equal);
    if version.has_caret {
        if version.major_base == 0 {
            patch_must_equal = false;
        } else {
            minor_must_equal = false;
            patch_must_equal = false;
        }
    }
    NormalizedVersion {
        major_base: version.major_base,
        major_must_equal: version.major_must_equal,
        minor_base: version.minor_base,
        minor_must_equal,
        patch_base: version.patch_base,
        patch_must_equal,
        not_before: version.pre_release.as_deref().and_then(not_before).unwrap_or(0),
        is_minimum: version.has_greater_equals,
    }
}

/// `NOT_BEFORE_REGEXP`: `-YYYYMMDD`, optionally with `HH` and `MM`, as `Date.UTC`.
fn not_before(pre_release: &str) -> Option<i64> {
    let digits = pre_release.strip_prefix('-')?;
    if !matches!(digits.len(), 8 | 10 | 12) || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let field = |range: std::ops::Range<usize>| digits.get(range).map_or(0, |text| text.parse::<i64>().expect("checked digits"));
    let (year, month, day, hours, minutes) = (field(0..4), field(4..6), field(6..8), field(8..10), field(10..12));
    // Days from civil (Howard Hinnant), which `Date.UTC` matches for months 1 to 12.
    let year_of_era = if month <= 2 { year - 1 } else { year };
    let era = year_of_era.div_euclid(400);
    let year_in_era = year_of_era - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let days = era * 146_097 + year_in_era * 365 + year_in_era / 4 - year_in_era / 100 + day_of_year - 719_468;
    Some(((days * 24 + hours) * 60 + minutes) * 60_000)
}

/// `isValidVersion` with a product version string; `product_date` is milliseconds since the epoch.
pub fn is_valid_version(product_version: &str, product_date: Option<i64>, desired_version: &str) -> bool {
    let (Some(version), Some(desired)) = (parse_version(product_version), parse_version(desired_version)) else { return false };
    matches_version(&normalize_version(version), product_date, &normalize_version(desired))
}

fn matches_version(version: &NormalizedVersion, product_date: Option<i64>, desired: &NormalizedVersion) -> bool {
    let (major, minor, patch) = (version.major_base, version.minor_base, version.patch_base);
    let (mut desired_major, mut desired_minor, mut desired_patch) = (desired.major_base, desired.minor_base, desired.patch_base);
    let (mut major_must_equal, mut minor_must_equal, mut patch_must_equal) = (desired.major_must_equal, desired.minor_must_equal, desired.patch_must_equal);
    let too_early = product_date.is_some_and(|date| date != 0 && date < desired.not_before);

    if desired.is_minimum {
        if major != desired_major {
            return major > desired_major;
        }
        if minor != desired_minor {
            return minor > desired_minor;
        }
        return !too_early && patch >= desired_patch;
    }

    // Anything < 1.0.0 is compatible with >= 1.0.0, except exact matches
    if major == 1 && desired_major == 0 && (!major_must_equal || !minor_must_equal || !patch_must_equal) {
        (desired_major, desired_minor, desired_patch) = (1, 0, 0);
        (major_must_equal, minor_must_equal, patch_must_equal) = (true, false, false);
    }
    if major != desired_major {
        return major > desired_major && !major_must_equal;
    }
    if minor != desired_minor {
        return minor > desired_minor && !minor_must_equal;
    }
    if patch != desired_patch {
        return patch > desired_patch && !patch_must_equal;
    }
    !too_early
}

/// `isValidExtensionVersion`: builtin and declarative extensions skip the check; the error is the notice.
pub fn is_valid_extension_version(product_version: &str, product_date: Option<i64>, manifest: &Value, is_builtin: bool) -> Result<(), String> {
    if is_builtin || (manifest.get("main").is_none() && manifest.get("browser").is_none()) {
        return Ok(());
    }
    is_version_valid(product_version, product_date, manifest["engines"]["vscode"].as_str().unwrap_or_default())
}

/// `isEngineValid`.
pub fn is_engine_valid(engine: &str, product_version: &str, product_date: Option<i64>) -> bool {
    engine == "*" || is_version_valid(product_version, product_date, engine).is_ok()
}

fn is_version_valid(product_version: &str, product_date: Option<i64>, requested: &str) -> Result<(), String> {
    let Some(desired) = parse_version(requested).map(normalize_version) else {
        return Err(format!("Could not parse `engines.vscode` value {requested}. Please use, for example: ^1.22.0, ^1.22.x, etc."));
    };
    if desired.major_base == 0 && (!desired.major_must_equal || !desired.minor_must_equal) {
        return Err(format!("Version specified in `engines.vscode` ({requested}) is not specific enough. For vscode versions before 1.0.0, please define at a minimum the major and minor desired version. E.g. ^0.10.0, 0.10.x, 0.11.0, etc."));
    }
    if desired.major_base != 0 && !desired.major_must_equal {
        return Err(format!("Version specified in `engines.vscode` ({requested}) is not specific enough. For vscode versions after 1.0.0, please define at a minimum the major desired version. E.g. ^1.10.0, 1.10.x, 1.x.x, 2.x.x, etc."));
    }
    let version = parse_version(product_version).map(normalize_version);
    if !version.is_some_and(|version| matches_version(&version, product_date, &desired)) {
        return Err(format!("Extension is not compatible with Code {product_version}. Extension requires: {requested}."));
    }
    Ok(())
}

/// `validateExtensionManifest`: the first error, as upstream stops at it. Warnings (a `main` outside the folder,
/// `extensionKind` without `main`) do not make an extension invalid and are not reported.
pub fn validate_extension_manifest(product_version: &str, product_date: Option<i64>, manifest: &Value, is_builtin: bool) -> Result<(), String> {
    let mandatory = |key: &str, kind: &str| format!("property `{key}` is mandatory and must be of type `{kind}`");
    let string_array = |key: &str| manifest[key].as_array().is_some_and(|items| items.iter().all(Value::is_string));
    if manifest.get("publisher").is_some_and(|publisher| !publisher.is_string()) {
        return Err("property publisher must be of type `string`.".into());
    }
    if !manifest["name"].is_string() {
        return Err(mandatory("name", "string"));
    }
    if !manifest["version"].is_string() {
        return Err(mandatory("version", "string"));
    }
    if manifest.get("engines").is_none_or(|engines| engines.is_null()) {
        return Err(mandatory("engines", "object"));
    }
    if !manifest["engines"]["vscode"].is_string() {
        return Err(mandatory("engines.vscode", "string"));
    }
    for key in ["extensionDependencies", "extensionAffinity"] {
        if manifest.get(key).is_some() && !string_array(key) {
            return Err(format!("property `{key}` can be omitted or must be of type `string[]`"));
        }
    }
    if manifest.get("activationEvents").is_some() {
        if !string_array("activationEvents") {
            return Err("property `activationEvents` can be omitted or must be of type `string[]`".into());
        }
        if manifest.get("main").is_none() && manifest.get("browser").is_none() {
            return Err("property `activationEvents` should be omitted if the extension doesn't have a `main` or `browser` property.".into());
        }
    }
    for key in ["main", "browser"] {
        if manifest.get(key).is_some_and(|entry| !entry.is_string()) {
            return Err(format!("property `{key}` can be omitted or must be of type `string`"));
        }
    }
    if !is_semver(manifest["version"].as_str().expect("checked above")) {
        return Err("Extension version is not semver compatible.".into());
    }
    is_valid_extension_version(product_version, product_date, manifest, is_builtin)
}

/// node-semver `valid`, which takes a leading `v` or `=`.
pub fn is_semver(version: &str) -> bool {
    semver_version(version).is_some()
}

pub fn semver_version(version: &str) -> Option<semver::Version> {
    let version = version.trim();
    let version = version.strip_prefix('=').unwrap_or(version);
    semver::Version::parse(version.strip_prefix('v').unwrap_or(version)).ok()
}

#[cfg(test)]
mod tests;
