//! The main-thread side of `window.createQuickPick`, `createInputBox`, `showQuickPick` and `showInputBox`:
//! workbench/api/browser/mainThreadQuickOpen.ts. The quick inputs themselves live in the workbench's
//! quick input controller (workbench/quick_input_view.rs); this module turns host requests into calls on
//! them and their events into host calls.

use std::time::Duration;

use exthost::session::ForwardedRequest;
use futures::channel::oneshot;
use gpui::{Context, Window};
use serde_json::{Value, json};

use crate::commands::Outcome;
use crate::quick_input::{self, Event, Kind, QuickInput, Severity};
use crate::workbench::Workbench;

type Reply = Option<oneshot::Sender<Result<Value, Value>>>;

/// Who a quick input reports to.
pub enum Owner {
    /// A `QuickPick` or `InputBox` an extension created, by its session id.
    Session(i64),
    /// `showQuickPick`: the `$show` request answered with the picked handles; `instance` matches `$setItems`.
    Pick { instance: i64, req: u32, reply: Reply },
    /// `showInputBox`: the `$input` request answered with the accepted value.
    Input { req: u32, reply: Reply, validate: bool, validated: Option<(String, Value)> },
    /// The workbench's quick access picker, with what accepting each item does.
    QuickAccess(Vec<crate::workbench::quick_access::Accept>),
    /// The `workbench.action.selectTheme` picker.
    ColorThemes(crate::workbench::themes::ColorThemePicker),
    /// The `editor.action.goToMatchFindAction` input box.
    GoToMatch,
    /// A picker of the editor status bar entries, with what accepting each item does.
    EditorStatus(Vec<crate::workbench::editor_status::StatusPick>),
    /// `list.resizeColumn`: the column pick, then the input box of the picked column's percentage.
    ResizeColumn(Option<usize>),
    /// A picker of output channels, with what accepting each item does.
    Output(Vec<crate::workbench::output_actions::OutputPick>),
    /// The terminal profile picker or rename input.
    Terminal(crate::workbench::terminal_view::TerminalInput),
    /// The `workbench.action.reopenWithEditor` picker.
    OpenWith(crate::workbench::custom_editors::OpenWithPicker),
    /// The `imagePreview.selectZoomLevel` picker.
    ImageZoom,
    /// `workbench.action.openRecent`: the recently opened entry of each item by handle.
    RecentlyOpened(Vec<Value>),
    /// `workbench.action.localHistory.restoreViaPicker`: the files to pick, by handle.
    LocalHistoryFiles(Vec<std::path::PathBuf>),
    /// The picked file's local history entries, by handle.
    LocalHistoryEntries(std::path::PathBuf, Vec<crate::local_history::Entry>),
    /// `workbench.action.localHistory.create`: the input box of the new entry's source for the file.
    LocalHistoryCreate(std::path::PathBuf),
    /// A workbench `quickInputService.pick` of one item, answered with its handle or null.
    Choose(Reply),
    /// The folder pick of Configure Recommended Extensions (Workspace Folder).
    ExtensionsFolder(Vec<std::path::PathBuf>),
    /// Configure Display Language: the languages, by item handle.
    DisplayLanguage(Vec<crate::workbench::localization::LanguageItem>),
    /// Help: List Signal Sounds and Help: List Signal Announcements.
    AccessibilitySignals(crate::workbench::accessibility::SignalHelp),
    /// Accessibility Help Configure (Un)assigned Keybindings.
    AccessibleViewKeybindings(crate::workbench::accessible_view::KeybindingPick),
    /// File > New File...
    NewFile(crate::workbench::new_file::NewFilePicker),
    /// Configure Metered Connection.
    MeteredConnection,
    /// A pick or input of the debug start flow, answered with the picked handle or the typed value.
    Debug(Option<futures::channel::oneshot::Sender<Option<String>>>),
    /// A task picker (run, build, test, terminate, restart).
    Tasks(crate::workbench::task_quick_pick::TaskPicker),
    /// The tasks.json template picker of the folder.
    TaskTemplates(std::path::PathBuf),
}

