//! shellEnv.ts: the login shell's environment, resolved once at startup for a launch outside a terminal.

use std::collections::HashMap;
use std::io::Read;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const TIMEOUT_ERROR: &str = "Unable to resolve your shell environment in a reasonable time. Please review your shell configuration and restart.";

/// `getResolvedShellEnv`: None when it is skipped (launched from the CLI: `VSCODE_CLI` set in `env`, as `isLaunchedFromCli`; or `force_disable`), else the login shell's environment.
/// `env` stands in for upstream's `process.env`: the child inherits it and the `ELECTRON_*` values are restored from it.
pub fn resolve_shell_env(
    env: &HashMap<String, String>,
    force_disable: bool,
    timeout: Duration,
) -> Option<Result<HashMap<String, String>, String>> {
    if force_disable || env.get("VSCODE_CLI").map(String::as_str) == Some("1") {
        return None;
    }
    Some(do_resolve_unix_shell_env(env, Instant::now() + timeout))
}

/// `application.shellEnvironmentResolutionTimeout` in seconds, clamped to 1..120; 10 s when it is not a number.
pub fn resolution_timeout(configured: Option<serde_json::Value>) -> Duration {
    match configured.as_ref().and_then(serde_json::Value::as_f64) {
        Some(seconds) => Duration::from_secs_f64(seconds.clamp(1.0, 120.0)),
        None => Duration::from_secs(10),
    }
}

/// `doResolveUnixShellEnv`, including the timeout and error wrapping of `getResolvedShellEnv`.
fn do_resolve_unix_shell_env(
    env: &HashMap<String, String>,
    deadline: Instant,
) -> Result<HashMap<String, String>, String> {
    let mark = generate_mark();
    let mut child_env = env.clone();
    child_env.insert("ELECTRON_RUN_AS_NODE".into(), "1".into());
    child_env.insert("ELECTRON_NO_ATTACH_CONSOLE".into(), "1".into());
    child_env.insert("VSCODE_RESOLVING_ENVIRONMENT".into(), "1".into());

    let shell = system_shell(&child_env);
    let name = Path::new(&shell)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (shell_args, command) = shell_command(&name, &mark);

    let mut cmd = Command::new(&shell);
    cmd.args(shell_args)
        .arg(command)
        .env_clear()
        .envs(&child_env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // Node's `detached: true`: a new session, so an interactive shell cannot take over or stop on our terminal.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = cmd.spawn().map_err(|err| unable_to_resolve(&err.to_string()))?;

    let mut stdout = child.stdout.take().expect("stdout is piped");
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut raw = Vec::new();
        let _ = stdout.read_to_end(&mut raw);
        let _ = sender.send(raw);
    });

    let Ok(raw) = receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(TIMEOUT_ERROR.into());
    };
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(TIMEOUT_ERROR.into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(err) => return Err(unable_to_resolve(&err.to_string())),
        }
    };

    if status.code() != Some(0) {
        let code = status.code().map_or("null".into(), |code| code.to_string());
        let signal = status.signal().map_or("null".into(), |signal| signal.to_string());
        return Err(unable_to_resolve(&format!(
            "Unexpected exit code from spawned shell (code {code}, signal {signal})"
        )));
    }

    let mut resolved = parse_marked_env(&String::from_utf8_lossy(&raw), &mark);
    for key in ["ELECTRON_RUN_AS_NODE", "ELECTRON_NO_ATTACH_CONSOLE"] {
        match env.get(key) {
            Some(value) if !value.is_empty() => resolved.insert(key.into(), value.clone()),
            _ => resolved.remove(key),
        };
    }
    resolved.remove("VSCODE_RESOLVING_ENVIRONMENT");
    // https://github.com/microsoft/vscode/issues/22593#issuecomment-336050758
    resolved.remove("XDG_RUNTIME_DIR");
    Ok(resolved)
}

fn unable_to_resolve(message: &str) -> String {
    format!("Unable to resolve your shell environment: {message}")
}

