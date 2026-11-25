use std::path::{Path, PathBuf};

use crate::commands::Outcome;

use super::Workbench;

/// externalTerminal.ts `DEFAULT_TERMINAL_OSX`.
const DEFAULT_TERMINAL_OSX: &str = "Terminal.app";

impl Workbench {
    pub(super) fn register_external_terminal_actions(&mut self) {
        self.commands.register("workbench.action.terminal.openNativeConsole", |workbench, _, _, _| {
            workbench.open_external_terminal();
            Outcome::done()
        });
    }

    /// externalTerminal.contribution.ts `openNativeConsole`: the last active workspace root, else the active file's
    /// folder, else no cwd, which `open` turns into the home directory.
    // shortcut: no folder quick pick in multi-root workspaces, the active editor's folder is used; port it with a multi-root picker.
    fn open_external_terminal(&mut self) {
        let active = self.editors.active().filter(|editor| editor.document().scheme == "file").map(|editor| editor.document().path.clone());
        let root = self.workspace().and_then(|workspace| {
            let containing = active.as_ref().and_then(|file| workspace.folders.iter().find(|folder| file.starts_with(&folder.path)));
            containing.or(workspace.folders.first()).map(|folder| folder.path.clone())
        });
        let cwd = root.or_else(|| active.and_then(|file| file.parent().map(Path::to_path_buf)));
        let exec = self.setting("terminal.external.osxExec");
        let (program, args) = open_terminal_command(exec.as_str(), cwd.as_deref());
        let mut command = std::process::Command::new(program);
        command.args(&args);
        if let Some(cwd) = &cwd {
            command.current_dir(cwd);
        }
        for (key, _) in std::env::vars_os() {
            if key.to_str().is_some_and(crate::terminal::sanitized_away) {
                command.env_remove(key);
            }
        }
        match command.spawn() {
            Ok(mut child) => {
                std::thread::spawn(move || child.wait());
            }
            Err(error) => self.notify_error(error.to_string()),
        }
    }
}

/// externalTerminalService.ts `MacExternalTerminalService.spawnTerminal`: `/usr/bin/open -a <osxExec> [cwd]`.
fn open_terminal_command(osx_exec: Option<&str>, cwd: Option<&Path>) -> (&'static str, Vec<PathBuf>) {
    let terminal_app = osx_exec.filter(|exec| !exec.is_empty()).unwrap_or(DEFAULT_TERMINAL_OSX);
    let mut args = vec![PathBuf::from("-a"), PathBuf::from(terminal_app)];
    args.extend(cwd.map(Path::to_path_buf));
    ("/usr/bin/open", args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_terminal_command_defaults_to_terminal_app() {
        let (program, args) = open_terminal_command(None, Some(Path::new("/work")));
        assert_eq!(program, "/usr/bin/open");
        assert_eq!(args, [PathBuf::from("-a"), "Terminal.app".into(), "/work".into()]);
        assert_eq!(open_terminal_command(Some(""), None).1, [PathBuf::from("-a"), "Terminal.app".into()]);
    }

    #[test]
    fn open_terminal_command_uses_osx_exec() {
        let (_, args) = open_terminal_command(Some("iTerm.app"), None);
        assert_eq!(args, [PathBuf::from("-a"), "iTerm.app".into()]);
    }
}
