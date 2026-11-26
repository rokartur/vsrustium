//! goToCommands.ts, goToSymbol.ts and symbolNavigation.ts: Go to and Peek Definition, Declaration, Type Definition,
//! Implementations and References, their `_execute*Provider` API commands, and the F12 walk through the results.

use futures::channel::oneshot;
use gpui::{AppContext, Context, Div, Styled, Task, Window, div, px};
use serde_json::{Value, json};

use super::Workbench;
use super::editor_message::MessagePart;
use super::host_requests::uri_parts;
use crate::commands::Outcome;
use crate::editor::{OpenEditor, Position, utf16_column, word_at};
use crate::language_features::Candidate;
use crate::references_model::{Location, ReferencesModel};

/// A provider registry: its `$register*Support`, the host's `$provide*`, the model title and the messages.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct SymbolKind {
    pub register: &'static str,
    provide: &'static str,
    title: &'static str,
    no_result_word: &'static str,
    no_result: &'static str,
    setting: &'static str,
    context_key: &'static str,
}

pub(super) const DEFINITION: SymbolKind = SymbolKind {
    register: "$registerDefinitionSupport",
    provide: "$provideDefinition",
    title: "Definitions",
    no_result_word: "No definition found for '{0}'",
    no_result: "No definition found",
    setting: "Definition",
    context_key: "editorHasDefinitionProvider",
};
const DECLARATION: SymbolKind = SymbolKind {
    register: "$registerDeclarationSupport",
    provide: "$provideDeclaration",
    title: "Declarations",
    no_result_word: "No declaration found for '{0}'",
    no_result: "No declaration found",
    setting: "Declaration",
    context_key: "editorHasDeclarationProvider",
};
const TYPE_DEFINITION: SymbolKind = SymbolKind {
    register: "$registerTypeDefinitionSupport",
    provide: "$provideTypeDefinition",
    title: "Type Definitions",
    no_result_word: "No type definition found for '{0}'",
    no_result: "No type definition found",
    setting: "TypeDefinition",
    context_key: "editorHasTypeDefinitionProvider",
};
const IMPLEMENTATION: SymbolKind = SymbolKind {
    register: "$registerImplementationSupport",
    provide: "$provideImplementation",
    title: "Implementations",
    no_result_word: "No implementation found for '{0}'",
    no_result: "No implementation found",
    setting: "Implementation",
    context_key: "editorHasImplementationProvider",
};
pub(super) const REFERENCES: SymbolKind = SymbolKind {
    register: "$registerReferenceSupport",
    provide: "$provideReferences",
    title: "References",
    no_result_word: "No references found for '{0}'",
    no_result: "No references found",
    setting: "Reference",
    context_key: "editorHasReferenceProvider",
};

/// `SymbolNavigationActionConfig` and the action's provider.
#[derive(Clone, Copy)]
struct Action {
    id: &'static str,
    kind: SymbolKind,
    open_to_side: bool,
    open_in_peek: bool,
    /// The references actions pass `compact` (`getReferencesAtPosition`), Peek References does not.
    compact: bool,
    /// The Cmd+click gesture's action shows no message when nothing is found.
    mute_message: bool,
}

const ACTIONS: [Action; 11] = [
    Action { id: "editor.action.revealDefinition", kind: DEFINITION, open_to_side: false, open_in_peek: false, compact: false, mute_message: false },
    Action { id: "editor.action.revealDefinitionAside", kind: DEFINITION, open_to_side: true, open_in_peek: false, compact: false, mute_message: false },
    Action { id: "editor.action.peekDefinition", kind: DEFINITION, open_to_side: false, open_in_peek: true, compact: false, mute_message: false },
    Action { id: "editor.action.revealDeclaration", kind: DECLARATION, open_to_side: false, open_in_peek: false, compact: false, mute_message: false },
    Action { id: "editor.action.peekDeclaration", kind: DECLARATION, open_to_side: false, open_in_peek: true, compact: false, mute_message: false },
    Action { id: "editor.action.goToTypeDefinition", kind: TYPE_DEFINITION, open_to_side: false, open_in_peek: false, compact: false, mute_message: false },
    Action { id: "editor.action.peekTypeDefinition", kind: TYPE_DEFINITION, open_to_side: false, open_in_peek: true, compact: false, mute_message: false },
    Action { id: "editor.action.goToImplementation", kind: IMPLEMENTATION, open_to_side: false, open_in_peek: false, compact: false, mute_message: false },
    Action { id: "editor.action.peekImplementation", kind: IMPLEMENTATION, open_to_side: false, open_in_peek: true, compact: false, mute_message: false },
    Action { id: "editor.action.goToReferences", kind: REFERENCES, open_to_side: false, open_in_peek: false, compact: true, mute_message: false },
    Action { id: "editor.action.referenceSearch.trigger", kind: REFERENCES, open_to_side: false, open_in_peek: true, compact: false, mute_message: false },
];

