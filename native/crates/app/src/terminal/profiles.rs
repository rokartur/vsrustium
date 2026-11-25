//! terminalProfiles.ts `detectAvailableProfiles` for macOS and terminalProfileResolverService.ts
//! `getDefaultProfile`: the shells a terminal can launch and the one a new terminal uses.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    pub name: String,
    pub path: PathBuf,
    pub args: Vec<String>,
    pub env: HashMap<String, Option<String>>,
    pub icon: Option<String>,
    pub color: Option<String>,
    pub override_name: bool,
    /// Found in /etc/shells, not configured.
    pub auto_detected: bool,
    /// `isFromPath`: the configured path was a bare name found on PATH.
    pub is_from_path: bool,
}

/// `detectAvailableUnixProfiles`: the shells of /etc/shells (named by basename, `name (2)` for the second
/// with that name), then `terminal.integrated.profiles.osx` over them; each kept only if its path exists.
pub fn detect(configured: &Value, shells_file: &str, env_path: Option<&str>) -> Vec<Profile> {
    let mut entries: Vec<(String, Value)> = Vec::new();
    let mut counts: HashMap<String, usize> = HashMap::new();
    for line in shells_file.split('\n') {
        let line = line.split('#').next().unwrap_or_default();
        if line.trim().is_empty() {
            continue;
        }
        let base = Path::new(line).file_name().map_or(line.to_owned(), |name| name.to_string_lossy().into_owned());
        let count = counts.entry(base.clone()).or_default();
        *count += 1;
        let name = if *count > 1 { format!("{base} ({count})") } else { base };
        entries.push((name, serde_json::json!({ "path": line, "isAutoDetected": true })));
    }
    // applyConfigProfilesToMap: null or a profile without a path removes it; a new one keeps the old icon.
    for (name, value) in configured.as_object().into_iter().flatten() {
        let position = entries.iter().position(|(known, _)| known == name);
        if !value.is_object() || value.get("path").is_none() {
            if let Some(position) = position {
                entries.remove(position);
            }
            continue;
        }
        let mut value = value.clone();
        if value.get("icon").is_none_or(Value::is_null) {
            if let Some(icon) = position.and_then(|position| entries[position].1.get("icon").cloned()) {
                value["icon"] = icon;
            }
        }
        match position {
            Some(position) => entries[position].1 = value,
            None => entries.push((name.clone(), value)),
        }
    }
    entries.into_iter().filter_map(|(name, value)| validate(name, &value, env_path)).collect()
}

/// `getValidatedProfile` and `validateProfilePaths`: the first of the profile's paths that exists, a bare name
/// looked up on PATH.
fn validate(name: String, value: &Value, env_path: Option<&str>) -> Option<Profile> {
    let paths: Vec<String> = match &value["path"] {
        Value::String(path) => vec![path.clone()],
        Value::Array(paths) => paths.iter().filter_map(|path| path.as_str().map(str::to_owned)).collect(),
        _ => return None,
    };
    let (path, is_from_path) = paths.iter().filter(|path| !path.is_empty()).find_map(|path| {
        if Path::new(path).file_name().is_some_and(|name| name == path.as_str()) {
            return find_executable(path, env_path).map(|found| (found, true));
        }
        Path::new(path).is_file().then(|| (PathBuf::from(path), false))
    })?;
    let args = match &value["args"] {
        Value::Array(args) => args.iter().filter_map(|arg| arg.as_str().map(str::to_owned)).collect(),
        _ => Vec::new(),
    };
    let env = value["env"].as_object().into_iter().flatten().map(|(key, value)| (key.clone(), value.as_str().map(str::to_owned))).collect();
    Some(Profile {
        name,
        path,
        args,
        env,
        icon: value["icon"].as_str().map(str::to_owned),
        color: value["color"].as_str().map(str::to_owned),
        override_name: value["overrideName"] == true,
        auto_detected: value["isAutoDetected"] == true,
        is_from_path,
    })
}

/// processes.ts `findExecutable` without a cwd: the first PATH entry holding the file.
pub fn find_executable(name: &str, env_path: Option<&str>) -> Option<PathBuf> {
    env_path?.split(':').map(|dir| Path::new(dir).join(name)).find(|candidate| candidate.is_file())
}

/// `getDefaultProfile`: the configured `terminal.integrated.defaultProfile.osx` if it names a configured profile,
/// else the profile of the system shell ($SHELL), else a generated one that launches a zsh or bash as a login
/// shell (`_getUnresolvedFallbackDefaultProfile`).
pub fn default_profile(profiles: &[Profile], default_name: Option<&str>, system_shell: &str) -> Profile {
    if let Some(profile) = default_name.and_then(|name| profiles.iter().find(|profile| profile.name == name && !profile.auto_detected)) {
        return profile.clone();
    }
    let stem = |path: &Path| path.file_stem().map(|stem| stem.to_string_lossy().into_owned());
    let shell_stem = stem(Path::new(system_shell));
    if let Some(profile) = profiles.iter().find(|profile| stem(&profile.path) == shell_stem) {
        return profile.clone();
    }
    let login = shell_stem.as_deref().is_some_and(|stem| stem.contains("zsh") || stem.contains("bash"));
    Profile {
        name: "Generated".into(),
        path: PathBuf::from(system_shell),
        args: if login { vec!["--login".into()] } else { Vec::new() },
        env: HashMap::new(),
        icon: None,
        color: None,
        override_name: false,
        auto_detected: false,
        is_from_path: false,
    }
}

/// shell.ts `getSystemShellUnixLike`: $SHELL, else the user's login shell, else `sh`; `/bin/false` means `/bin/bash`.
pub fn system_shell(env: &HashMap<String, String>) -> String {
    let shell = env.get("SHELL").cloned().filter(|shell| !shell.is_empty()).or_else(login_shell).unwrap_or_else(|| "sh".into());
    if shell == "/bin/false" { "/bin/bash".into() } else { shell }
}

fn login_shell() -> Option<String> {
    let entry = unsafe { libc::getpwuid(libc::getuid()) };
    if entry.is_null() || unsafe { (*entry).pw_shell.is_null() } {
        return None;
    }
    Some(unsafe { std::ffi::CStr::from_ptr((*entry).pw_shell) }.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn profiles_come_from_etc_shells_and_settings() {
        let dir = std::env::temp_dir().join(format!("code-gpui-profiles-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["bash", "zsh"] {
            std::fs::write(dir.join(name), "").unwrap();
        }
        let shells = format!("# comment\n{0}/bash\n{0}/zsh\n/missing/bash\n", dir.display());
        let configured = json!({ "zsh": { "path": "zsh", "args": ["-l"] }, "bash": null, "custom": { "path": ["/missing", format!("{}/bash", dir.display())] } });
        let profiles = detect(&configured, &shells, Some(&dir.display().to_string()));
        let names: Vec<_> = profiles.iter().map(|profile| (profile.name.as_str(), profile.auto_detected, profile.args.clone())).collect();
        assert_eq!(names, [("zsh", false, vec!["-l".to_owned()]), ("custom", false, vec![])]);

        assert_eq!(default_profile(&profiles, Some("custom"), "/bin/zsh").name, "custom");
        assert_eq!(default_profile(&profiles, None, "/bin/zsh").name, "zsh");
        let generated = default_profile(&profiles, None, "/opt/bin/bash");
        assert_eq!((generated.name.as_str(), generated.args.clone()), ("custom", vec![]));
        let generated = default_profile(&[], None, "/bin/bash");
        assert_eq!((generated.name.as_str(), generated.args), ("Generated", vec!["--login".to_owned()]));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