/// `generateUuid().replace(/-/g, '').substr(0, 12)`: 12 hex characters.
fn generate_mark() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(Instant::now().elapsed().as_nanos() ^ u128::from(std::process::id()));
    format!("{:012x}", hasher.finish() & 0xffff_ffff_ffff)
}

/// `getSystemShellUnixLike`: `$SHELL`, else the passwd entry, else `sh`; `/bin/false` becomes `/bin/bash`.
fn system_shell(env: &HashMap<String, String>) -> String {
    let shell = env
        .get("SHELL")
        .filter(|shell| !shell.is_empty())
        .cloned()
        .or_else(passwd_shell)
        .unwrap_or_else(|| "sh".into());
    if shell == "/bin/false" { "/bin/bash".into() } else { shell }
}

/// `os.userInfo().shell`.
fn passwd_shell() -> Option<String> {
    // getpwuid's static buffer is read before any other passwd call can overwrite it.
    unsafe {
        let entry = libc::getpwuid(libc::getuid());
        if entry.is_null() || (*entry).pw_shell.is_null() {
            return None;
        }
        let shell = std::ffi::CStr::from_ptr((*entry).pw_shell).to_string_lossy().into_owned();
        (!shell.is_empty()).then_some(shell)
    }
}

/// The shell branches of `doResolveUnixShellEnv`. Upstream runs Electron as node to print
/// `JSON.stringify(process.env)`; the port has no node, so it prints `env -0` (or the shell's native env) between the marks.
fn shell_command(name: &str, mark: &str) -> (&'static [&'static str], String) {
    let is_pwsh = matches!(
        name,
        "pwsh" | "powershell" | "pwsh-preview" | "powershell-preview"
    );
    if is_pwsh {
        let command = format!(
            "[Console]::Out.Write('{mark}'); Get-ChildItem env: | ForEach-Object {{ [Console]::Out.Write($_.Name + '=' + $_.Value + [char]0) }}; [Console]::Out.Write('{mark}')"
        );
        return (&["-Login", "-Command"], command);
    }
    match name {
        "nu" => (
            &["-i", "-l", "-c"],
            format!("^/bin/sh -c 'printf {mark}; command env -0; printf {mark}'"),
        ),
        "xonsh" => (
            &["-i", "-l", "-c"],
            format!(
                "import os, sys; sys.stdout.write(\"{mark}\" + \"\".join(k + \"=\" + v + \"\\0\" for k, v in os.environ.items()) + \"{mark}\")"
            ),
        ),
        "tcsh" | "csh" => (&["-ic"], format!("printf {mark}; env -0; printf {mark}")),
        _ => (
            &["-i", "-l", "-c"],
            format!("printf {mark}; command env -0; printf {mark}"),
        ),
    }
}