const ALIASES: [(&str, &str); 4] = [
    ("editor.action.goToDeclaration", "editor.action.revealDefinition"),
    ("editor.action.openDeclarationToTheSide", "editor.action.revealDefinitionAside"),
    ("editor.action.previewDeclaration", "editor.action.peekDefinition"),
    ("editor.action.showReferences", "editor.action.peekLocations"),
];

#[derive(Default)]
pub(super) struct GotoSymbol {
    /// `symbol-navigate-action-highlight`: the model, the range, and the serial its 350ms timer clears.
    highlight: Option<(u32, Position, Position, usize)>,
    serial: usize,
    /// `SymbolNavigationService`: the 'goto' results F12 walks through and the current index (`hasSymbols`).
    navigation: Option<(ReferencesModel, usize)>,
    /// `_activeAlternativeCommands`: the actions running their alternative, which cannot run theirs.
    active_alternatives: Vec<&'static str>,
    _highlight_timer: Option<Task<()>>,
}

impl Workbench {
    pub(super) fn register_goto_symbol_commands(&mut self) {
        for action in ACTIONS {
            self.commands.register(action.id, move |workbench, _, window, cx| {
                workbench.run_symbol_navigation(action, None, window, cx);
                Outcome::done()
            });
        }
        for (alias, id) in ALIASES {
            self.commands.register(alias, move |workbench, args, window, cx| workbench.execute_command(id, args, window, cx));
        }
        for (id, kind) in [("Definition", DEFINITION), ("Declaration", DECLARATION), ("TypeDefinition", TYPE_DEFINITION), ("Implementation", IMPLEMENTATION), ("Reference", REFERENCES)] {
            let command: &'static str = format!("_execute{id}Provider").leak();
            self.commands.register(command, move |workbench, args, _, cx| workbench.execute_location_provider(kind, &args, cx));
            let recursive: &'static str = format!("_execute{id}Provider_recursive").leak();
            self.commands.register(recursive, move |workbench, args, _, cx| workbench.execute_location_provider(kind, &args, cx));
        }
        self.commands.register("editor.action.goToLocations", |workbench, args, window, cx| workbench.go_to_locations(&args, window, cx));
        self.commands.register("editor.action.peekLocations", |workbench, args, window, cx| {
            let mut args = args;
            args.resize(4, Value::Null);
            args.extend([Value::Null, json!(true)]);
            workbench.go_to_locations(&args, window, cx)
        });
        self.commands.register("editor.action.findReferences", |workbench, args, window, cx| workbench.find_references(&args, window, cx));
        self.commands.register("editor.gotoNextSymbolFromResult", |workbench, _, window, cx| {
            workbench.reveal_next_symbol(window, cx);
            Outcome::done()
        });
        self.commands.register("editor.gotoNextSymbolFromResult.cancel", |workbench, _, _, cx| {
            workbench.reset_symbol_navigation(cx);
            Outcome::done()
        });
    }

    /// The provider context keys of editorContextKeys.ts and symbolNavigation.ts `hasSymbols`.
    pub(super) fn goto_symbol_context(&self, context: &mut std::collections::HashMap<String, Value>) {
        context.insert("hasSymbols".into(), json!(self.goto_symbol.navigation.is_some()));
        let Some(editor) = self.editors.active() else { return };
        let uri = editor.document().uri();
        let candidate = Candidate { uri: &uri, language: &editor.document().language_id };
        for kind in [DEFINITION, DECLARATION, TYPE_DEFINITION, IMPLEMENTATION, REFERENCES] {
            context.insert(kind.context_key.into(), json!(!self.language_features.ordered(kind.register, &candidate).is_empty()));
        }
    }

