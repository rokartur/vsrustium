//! platform/terminal/node/terminalEnvironment.ts `getShellIntegrationInjection` for macOS: the arguments and
//! environment that make bash, zsh, fish and pwsh load the shell integration scripts.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// What to change in the launch, as `IShellIntegrationConfigInjection`.
#[derive(Debug, PartialEq)]
pub struct Injection {
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    /// Files to copy before the spawn (`filesToCopy`): the zsh dotfiles into `ZDOTDIR`.
    pub files_to_copy: Vec<(PathBuf, PathBuf)>,
}

/// `None` where upstream answers `IShellIntegrationInjectionFailure`. `path_prefix` is the PATH the extensions
/// prepend (`addEnvMixinPathPrefix`), `zdotdir` the temp folder for zsh and `user_zdotdir` the user's own.
pub fn injection(executable: &Path, args: &[String], nonce: &str, scripts: &Path, path_prefix: &str, zdotdir: &Path, user_zdotdir: &str) -> Option<Injection> {
    let shell = executable.file_name()?.to_string_lossy().into_owned();
    let mut env = HashMap::from([("VSCODE_INJECTION".to_owned(), "1".to_owned())]);
    if !nonce.is_empty() {
        env.insert("VSCODE_NONCE".into(), nonce.into());
    }
    let add_path_prefix = |env: &mut HashMap<String, String>| {
        if !path_prefix.is_empty() {
            env.insert("VSCODE_PATH_PREFIX".into(), path_prefix.into());
        }
    };
    let scripts = scripts.to_string_lossy();
    // product.json of the port has no `quality`, so it is not 'stable'.
    let stable = ("VSCODE_STABLE".to_owned(), "0".to_owned());
    let mut files_to_copy = Vec::new();
    let new_args = match shell.as_str() {
        "bash" => {
            if !args.is_empty() {
                if !are_login_args(args) {
                    return None;
                }
                env.insert("VSCODE_SHELL_LOGIN".into(), "1".into());
                add_path_prefix(&mut env);
            }
            env.extend([stable]);
            vec!["--init-file".into(), format!("{scripts}/shellIntegration-bash.sh")]
        }
        "fish" => {
            let source = format!("source \"{scripts}/shellIntegration.fish\"");
            let new_args = if args.is_empty() {
                vec!["--init-command".into(), source]
            } else if are_login_args(args) {
                vec!["-l".into(), "--init-command".into(), source]
            } else {
                return None;
            };
            // On fish, '$fish_user_paths' is always prepended to the PATH, for both login and non-login shells.
            add_path_prefix(&mut env);
            new_args
        }
        "pwsh" => {
            let new_args = if args.is_empty() || are_pwsh_implied_args(args) {
                vec!["-noexit".into(), "-command".into(), format!(". \"{scripts}/shellIntegration.ps1\"")]
            } else if are_pwsh_login_args(args) {
                vec!["-l".into(), "-noexit".into(), "-command".into(), format!(". \"{scripts}/shellIntegration.ps1\"")]
            } else {
                return None;
            };
            env.extend([stable]);
            new_args
        }
        "zsh" => {
            let new_args = if args.is_empty() {
                vec!["-i".into()]
            } else if are_login_args(args) {
                add_path_prefix(&mut env);
                vec!["-il".into()]
            } else {
                return None;
            };
            env.insert("ZDOTDIR".into(), zdotdir.to_string_lossy().into_owned());
            env.insert("USER_ZDOTDIR".into(), user_zdotdir.into());
            for (source, dest) in [("shellIntegration-rc.zsh", ".zshrc"), ("shellIntegration-profile.zsh", ".zprofile"), ("shellIntegration-env.zsh", ".zshenv"), ("shellIntegration-login.zsh", ".zlogin")] {
                files_to_copy.push((Path::new(scripts.as_ref()).join(source), zdotdir.join(dest)));
            }
            new_args
        }
        _ => return None,
    };
    Some(Injection { args: new_args, env, files_to_copy })
}

/// `areZshBashFishLoginArgs`: `--login` or `-l`, interactive flags aside.
fn are_login_args(args: &[String]) -> bool {
    let rest: Vec<&String> = args.iter().filter(|arg| !["-i", "--interactive"].contains(&arg.to_lowercase().as_str())).collect();
    rest.len() == 1 && ["--login", "-l"].contains(&rest[0].to_lowercase().as_str())
}

fn are_pwsh_login_args(args: &[String]) -> bool {
    let login = |arg: &String| ["-login", "-l"].contains(&arg.to_lowercase().as_str());
    let implied = |arg: &String| ["-nol", "-nologo"].contains(&arg.to_lowercase().as_str());
    match args {
        [arg] => login(arg),
        [a, b] => (login(a) || login(b)) && (implied(a) || implied(b)),
        _ => false,
    }
}

fn are_pwsh_implied_args(args: &[String]) -> bool {
    args.is_empty() || args.len() == 1 && ["-nol", "-nologo"].contains(&args[0].to_lowercase().as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inject(executable: &str, args: &[&str]) -> Option<Injection> {
        let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
        injection(Path::new(executable), &args, "n", Path::new("/scripts"), "/prefix:", Path::new("/tmp/z"), "/home")
    }

    /// terminalEnvironment.test.ts `getShellIntegrationInjection` cases for bash and zsh on macOS.
    #[test]
    fn bash_gets_the_init_file_and_a_login_flag() {
        let plain = inject("/bin/bash", &[]).unwrap();
        assert_eq!(plain.args, ["--init-file", "/scripts/shellIntegration-bash.sh"]);
        assert_eq!(plain.env.get("VSCODE_SHELL_LOGIN"), None);
        assert_eq!(plain.env["VSCODE_NONCE"], "n");
        let login = inject("/bin/bash", &["-l"]).unwrap();
        assert_eq!(login.args, plain.args);
        assert_eq!(login.env["VSCODE_SHELL_LOGIN"], "1");
        assert_eq!(login.env["VSCODE_PATH_PREFIX"], "/prefix:");
        assert_eq!(inject("/bin/bash", &["-c", "echo"]), None);
        assert_eq!(inject("/bin/csh", &[]), None);
    }

    #[test]
    fn zsh_moves_its_dotfiles_into_zdotdir() {
        let login = inject("/bin/zsh", &["-l"]).unwrap();
        assert_eq!(login.args, ["-il"]);
        assert_eq!(login.env["ZDOTDIR"], "/tmp/z");
        assert_eq!(login.env["USER_ZDOTDIR"], "/home");
        assert_eq!(login.files_to_copy[0], (PathBuf::from("/scripts/shellIntegration-rc.zsh"), PathBuf::from("/tmp/z/.zshrc")));
        assert_eq!(inject("/bin/zsh", &[]).unwrap().args, ["-i"]);
        assert_eq!(inject("/bin/zsh", &["-i"]), None, "upstream only keeps its own argument arrays, by identity");
    }
}