/// The `mark(...)mark` match of `doResolveUnixShellEnv`, over NUL-separated `KEY=VALUE` instead of JSON; no marks gives an empty map as upstream's `'{}'`.
fn parse_marked_env(raw: &str, mark: &str) -> HashMap<String, String> {
    let mut env = HashMap::new();
    let Some(start) = raw.find(mark).map(|index| index + mark.len()) else {
        return env;
    };
    let Some(end) = raw[start..].rfind(mark).map(|index| start + index) else {
        return env;
    };
    for entry in raw[start..end].split('\0') {
        if let Some((key, value)) = entry.split_once('=') {
            env.insert(key.to_string(), value.to_string());
        }
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(key, value)| (key.to_string(), value.to_string())).collect()
    }

    fn script_shell(name: &str, body: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("shell_env_test_{}_{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn parses_marked_nul_separated_output() {
        let raw = "motd\n\0junk=1\0abc123A=1\0B=x=y\0C=line1\nline2\0\0abc123trailing\0Z=9";
        let env = parse_marked_env(raw, "abc123");
        assert_eq!(env, env_of(&[("A", "1"), ("B", "x=y"), ("C", "line1\nline2")]));
    }

    #[test]
    fn missing_marks_parse_as_empty() {
        assert!(parse_marked_env("A=1\0", "abc123").is_empty());
        assert!(parse_marked_env("abc123A=1\0", "abc123").is_empty());
    }

    #[test]
    fn resolution_timeout_clamps_the_setting() {
        use serde_json::json;
        assert_eq!(resolution_timeout(None), Duration::from_secs(10));
        assert_eq!(resolution_timeout(Some(json!("30"))), Duration::from_secs(10));
        assert_eq!(resolution_timeout(Some(json!(30))), Duration::from_secs(30));
        assert_eq!(resolution_timeout(Some(json!(0))), Duration::from_secs(1));
        assert_eq!(resolution_timeout(Some(json!(500))), Duration::from_secs(120));
        assert_eq!(resolution_timeout(Some(json!(1.5))), Duration::from_millis(1500));
    }

    #[test]
    fn skips_when_launched_from_cli_or_force_disabled() {
        let timeout = Duration::from_secs(5);
        assert!(resolve_shell_env(&env_of(&[("VSCODE_CLI", "1")]), false, timeout).is_none());
        assert!(resolve_shell_env(&env_of(&[]), true, timeout).is_none());
    }

    #[test]
    fn args_per_shell_name() {
        for name in ["bash", "zsh", "sh", "fish", "nu", "xonsh"] {
            assert_eq!(shell_command(name, "m").0, ["-i", "-l", "-c"], "{name}");
        }
        assert_eq!(shell_command("tcsh", "m").0, ["-ic"]);
        assert_eq!(shell_command("csh", "m").0, ["-ic"]);
        assert_eq!(shell_command("pwsh", "m").0, ["-Login", "-Command"]);
        assert_eq!(shell_command("powershell-preview", "m").0, ["-Login", "-Command"]);
        assert_eq!(shell_command("zsh", "m").1, "printf m; command env -0; printf m");
    }

    #[test]
    fn system_shell_follows_get_system_shell() {
        assert_eq!(system_shell(&env_of(&[("SHELL", "/bin/zsh")])), "/bin/zsh");
        assert_eq!(system_shell(&env_of(&[("SHELL", "/bin/false")])), "/bin/bash");
        assert!(!system_shell(&env_of(&[])).is_empty());
    }

    #[test]
    fn resolves_env_from_bin_sh() {
        let env = env_of(&[
            ("SHELL", "/bin/sh"),
            ("PATH", "/usr/bin:/bin:/shell-env-test"),
            ("HOME", &std::env::var("HOME").unwrap()),
            ("XDG_RUNTIME_DIR", "/run/user/1"),
            ("ELECTRON_RUN_AS_NODE", "keep"),
        ]);
        let resolved = resolve_shell_env(&env, false, Duration::from_secs(10)).unwrap().unwrap();
        assert!(resolved["PATH"].contains("/usr/bin"), "{resolved:?}");
        assert_eq!(resolved["ELECTRON_RUN_AS_NODE"], "keep");
        assert!(!resolved.contains_key("ELECTRON_NO_ATTACH_CONSOLE"));
        assert!(!resolved.contains_key("VSCODE_RESOLVING_ENVIRONMENT"));
        assert!(!resolved.contains_key("XDG_RUNTIME_DIR"));
    }

    #[test]
    fn times_out_on_a_hanging_shell() {
        let shell = script_shell("hang", "sleep 5");
        let started = Instant::now();
        let result = resolve_shell_env(&env_of(&[("SHELL", &shell)]), false, Duration::from_millis(300));
        assert_eq!(result, Some(Err(TIMEOUT_ERROR.to_string())));
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn reports_non_zero_exit() {
        let shell = script_shell("fail", "exit 3");
        let result = resolve_shell_env(&env_of(&[("SHELL", &shell)]), false, Duration::from_secs(5));
        assert_eq!(
            result,
            Some(Err("Unable to resolve your shell environment: Unexpected exit code from spawned shell (code 3, signal null)".to_string()))
        );
    }
}