    /// `getLocationLinks`: every provider's links at a position (1-based, UTF-16 column) in provider order, without
    /// the internal schemes of other sources. With `compact`, a references provider giving exactly two is asked
    /// again without the declaration and its single answer kept (`getReferencesAtPosition`).
    pub(super) fn provide_locations(&self, kind: SymbolKind, editor: &OpenEditor, position: Value, compact: bool, cx: &mut Context<Self>) -> Task<Vec<Location>> {
        let uri = editor.document().uri();
        let candidate = Candidate { uri: &uri, language: &editor.document().language_id };
        let mut calls = Vec::new();
        for registration in self.language_features.ordered(kind.register, &candidate) {
            let mut args = vec![json!(registration.handle), uri.clone(), position.clone()];
            if kind == REFERENCES {
                args.push(json!({ "includeDeclaration": true }));
            }
            let second = (compact && kind == REFERENCES).then(|| {
                let mut args = args.clone();
                args[3] = json!({ "includeDeclaration": false });
                args
            });
            calls.push((self.host.call_cancellable("ExtHostLanguageFeatures", kind.provide, args), second));
        }
        let scheme = editor.document().scheme.clone();
        cx.spawn(async move |workbench, cx| {
            // Every provider was asked above; their answers are taken in provider order.
            let mut all = Vec::new();
            for (call, second) in calls {
                let links = links(&scheme, call.await);
                let second = second.filter(|_| links.len() == 2).and_then(|args| workbench.update(cx, |workbench, _| workbench.host.call_cancellable("ExtHostLanguageFeatures", REFERENCES.provide, args)).ok());
                let Some(second) = second else {
                    all.extend(links);
                    continue;
                };
                let without_declaration = self::links(&scheme, second.await);
                all.extend(if without_declaration.len() == 1 { without_declaration } else { links });
            }
            all
        })
    }

    /// `_sortedAndDeduped`: an `_execute*Provider` command's links of a model at a position, as `ReferencesModel` orders them.
    fn execute_location_provider(&mut self, kind: SymbolKind, args: &[Value], cx: &mut Context<Self>) -> Outcome {
        let uri = args.first().unwrap_or(&Value::Null);
        let Some(editor) = uri_parts(uri).ok().and_then(|(scheme, path)| self.editors.model(scheme, &path)) else {
            return Outcome::Ready(Err(crate::commands::error_value("Illegal argument")));
        };
        let task = self.provide_locations(kind, editor, args.get(1).cloned().unwrap_or_default(), false, cx);
        let (sender, receiver) = oneshot::channel();
        cx.background_spawn(async move {
            let model = ReferencesModel::new(task.await, "");
            let _ = sender.send(Ok(Value::Array(model.references.into_iter().map(|reference| reference.location.link).collect())));
        })
        .detach();
        Outcome::Pending(receiver)
    }

    /// goToDefinitionAtPosition.ts `gotoDefinition`: the cursor moves to the clicked position, then a muted Go to
    /// Definition runs, in the peek with `editor.definitionLinkOpensInPeek` unless it opens to the side.
    pub(super) fn go_to_definition_at(&mut self, position: Position, open_to_side: bool, window: &mut Window, cx: &mut Context<Self>) {
        let open_in_peek = !open_to_side && self.setting("editor.definitionLinkOpensInPeek") == json!(true);
        self.editors.active_mut().expect("the link is in the active editor").move_to(position, false);
        let action = Action { id: "", kind: DEFINITION, open_to_side, open_in_peek, compact: false, mute_message: true };
        self.run_symbol_navigation(action, None, window, cx);
    }