impl Workbench {
    pub fn handle_quick_open_request(&mut self, request: ForwardedRequest, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let arg = |index: usize| request.args.get(index).cloned().unwrap_or(Value::Null);
        match request.method.as_str() {
            "$show" => return self.show_pick(arg(0).as_i64().unwrap_or_default(), &arg(1), request.req, window, cx),
            "$setItems" => {
                let instance = arg(0).as_i64().unwrap_or_default();
                let items: Vec<_> = arg(1).as_array().into_iter().flatten().map(|item| quick_input::entry_from_transfer(item, self.workspace())).collect();
                if let Some(input) = self.quick_inputs.find_mut(|owner| matches!(owner, Owner::Pick { instance: pick, .. } if *pick == instance)) {
                    // pick(): the items arrive, the progress stops, and checked items start selected.
                    input.busy = false;
                    let picked: Vec<i64> = items.iter().filter_map(|entry| match entry {
                        quick_input::Entry::Item(item) if item.picked => Some(item.handle),
                        _ => None,
                    }).collect();
                    input.set_items(items);
                    if input.can_select_many {
                        input.set_selected_items(&picked);
                    }
                }
            }
            "$setError" => {
                let instance = arg(0).as_i64().unwrap_or_default();
                if let Some(id) = self.quick_inputs.find_id(|owner| matches!(owner, Owner::Pick { instance: pick, .. } if *pick == instance)) {
                    if let Some(Owner::Pick { reply, .. }) = self.quick_inputs.owner_mut(id) {
                        resolve(reply, Err(arg(1)));
                    }
                    self.hide_quick_input_by_id(id, window, cx);
                }
            }
            "$input" => return self.show_input_box(&arg(0), arg(1).as_bool().unwrap_or(false), request.req, window, cx),
            "$createOrUpdate" => self.create_or_update_quick_input(&arg(0), window, cx),
            "$dispose" => {
                let session = arg(0).as_i64().unwrap_or_default();
                if let Some(id) = self.quick_inputs.find_id(|owner| matches!(owner, Owner::Session(s) if *s == session)) {
                    self.dispose_quick_input(id, window, cx);
                }
            }
            "$cancel" => {
                // The host cancelled the token of a `$show` or `$input` request: its quick input hides.
                let req = request.req;
                if let Some(id) = self.quick_inputs.find_id(|owner| matches!(owner, Owner::Pick { req: r, .. } | Owner::Input { req: r, .. } if *r == req)) {
                    self.hide_quick_input_by_id(id, window, cx);
                }
            }
            _ => {}
        }
        self.flush_quick_input_events(window, cx);
        Outcome::done()
    }

    /// `$show` and `quickInputService.pick`: the pick shows busy until `$setItems` brings its items.
    fn show_pick(&mut self, instance: i64, options: &Value, req: u32, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let (sender, receiver) = oneshot::channel();
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.title = options["title"].as_str().map(str::to_owned);
        input.can_select_many = options["canPickMany"].as_bool().unwrap_or(false);
        input.placeholder = options["placeHolder"].as_str().map(str::to_owned);
        input.prompt = options["prompt"].as_str().map(str::to_owned);
        input.ignore_focus_out = options["ignoreFocusLost"].as_bool().unwrap_or(false);
        input.match_on_description = options["matchOnDescription"].as_bool().unwrap_or(false);
        input.match_on_detail = options["matchOnDetail"].as_bool().unwrap_or(false);
        input.busy = true;
        let id = self.quick_inputs.add(input, Owner::Pick { instance, req, reply: Some(sender) });
        self.show_quick_input(id, window, cx);
        self.flush_quick_input_events(window, cx);
        Outcome::Pending(receiver)
    }

    /// `$input` and `quickInputService.input`.
    fn show_input_box(&mut self, options: &Value, validate: bool, req: u32, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let (sender, receiver) = oneshot::channel();
        let mut input = QuickInput::new(0, Kind::InputBox);
        input.title = options["title"].as_str().map(str::to_owned);
        input.input_box.value = options["value"].as_str().unwrap_or_default().to_owned();
        input.value_selection = selection_range(&options["valueSelection"]);
        input.prompt = options["prompt"].as_str().map(str::to_owned);
        input.placeholder = options["placeHolder"].as_str().map(str::to_owned);
        input.password = options["password"].as_bool().unwrap_or(false);
        input.ignore_focus_out = options["ignoreFocusOut"].as_bool().unwrap_or(false);
        let id = self.quick_inputs.add(input, Owner::Input { req, reply: Some(sender), validate, validated: None });
        self.show_quick_input(id, window, cx);
        if validate {
            let value = self.quick_inputs.get(id).map(|input| input.input_box.value.clone()).unwrap_or_default();
            self.validate_input(id, value, false, window, cx);
        }
        self.flush_quick_input_events(window, cx);
        Outcome::Pending(receiver)
    }

