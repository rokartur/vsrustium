//! keybindingCancellation.ts: the provider requests an `EditorStateCancellationTokenSource` guards are also
//! cancelled by `editor.cancelOperation` (Escape while `cancellableOperation`), the last one started first.

use std::cell::Cell;
use std::rc::{Rc, Weak};

use serde_json::json;

use super::Workbench;
use crate::commands::Outcome;

/// One operation's token: set once `editor.cancelOperation` cancels it. Dropping it (the operation's task ended or
/// was dropped) takes the operation off its editor (`dispose`).
pub(super) type OperationToken = Rc<Cell<bool>>;

#[derive(Default)]
pub(super) struct CancellableOperations {
    /// The operations by editor tab, oldest first (`IEditorCancellationTokens`); dropped tokens no longer count.
    pending: Vec<(u32, Weak<Cell<bool>>)>,
}

impl CancellableOperations {
    fn live(&self, editor: u32) -> impl DoubleEndedIterator<Item = Rc<Cell<bool>>> + '_ {
        self.pending.iter().filter(move |(owner, _)| *owner == editor).filter_map(|(_, token)| token.upgrade()).filter(|token| !token.get())
    }
}

impl Workbench {
    pub(super) fn register_cancel_operation_command(&mut self) {
        self.commands.register("editor.cancelOperation", |workbench, _, _, cx| {
            let Some(editor) = workbench.editors.active_tab().map(|editor| editor.model_id) else { return Outcome::done() };
            if let Some(token) = workbench.cancellable_operations.live(editor).next_back() {
                token.set(true);
                cx.notify();
            }
            workbench.cancellable_operations.pending.retain(|(_, token)| token.upgrade().is_some_and(|token| !token.get()));
            Outcome::done()
        });
    }

    /// `new EditorKeybindingCancellationTokenSource(editor)` on the active editor; the caller keeps the token until
    /// the operation ends and checks it with `is_cancelled`.
    pub(super) fn start_cancellable_operation(&mut self) -> Option<OperationToken> {
        let editor = self.editors.active_tab()?.model_id;
        let token = Rc::new(Cell::new(false));
        let operations = &mut self.cancellable_operations;
        operations.pending.retain(|(_, token)| token.strong_count() > 0);
        operations.pending.push((editor, Rc::downgrade(&token)));
        Some(token)
    }

    /// `cancellableOperation` of the active editor.
    pub(super) fn cancellable_operation_context(&self, context: &mut std::collections::HashMap<String, serde_json::Value>) {
        let pending = self.editors.active_tab().is_some_and(|editor| self.cancellable_operations.live(editor.model_id).next().is_some());
        context.insert("cancellableOperation".into(), json!(pending));
    }
}

/// The token's `isCancellationRequested`; an operation without one cannot be cancelled by key.
pub(super) fn is_cancelled(token: &Option<OperationToken>) -> bool {
    token.as_ref().is_some_and(|token| token.get())
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;
    use serde_json::json;

    use crate::workbench::goto_symbol::tests::{answer, open, run};

    /// Escape (`editor.cancelOperation`) drops a pending Go to Definition: its answer moves nothing.
    #[gpui::test]
    fn cancel_operation_drops_a_pending_definition_request(cx: &mut TestAppContext) {
        let (workbench, cx, receiver, dir) = open(cx, "cancel-operation", "foo\nbar foo\n");
        workbench.update(cx, |workbench, _| workbench.editors.active_mut().unwrap().move_to(crate::editor::Position::new(1, 5), false));
        run(&workbench, cx, "editor.action.revealDefinition");
        let cancellable = |cx: &mut gpui::VisualTestContext| workbench.update_in(cx, |workbench, window, _| workbench.key_context(window).get("cancellableOperation").cloned());
        assert_eq!(cancellable(cx), Some(json!(true)));
        run(&workbench, cx, "editor.cancelOperation");
        assert_eq!(cancellable(cx), Some(json!(false)));
        answer(&receiver, "$provideDefinition", json!([crate::workbench::goto_symbol::tests::link(&dir, 1, 1, 4)]), cx);
        workbench.read_with(cx, |workbench, _| assert_eq!(workbench.editors.active().unwrap().selection().head, crate::editor::Position::new(1, 5)));
    }
}