    /// `SymbolNavigationAction.runEditorCommand`: the links at the cursor; a change of the text or the cursor before
    /// they arrive drops them (`EditorStateCancellationTokenSource` with Value and Position).
    fn run_symbol_navigation(&mut self, action: Action, generic: Option<GenericLocations>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        let head = editor.selection().head;
        let state = (editor.model_id, editor.text_model().version_id, head);
        let position = json!({ "lineNumber": head.line + 1, "column": utf16_column(&editor.document().lines[head.line], head.column) + 1 });
        let task = match &generic {
            Some(generic) => Task::ready(generic.locations.clone()),
            None => self.provide_locations(action.kind, editor, position, action.compact, cx),
        };
        let title = if generic.is_some() { "Locations" } else { action.kind.title };
        let operation = self.start_cancellable_operation();
        cx.spawn_in(window, async move |workbench, cx| {
            let links = task.await;
            let _ = workbench.update_in(cx, |workbench, window, cx| {
                if super::editor_state::is_cancelled(&operation) || workbench.editors.active().is_none_or(|editor| (editor.model_id, editor.text_model().version_id, editor.selection().head) != state) {
                    return;
                }
                workbench.symbol_navigation_result(action, ReferencesModel::new(links, title), generic, window, cx);
            });
        })
        .detach();
    }

    fn symbol_navigation_result(&mut self, action: Action, model: ReferencesModel, generic: Option<GenericLocations>, window: &mut Window, cx: &mut Context<Self>) {
        let editor = self.editors.active().expect("checked by the caller");
        let head = editor.selection().head;
        let line = editor.document().lines[head.line].clone();
        if model.references.is_empty() {
            let message = match generic {
                _ if action.mute_message => return,
                Some(generic) => match generic.no_result {
                    Some(message) => message,
                    None => return,
                },
                None => match word_at(&line, head.column) {
                    Some(word) => action.kind.no_result_word.replace("{0}", &line.chars().skip(word.start).take(word.len()).collect::<String>()),
                    None => action.kind.no_result.to_string(),
                },
            };
            return self.show_editor_message(vec![MessagePart::text(message)], cx);
        }
        let at_result = model.reference_at(&editor.document().uri_string(), (head.line + 1, utf16_column(&line, head.column) + 1)).is_some();
        let alternative = match &generic {
            Some(_) => None,
            None => self.setting(&format!("editor.gotoLocation.alternative{}Command", action.kind.setting)).as_str().and_then(|id| ACTIONS.into_iter().find(|other| other.id == id)),
        };
        if model.references.len() == 1
            && at_result
            && let Some(alternative) = alternative.filter(|other| !self.goto_symbol.active_alternatives.contains(&other.id))
        {
            // Already at the only result: the alternative runs, and cannot run this action back.
            self.goto_symbol.active_alternatives.push(action.id);
            self.run_symbol_navigation(alternative, None, window, cx);
            self.goto_symbol.active_alternatives.retain(|id| *id != action.id);
            return;
        }
        let setting = || self.setting(&format!("editor.gotoLocation.multiple{}s", action.kind.setting)).as_str().unwrap_or("peek").to_string();
        let preference = generic.and_then(|generic| generic.multiple).map(String::from).unwrap_or_else(setting);
        let selection = editor.selection();
        if action.open_in_peek || preference == "peek" && model.references.len() > 1 {
            return self.open_references_peek(model, selection.start()..selection.end(), action.open_in_peek, window, cx);
        }
        let first = model.first_reference().expect("not empty");
        let peek = model.references.len() > 1 && preference == "gotoAndPeek";
        // shortcut: one editor group, so Open Definition to the Side opens in it; the editor groups row adds the side.
        let _ = action.open_to_side;
        if !self.open_reference(&model.references[first].location, !peek, window, cx) {
            return;
        }
        if peek {
            let selection = self.editors.active().expect("just opened").selection();
            return self.open_references_peek(model, selection.start()..selection.end(), false, window, cx);
        }
        if preference == "goto" {
            self.put_symbol_navigation(model, first, window, cx);
        }
    }