    /// `$createOrUpdate`: applies the properties in the order the host sent them.
    fn create_or_update_quick_input(&mut self, params: &Value, window: &mut Window, cx: &mut Context<Self>) {
        let session = params["id"].as_i64().unwrap_or_default();
        let id = match self.quick_inputs.find_id(|owner| matches!(owner, Owner::Session(s) if *s == session)) {
            Some(id) => id,
            None => {
                let kind = if params["type"] == "quickPick" { Kind::QuickPick } else { Kind::InputBox };
                self.quick_inputs.add(QuickInput::new(0, kind), Owner::Session(session))
            }
        };
        let workspace = self.workspace().cloned();
        let Value::Object(params) = params else { return };
        for (key, value) in params {
            let Some(input) = self.quick_inputs.get_mut(id) else { return };
            let text = || value.as_str().map(str::to_owned);
            let flag = || value.as_bool().unwrap_or(false);
            let handles = || value.as_array().into_iter().flatten().filter_map(Value::as_i64).collect::<Vec<_>>();
            match key.as_str() {
                "id" | "type" => {}
                "visible" if flag() => self.show_quick_input(id, window, cx),
                "visible" => self.hide_quick_input_by_id(id, window, cx),
                "items" => input.set_items(value.as_array().into_iter().flatten().map(|item| quick_input::entry_from_transfer(item, workspace.as_ref())).collect()),
                "activeItems" => input.set_active_items(&handles()),
                "selectedItems" => input.set_selected_items(&handles()),
                "buttons" => input.buttons = value.as_array().into_iter().flatten().map(quick_input::button_from_transfer).collect(),
                "title" => input.title = text(),
                "description" => input.description = text(),
                "step" => input.step = value.as_u64(),
                "totalSteps" => input.total_steps = value.as_u64(),
                "enabled" => input.enabled = flag(),
                "busy" => input.busy = flag(),
                "ignoreFocusOut" => input.ignore_focus_out = flag(),
                "value" => input.set_value(text().unwrap_or_default()),
                "valueSelection" => input.set_value_selection(selection_range(value)),
                "placeholder" => input.placeholder = text(),
                "prompt" => input.prompt = text(),
                "password" => input.password = flag(),
                "validationMessage" => input.validation_message = text(),
                "severity" => input.severity = severity(value),
                "canSelectMany" => input.can_select_many = flag(),
                "matchOnDescription" => input.match_on_description = flag(),
                "matchOnDetail" => input.match_on_detail = flag(),
                "matchOnLabel" => input.match_on_label = value.as_bool().unwrap_or(true),
                "sortByLabel" => input.sort_by_label = value.as_bool().unwrap_or(true),
                "hideInput" => input.hide_input = flag(),
                "keepScrollPosition" => input.keep_scroll_position = flag(),
                _ => {}
            }
            if let Some(input) = self.quick_inputs.get_mut(id) {
                input.update();
            }
        }
        cx.notify();
    }

    /// The events of the quick inputs go to their owners, in the order they happened.
    pub fn deliver_quick_input_event(&mut self, id: i64, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        let Some(owner) = self.quick_inputs.owner(id) else { return };
        match owner {
            Owner::Session(session) => {
                let session = json!(session);
                let (method, args) = match event {
                    Event::ChangeActive(handles) => ("$onDidChangeActive", vec![session, json!(handles)]),
                    Event::ChangeSelection(handles) => ("$onDidChangeSelection", vec![session, json!(handles)]),
                    Event::Accept(_) => ("$onDidAccept", vec![session]),
                    Event::ChangeValue(value) => ("$onDidChangeValue", vec![session, json!(value)]),
                    Event::Hide => ("$onDidHide", vec![session]),
                    Event::TriggerButton(handle, checked) => ("$onDidTriggerButton", vec![session, json!(handle), json!(checked)]),
                    Event::TriggerItemButton(item, button, checked) => ("$onDidTriggerItemButton", vec![session, json!(item), json!(button), json!(checked)]),
                };
                self.host.send("ExtHostQuickOpen", method, args);
            }
            Owner::Pick { .. } => self.pick_event(id, event, window, cx),
            Owner::Input { .. } => self.input_event(id, event, window, cx),
            Owner::QuickAccess(_) => self.quick_access_event(id, event, window, cx),
            Owner::ColorThemes(_) => self.color_theme_picker_event(id, event, window, cx),
            Owner::AccessibilitySignals(_) => self.signal_help_event(id, event, window, cx),
            Owner::AccessibleViewKeybindings(_) => self.accessible_view_keybindings_event(id, event, window, cx),
            Owner::NewFile(_) => self.new_file_picker_event(id, event, window, cx),
            Owner::MeteredConnection => self.metered_connection_picker_event(id, event, window, cx),
            Owner::GoToMatch => self.go_to_match_event(id, event, window, cx),
            Owner::EditorStatus(_) => self.editor_status_event(id, event, window, cx),
            Owner::ResizeColumn(_) => self.resize_column_event(id, event, window, cx),
            Owner::Output(_) => self.output_pick_event(id, event, window, cx),
            Owner::Terminal(_) => self.terminal_input_event(id, event, window, cx),
            Owner::OpenWith(_) => self.open_with_picker_event(id, event, window, cx),
            Owner::ImageZoom => self.image_zoom_picker_event(id, event, window, cx),
            Owner::RecentlyOpened(_) => self.recently_opened_event(id, event, window, cx),
            Owner::LocalHistoryFiles(_) => self.local_history_files_event(id, event, window, cx),
            Owner::LocalHistoryEntries(..) => self.local_history_entries_event(id, event, window, cx),
            Owner::LocalHistoryCreate(_) => self.create_local_history_entry_event(id, event, window, cx),
            Owner::Choose(_) => self.choose_event(id, event, window, cx),
            Owner::ExtensionsFolder(_) => self.extensions_folder_pick_event(id, event, window, cx),
            Owner::DisplayLanguage(_) => self.display_language_pick_event(id, event, window, cx),
            Owner::Debug(_) => self.debug_pick_event(id, event, window, cx),
            Owner::Tasks(_) => self.task_pick_event(id, event, window, cx),
            Owner::TaskTemplates(_) => self.task_template_event(id, event, window, cx),
        }
    }

