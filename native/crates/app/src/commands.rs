//! The command registry of platform/commands/common/commands.ts (`CommandsRegistry`) and the
//! `executeCommand` of workbench/services/commands/common/commandService.ts. Workbench actions, editor
//! commands, keybindings and `vscode.commands` all run through it.

use std::collections::BTreeMap;
use std::rc::Rc;

use futures::channel::oneshot;
use gpui::{Context, Window};
use serde_json::{Value, json};

use crate::workbench::Workbench;

/// What a command returns: a value now, or one an extension host sends later. Errors are `SerializedError`s.
pub enum Outcome {
    Ready(Result<Value, Value>),
    Pending(oneshot::Receiver<Result<Value, Value>>),
}

impl Outcome {
    pub fn done() -> Outcome {
        Outcome::Ready(Ok(Value::Null))
    }

    pub fn error(message: impl Into<String>) -> Outcome {
        Outcome::Ready(Err(error_value(&message.into())))
    }
}

pub type Handler = Rc<dyn Fn(&mut Workbench, Vec<Value>, &mut Window, &mut Context<Workbench>) -> Outcome>;

/// Registrations of one id stack: the newest runs, and removing it brings back the one under it.
#[derive(Default)]
pub struct Commands {
    handlers: BTreeMap<String, Vec<Handler>>,
}

impl Commands {
    pub fn register(&mut self, id: impl Into<String>, handler: impl Fn(&mut Workbench, Vec<Value>, &mut Window, &mut Context<Workbench>) -> Outcome + 'static) {
        self.handlers.entry(id.into()).or_default().push(Rc::new(handler));
    }

    /// Registers a table of upstream ids, such as the editor's command table, in one call.
    pub fn register_all<F>(&mut self, table: impl IntoIterator<Item = (&'static str, F)>)
    where
        F: Fn(&mut Workbench, Vec<Value>, &mut Window, &mut Context<Workbench>) -> Outcome + 'static,
    {
        for (id, handler) in table {
            self.register(id, handler);
        }
    }

    /// Removes the newest registration of `id`.
    pub fn unregister(&mut self, id: &str) {
        let Some(stack) = self.handlers.get_mut(id) else { return };
        stack.pop();
        if stack.is_empty() {
            self.handlers.remove(id);
        }
    }

    pub fn get(&self, id: &str) -> Option<Handler> {
        self.handlers.get(id).and_then(|stack| stack.last()).cloned()
    }

    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.handlers.keys().map(String::as_str)
    }
}

/// `transformErrorForSerialization` of an `Error` thrown on the main thread.
pub fn error_value(message: &str) -> Value {
    json!({ "$isError": true, "name": "Error", "message": message, "stack": format!("Error: {message}") })
}

impl Workbench {
    /// `CommandService.executeCommand`: activates `onCommand:<id>` and runs the command, waiting for the
    /// activation first when no one registered it yet.
    pub fn execute_command(&mut self, id: &str, args: Vec<Value>, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let activation = self.host.activate_by_event(&format!("onCommand:{id}"));
        if let Some(handler) = self.commands.get(id) {
            return handler(self, args, window, cx);
        }
        let (sender, receiver) = oneshot::channel();
        let id = id.to_owned();
        cx.spawn_in(window, async move |workbench, cx| {
            // A host that is gone answers nothing; the lookup below then reports the command as missing.
            let _ = activation.await;
            let outcome = workbench.update_in(cx, |workbench, window, cx| match workbench.commands.get(&id) {
                Some(handler) => handler(workbench, args, window, cx),
                None => Outcome::error(format!("command '{id}' not found")),
            });
            let result = match outcome {
                Ok(Outcome::Ready(result)) => result,
                Ok(Outcome::Pending(pending)) => pending.await.unwrap_or(Ok(Value::Null)),
                Err(_) => return,
            };
            let _ = sender.send(result);
        })
        .detach();
        Outcome::Pending(receiver)
    }
}