    /// `_openReference`: the target selection range (or the range) collapsed to its start, revealed near the top
    /// when outside the viewport, and highlighted for 350ms.
    pub(super) fn open_reference(&mut self, location: &Location, highlight: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let range = location.selection();
        let selection = json!({ "startLineNumber": range.start.0, "startColumn": range.start.1, "endLineNumber": range.start.0, "endColumn": range.start.1 });
        if self.try_show_text_document(&location.resource, &json!({ "selection": selection, "selectionRevealType": 3 }), window, cx).is_err() {
            return false;
        }
        if highlight {
            let editor = self.editors.active().expect("just shown");
            let (start, end) = super::host_requests::range(editor, &json!({ "startLineNumber": range.start.0, "startColumn": range.start.1, "endLineNumber": range.end.0, "endColumn": range.end.1 }));
            self.goto_symbol.serial += 1;
            let serial = self.goto_symbol.serial;
            self.goto_symbol.highlight = Some((editor.model_id, start, end, serial));
            self.goto_symbol._highlight_timer = Some(cx.spawn(async move |workbench, cx| {
                cx.background_executor().timer(std::time::Duration::from_millis(350)).await;
                let _ = workbench.update(cx, |workbench, cx| {
                    if workbench.goto_symbol.highlight.is_some_and(|highlight| highlight.3 == serial) {
                        workbench.goto_symbol.highlight = None;
                        cx.notify();
                    }
                });
            }));
        }
        cx.notify();
        true
    }

    /// editor.css `.symbolHighlight` on a line: the go to highlight's part of it.
    pub(super) fn symbol_highlight(&self, editor: &OpenEditor, index: usize, char_width: f32) -> Option<Div> {
        let (model_id, start, end, _) = self.goto_symbol.highlight?;
        if model_id != editor.model_id || index < start.line || index > end.line {
            return None;
        }
        let text = &editor.document().lines[index];
        let from = if start.line == index { start.column } else { 0 };
        let to = if end.line == index { end.column } else { text.chars().count() };
        let (left, right) = editor.line_layout(index).range(from, to);
        let (left, width) = (left * char_width, (right - left) * char_width);
        let mut highlight = div().absolute().top_0().h_full().left(px(left)).w(px(width));
        if let Some(background) = self.theme.color("editor.symbolHighlightBackground") {
            highlight = highlight.bg(background);
        }
        if let Some(border) = self.theme.color("editor.symbolHighlightBorder") {
            highlight = highlight.border_1().border_color(border);
        }
        Some(highlight)
    }

    /// `editor.action.goToLocations`: the resource opens with the cursor at the position, then the locations go
    /// through a `GenericGoToLocationAction` with the given multiple behavior and no-result message.
    fn go_to_locations(&mut self, args: &[Value], window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let illegal = || Outcome::Ready(Err(crate::commands::error_value("Illegal argument")));
        let (Some(resource), Some(position), Some(Value::Array(locations))) = (args.first(), args.get(1), args.get(2)) else { return illegal() };
        let multiple = match args.get(3) {
            None | Some(Value::Null) => None,
            Some(Value::String(multiple)) => Some(match multiple.as_str() {
                "gotoAndPeek" => "gotoAndPeek",
                "goto" => "goto",
                _ => "peek",
            }),
            Some(_) => return illegal(),
        };
        let no_result = args.get(4).and_then(Value::as_str).filter(|message| !message.is_empty()).map(String::from);
        let open_in_peek = args.get(5) == Some(&json!(true));
        if position["lineNumber"].as_u64().is_none() || position["column"].as_u64().is_none() {
            return illegal();
        }
        let selection = json!({ "startLineNumber": position["lineNumber"], "startColumn": position["column"], "endLineNumber": position["lineNumber"], "endColumn": position["column"] });
        if self.try_show_text_document(resource, &json!({ "selection": selection, "selectionRevealType": 1 }), window, cx).is_err() {
            return Outcome::done();
        }
        let generic = GenericLocations { locations: locations.iter().filter_map(Location::from_dto).collect(), multiple, no_result };
        let action = Action { id: "editor.action.goToLocation", kind: REFERENCES, open_to_side: false, open_in_peek, compact: false, mute_message: false };
        self.run_symbol_navigation(action, Some(generic), window, cx);
        Outcome::done()
    }