    /// `quickInputService.pick` for the workbench: the picked item's handle, or null once the pick hides.
    pub fn choose(&mut self, input: QuickInput, window: &mut Window, cx: &mut Context<Self>) -> oneshot::Receiver<Result<Value, Value>> {
        let (sender, receiver) = oneshot::channel();
        let id = self.quick_inputs.add(input, Owner::Choose(Some(sender)));
        self.show_quick_input(id, window, cx);
        self.flush_quick_input_events(window, cx);
        receiver
    }

    /// `quickInputService.input` for the workbench: the accepted value, or null once the input box hides.
    pub fn ask_input(&mut self, input: QuickInput, window: &mut Window, cx: &mut Context<Self>) -> oneshot::Receiver<Result<Value, Value>> {
        let (sender, receiver) = oneshot::channel();
        // No host request: an id `$cancel` never names.
        let id = self.quick_inputs.add(input, Owner::Input { req: u32::MAX, reply: Some(sender), validate: false, validated: None });
        self.show_quick_input(id, window, cx);
        self.flush_quick_input_events(window, cx);
        receiver
    }

    fn choose_event(&mut self, id: i64, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        let picked = match event {
            Event::Accept(_) => self.quick_inputs.get(id).and_then(|input| input.active_handles().first().copied()),
            Event::Hide => {
                if let Some(Owner::Choose(reply)) = self.quick_inputs.owner_mut(id) {
                    resolve(reply, Ok(Value::Null));
                }
                self.quick_inputs.remove(id);
                return;
            }
            _ => return,
        };
        if let Some(Owner::Choose(reply)) = self.quick_inputs.owner_mut(id) {
            resolve(reply, Ok(json!(picked)));
        }
        self.hide_quick_input_by_id(id, window, cx);
    }

    /// The listeners `quickInputService.pick` puts on its quick pick.
    fn pick_event(&mut self, id: i64, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.quick_inputs.get(id) else { return };
        let can_select_many = input.can_select_many;
        let picked = match event {
            Event::Accept(_) if can_select_many => Some(json!(input.selected_handles())),
            Event::Accept(_) => input.active_handles().first().map(|handle| json!(handle)),
            Event::ChangeSelection(handles) if !can_select_many => handles.first().map(|handle| json!(handle)),
            Event::ChangeActive(handles) => {
                // onDidFocus
                if let Some(handle) = handles.first() {
                    self.host.send("ExtHostQuickOpen", "$onItemSelected", vec![json!(handle)]);
                }
                None
            }
            Event::Hide => {
                if let Some(Owner::Pick { reply, .. }) = self.quick_inputs.owner_mut(id) {
                    resolve(reply, Ok(Value::Null));
                }
                self.quick_inputs.remove(id);
                return;
            }
            _ => None,
        };
        if let Some(picked) = picked {
            if let Some(Owner::Pick { reply, .. }) = self.quick_inputs.owner_mut(id) {
                resolve(reply, Ok(picked));
            }
            self.hide_quick_input_by_id(id, window, cx);
        }
    }

