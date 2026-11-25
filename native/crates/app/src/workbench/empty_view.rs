//! The Explorer of a window without a folder (emptyView.ts) with its welcome content (views_welcome.rs), and
//! `workbench.action.files.openFolder` (workspaceActions.ts).

use std::path::Path;

use gpui::{Context, Div, PathPromptOptions, Window, div};

use super::Workbench;
use crate::commands::Outcome;

const OPEN_FOLDER: &str = "workbench.action.files.openFolder";

impl Workbench {
    pub(super) fn register_empty_view_actions(&mut self) {
        self.commands.register(OPEN_FOLDER, |_, _, window, cx| {
            let picked = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Open".into()) });
            cx.spawn_in(window, async move |workbench, cx| {
                let Ok(Ok(Some(paths))) = picked.await else { return };
                let _ = workbench.update(cx, |workbench, cx| workbench.open_folder(&paths[0], cx));
            })
            .detach();
            Outcome::done()
        });
    }

    /// `window.openFoldersInNewWindow` default: the folder replaces this window, which here is a new process with
    /// this one's flags.
    // shortcut: with unsaved editors this window stays open, since hot exit backups are milestone 11 item 5.
    fn open_folder(&mut self, folder: &Path, cx: &mut Context<Self>) {
        let args: Vec<String> = std::env::args().collect();
        let mut flags = Vec::new();
        for (index, arg) in args.iter().enumerate().skip(1) {
            if arg.starts_with("--") || crate::VALUE_FLAGS.contains(&args[index - 1].as_str()) {
                flags.push(arg);
            }
        }
        let relaunched = std::process::Command::new(std::env::current_exe().expect("the running binary")).args(flags).arg(folder).spawn();
        if let Err(error) = relaunched {
            self.notify_error(format!("Unable to open '{}': {error}", folder.display()));
            return cx.notify();
        }
        if !self.editors.models.iter().any(|model| model.is_dirty()) {
            cx.quit();
        }
    }

    /// The body of the Explorer's first pane without a folder: its welcome content (viewPane.ts `WelcomeView`).
    pub(super) fn empty_view(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        self.view_welcome(super::views_welcome::EXPLORER_EMPTY_VIEW, window, cx).unwrap_or_else(div)
    }
}