    /// `editor.action.findReferences`: Peek References of the resource at the position.
    fn find_references(&mut self, args: &[Value], window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let (Some(resource), Some(position)) = (args.first(), args.get(1)) else { return Outcome::Ready(Err(crate::commands::error_value("Illegal argument"))) };
        let selection = json!({ "startLineNumber": position["lineNumber"], "startColumn": position["column"], "endLineNumber": position["lineNumber"], "endColumn": position["column"] });
        if self.try_show_text_document(resource, &json!({ "selection": selection, "selectionRevealType": 1 }), window, cx).is_err() {
            return Outcome::done();
        }
        let action = ACTIONS.into_iter().find(|action| action.id == "editor.action.referenceSearch.trigger").expect("registered");
        self.run_symbol_navigation(action, None, window, cx);
        Outcome::done()
    }

    /// `SymbolNavigationService.put`: one result resets, more set `hasSymbols` and show "Symbol n of m".
    fn put_symbol_navigation(&mut self, model: ReferencesModel, index: usize, window: &Window, cx: &mut Context<Self>) {
        if model.references.len() <= 1 {
            return self.reset_symbol_navigation(cx);
        }
        self.goto_symbol.navigation = Some((model, index));
        self.show_symbol_navigation_message(window, cx);
    }

    fn reset_symbol_navigation(&mut self, cx: &mut Context<Self>) {
        if self.goto_symbol.navigation.take().is_some() {
            self.status_message = None;
            cx.notify();
        }
    }

    /// `_showMessage`: the status bar message with the keybinding of `editor.gotoNextSymbolFromResult`.
    fn show_symbol_navigation_message(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some((model, index)) = &self.goto_symbol.navigation else { return };
        let (current, total) = (index + 1, model.references.len());
        let key = self.keybindings.resolver.lookup_primary_keybinding("editor.gotoNextSymbolFromResult", &self.key_context(window));
        let message = match key {
            Some(item) => format!("Symbol {current} of {total}, {} for next", crate::keybindings::label(&item.keybinding)),
            None => format!("Symbol {current} of {total}"),
        };
        // `INotificationService.status` without `hideAfter`: it stays until the navigation resets.
        self.status_message = Some((message, Task::ready(())));
        cx.notify();
    }

    /// `revealNext`: the next result opens near the top when outside the viewport.
    fn reveal_next_symbol(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((model, index)) = self.goto_symbol.navigation.as_mut() else { return };
        *index = (*index + 1) % model.references.len();
        let location = model.references[*index].location.clone();
        self.show_symbol_navigation_message(window, cx);
        let navigation = self.goto_symbol.navigation.take();
        self.open_reference(&location, false, window, cx);
        self.goto_symbol.navigation = navigation;
    }

    /// The `EditorState` listener, run on every render: a cursor outside every result of its file resets.
    pub(super) fn follow_symbol_navigation(&mut self) {
        let Some((model, _)) = &self.goto_symbol.navigation else { return };
        let Some(editor) = self.editors.active() else { return };
        let head = editor.selection().head;
        let uri = editor.document().uri_string();
        let position = (head.line + 1, utf16_column(&editor.document().lines[head.line], head.column) + 1);
        if model.reference_at(&uri, position).is_none() {
            self.goto_symbol.navigation = None;
            self.status_message = None;
        }
    }
}

/// `GenericGoToLocationAction`: given locations, titled "Locations", with no alternative, the multiple behavior
/// of `editor.gotoLocation.multipleReferences` unless given, and silent without a no-result message.
struct GenericLocations {
    locations: Vec<Location>,
    multiple: Option<&'static str>,
    no_result: Option<String>,
}

/// A provider's answer as locations: a single link or an array, without the internal schemes of other sources.
fn links(source_scheme: &str, answer: Result<Result<Value, Value>, oneshot::Canceled>) -> Vec<Location> {
    let Ok(Ok(answer)) = answer else { return Vec::new() };
    let dtos = match answer {
        Value::Array(dtos) => dtos,
        Value::Null => Vec::new(),
        dto => vec![dto],
    };
    dtos.iter()
        .filter_map(Location::from_dto)
        .filter(|location| {
            let scheme = location.resource["scheme"].as_str().unwrap_or_default();
            scheme == source_scheme || !matches!(scheme, "walkThroughSnippet" | "vscode-chat-code-block" | "vscode-chat-code-compare-block")
        })
        .collect()
}

#[cfg(test)]
pub(super) mod tests;
