//! hostService.ts `reload` and opening a workspace in the same window. The port has one window per process, so a
//! reload relaunches the app with the window's arguments: a new extension host, settings, trust and storage, as the
//! reference's renderer reload gives them.

use std::path::PathBuf;

use gpui::{Context, Window};

use super::Workbench;
use crate::commands::Outcome;

impl Workbench {
    pub(super) fn register_host_commands(&mut self) {
        self.commands.register("workbench.action.reloadWindow", |workbench, _, window, cx| {
            let folder = workbench.workspace_location();
            workbench.relaunch(folder, window, cx)
        });
    }

    /// Closes the editors, asking to save dirty ones, then starts the app on `folder` and quits this one.
    pub(super) fn relaunch(&mut self, folder: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let closed = self.close_all_editors(window, cx);
        cx.spawn(async move |workbench, cx| {
            if let Outcome::Pending(pending) = closed {
                let _ = pending.await;
            }
            let _ = workbench.update(cx, |workbench, cx| {
                if !workbench.editors.editors.is_empty() {
                    return;
                }
                match open_new_window(folder) {
                    Ok(()) => cx.quit(),
                    Err(error) => workbench.notify_error(&format!("Failed to reload the window: {error}")),
                }
            });
        })
        .detach();
        Outcome::done()
    }
}

/// Starts another app process on `folder` (or a file, for an empty window with it) with this one's flags: the port's
/// window of its own.
pub(super) fn open_new_window(folder: Option<PathBuf>) -> std::io::Result<()> {
    let mut args: Vec<String> = std::env::args().collect();
    for index in crate::positional_indexes(&args).into_iter().rev() {
        args.remove(index);
    }
    args.extend(folder.map(|folder| folder.to_string_lossy().into_owned()));
    std::process::Command::new(std::env::current_exe()?).args(&args[1..]).spawn()?;
    Ok(())
}