    /// The listeners `quickInputService.input` puts on its input box: validation runs 100ms after typing stops
    /// and on accept unless the accepted value was validated already.
    fn input_event(&mut self, id: i64, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Owner::Input { validate, validated, .. }) = self.quick_inputs.owner(id) else { return };
        let validate = *validate;
        let validated = validated.clone();
        match event {
            Event::ChangeValue(value) if validate => self.validate_input(id, value, false, window, cx),
            Event::Accept(_) => {
                let value = self.quick_inputs.get(id).map(|input| input.input_box.value.clone()).unwrap_or_default();
                match validated {
                    _ if !validate => self.finish_input(id, value, &Value::Null, window, cx),
                    Some((validated_value, result)) if validated_value == value => self.finish_input(id, value, &result, window, cx),
                    _ => self.validate_input(id, value, true, window, cx),
                }
            }
            Event::Hide => {
                if let Some(Owner::Input { reply, .. }) = self.quick_inputs.owner_mut(id) {
                    resolve(reply, Ok(Value::Null));
                }
                self.quick_inputs.remove(id);
            }
            _ => {}
        }
    }

    /// Asks the host's `validateInput` about `value`, then shows the answer, or finishes the input on accept.
    fn validate_input(&mut self, id: i64, value: String, accept: bool, window: &mut Window, cx: &mut Context<Self>) {
        let delay = if accept { Duration::ZERO } else { Duration::from_millis(100) };
        cx.spawn_in(window, async move |workbench, cx| {
            cx.background_executor().timer(delay).await;
            let current = |workbench: &mut Workbench| workbench.quick_inputs.get(id).is_some_and(|input| input.input_box.value == value);
            let Ok(Some(answer)) = workbench.update(cx, |workbench, _| current(workbench).then(|| workbench.host.call("ExtHostQuickOpen", "$validateInput", vec![json!(value)]))) else { return };
            let result = answer.await.ok().and_then(Result::ok).unwrap_or(Value::Null);
            let _ = workbench.update_in(cx, |workbench, window, cx| {
                if !current(workbench) {
                    return;
                }
                if let Some(Owner::Input { validated, .. }) = workbench.quick_inputs.owner_mut(id) {
                    *validated = Some((value.clone(), result.clone()));
                }
                match accept {
                    true => workbench.finish_input(id, value.clone(), &result, window, cx),
                    false => set_validation(workbench.quick_inputs.get_mut(id).expect("checked by current"), &result),
                }
                workbench.flush_quick_input_events(window, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn finish_input(&mut self, id: i64, value: String, result: &Value, window: &mut Window, cx: &mut Context<Self>) {
        if blocks_accept(result) {
            if let Some(input) = self.quick_inputs.get_mut(id) {
                set_validation(input, result);
            }
            cx.notify();
            return;
        }
        if let Some(Owner::Input { reply, .. }) = self.quick_inputs.owner_mut(id) {
            resolve(reply, Ok(json!(value)));
        }
        self.hide_quick_input_by_id(id, window, cx);
    }
}

fn resolve(reply: &mut Reply, result: Result<Value, Value>) {
    if let Some(sender) = reply.take() {
        let _ = sender.send(result);
    }
}

/// A validation result blocks the accept unless it is empty or below error severity.
fn blocks_accept(result: &Value) -> bool {
    match result {
        Value::String(message) => !message.is_empty(),
        Value::Object(_) => severity(&result["severity"]) == Severity::Error,
        _ => false,
    }
}

/// `setValidationOnInput`.
fn set_validation(input: &mut QuickInput, result: &Value) {
    (input.severity, input.validation_message) = match result {
        Value::String(message) if !message.is_empty() => (Severity::Error, Some(message.clone())),
        Value::Object(_) => (severity(&result["severity"]), result["content"].as_str().map(str::to_owned)),
        _ => (Severity::Ignore, None),
    };
}

/// `Severity` of base/common/severity.ts.
fn severity(value: &Value) -> Severity {
    match value.as_u64() {
        Some(1) => Severity::Info,
        Some(2) => Severity::Warning,
        Some(3) => Severity::Error,
        _ => Severity::Ignore,
    }
}

fn selection_range(value: &Value) -> Option<(usize, usize)> {
    let range = value.as_array()?;
    Some((range.first()?.as_u64()? as usize, range.get(1)?.as_u64()? as usize))
}
