//! terminal.environmentChanges.contribution.ts: Show Environment Contributions opens the extensions' environment
//! changes as Markdown.

use std::path::PathBuf;

use super::Workbench;
use crate::commands::Outcome;
use crate::editor::TextDocument;

/// `EnvironmentCollectionProvider.scheme`.
const ENVIRONMENT_CHANGES_SCHEME: &str = "ENVIRONMENT_CHANGES_COLLECTION";

impl Workbench {
    pub(super) fn register_terminal_environment_actions(&mut self) {
        self.commands.register("workbench.action.terminal.showEnvironmentContributions", |workbench, args, _, cx| {
            let collections = &workbench.terminal_view.host.environment_collections;
            if workbench.terminal_view.active_instance_id().is_none() || collections.is_empty() {
                return Outcome::done();
            }
            let scope = args.first().map(|scope| &scope["workspaceFolder"]);
            let folder = scope.and_then(|folder| folder["index"].as_u64());
            let name = scope.and_then(|folder| folder["name"].as_str()).map(|name| format!(" - {name}")).unwrap_or_default();
            let text = collections.describe(folder);
            let path = PathBuf::from(format!("Environment changes{name}"));
            let document = TextDocument::from_text(ENVIRONMENT_CHANGES_SCHEME, &path, &text, "utf8".into(), Some("markdown"), &workbench.languages, &workbench.settings.lock().unwrap());
            workbench.leave_locked_group(ENVIRONMENT_CHANGES_SCHEME, &path);
            workbench.editors.open(document, false);
            workbench.editors_changed(cx);
            Outcome::done()
        });
    }
}
