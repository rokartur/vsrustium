//! The command registry of platform/commands/common/commands.ts (`CommandsRegistry`) and the
//! `executeCommand` of workbench/services/commands/common/commandService.ts. Workbench actions, editor
//! commands, keybindings and `vscode.commands` all run through it.

use std::collections::BTreeMap;
use std::rc::Rc;

use futures::FutureExt;
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
    handlers: BTreeMap<String, Vec<(Registration, Handler)>>,
    next_registration: Registration,
}

/// What `registerCommand` returns: disposing it removes that one registration, wherever it sits in its id's stack.
pub type Registration = u64;

impl Commands {
    pub fn register(&mut self, id: impl Into<String>, handler: impl Fn(&mut Workbench, Vec<Value>, &mut Window, &mut Context<Workbench>) -> Outcome + 'static) -> Registration {
        self.next_registration += 1;
        self.handlers.entry(id.into()).or_default().push((self.next_registration, Rc::new(handler)));
        self.next_registration
    }

    /// Disposes one registration of `id`; the newest of the others runs from then on.
    pub fn dispose(&mut self, id: &str, registration: Registration) {
        let Some(stack) = self.handlers.get_mut(id) else { return };
        stack.retain(|(other, _)| *other != registration);
        if stack.is_empty() {
            self.handlers.remove(id);
        }
    }

    /// Registers a table of upstream ids, such as the editor's command table, in one call.
    #[allow(dead_code, reason = "Milestone 8 registers editor/commands.rs through it")]
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
        self.handlers.get(id).and_then(|stack| stack.last()).map(|(_, handler)| handler.clone())
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
    /// `CommandService.executeCommand`: runs the command at once when `onCommand:<id>` is done or the host is not up yet,
    /// else waits for that activation first, registered or not, as an extension may override the command (issue #71471).
    pub fn execute_command(&mut self, id: &str, args: Vec<Value>, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        self.welcome_command_executed(id);
        self.record_terminal_command(id);
        let side = self.leave_side_group_for(id);
        let event = format!("onCommand:{id}");
        let activation = self.host.activate_by_event(&event);
        // `activationEventIsDone` is also true for an event no running extension declares: waiting for the host
        // there would run a key's command after the keys typed behind it.
        let run_now = !self.host.activation_events.contains(&event) || activation.clone().now_or_never().is_some() || !self.host.installed_extensions_registered;
        if let Some(handler) = self.commands.get(id).filter(|_| run_now) {
            let outcome = handler(self, args, window, cx);
            if let Some(side) = side {
                self.return_to_side_group(side);
            }
            return outcome;
        }
        let (sender, receiver) = oneshot::channel();
        let id = id.to_owned();
        cx.spawn_in(window, async move |workbench, cx| {
            // A host that is gone answers nothing; the lookup below then reports the command as missing.
            if let Ok(Err(error)) = activation.await {
                let _ = sender.send(Err(error));
                return;
            }
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

    /// `runCommands` (commands.contribution.ts): runs `{ commands }` in order, each an id or `{ command, args }`
    /// whose array `args` spreads; the first failure stops the rest.
    pub(crate) fn register_run_commands(&mut self) {
        self.commands.register("runCommands", |workbench, args, window, cx| {
            let Some(commands) = runnable_commands(args.first()) else {
                workbench.notify_error("'runCommands' has received an argument with incorrect type. Please, review the argument passed to the command.");
                cx.notify();
                return Outcome::done();
            };
            if commands.is_empty() {
                workbench.notify_warning("'runCommands' has not received commands to run. Did you forget to pass commands in the 'runCommands' argument?");
                cx.notify();
                return Outcome::done();
            }
            let (sender, receiver) = oneshot::channel();
            cx.spawn_in(window, async move |workbench, cx| {
                for (id, args) in commands {
                    let Ok(outcome) = workbench.update_in(cx, |workbench, window, cx| workbench.execute_command(&id, args, window, cx)) else { return };
                    let result = match outcome {
                        Outcome::Ready(result) => result,
                        Outcome::Pending(pending) => pending.await.unwrap_or(Ok(Value::Null)),
                    };
                    if let Err(error) = result {
                        let _ = workbench.update(cx, |workbench, cx| {
                            workbench.notify_error(error["message"].as_str().unwrap_or_default());
                            cx.notify();
                        });
                        break;
                    }
                }
                let _ = sender.send(Ok(Value::Null));
            })
            .detach();
            Outcome::Pending(receiver)
        });
    }
}

/// `_isCommandArgs` and `_runCommand`: each command's id and its arguments, or `None` for a malformed argument.
fn runnable_commands(args: Option<&Value>) -> Option<Vec<(String, Vec<Value>)>> {
    let mut commands = Vec::new();
    for command in args?.get("commands")?.as_array()? {
        let entry = match command {
            Value::String(id) => (id.clone(), Vec::new()),
            _ => {
                let id = command.get("command")?.as_str()?.to_string();
                match command.get("args") {
                    None => (id, Vec::new()),
                    Some(Value::Array(args)) => (id, args.clone()),
                    Some(args) => (id, vec![args.clone()]),
                }
            }
        };
        commands.push(entry);
    }
    Some(commands)
}

#[cfg(test)]
mod tests;
