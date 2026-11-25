//! The workbench's opener service: openerService.ts `open` with the link validator of trustedDomainsValidator.ts,
//! the folder opener of opener.contribution.ts, the app url opener of urlService.ts and the extension openers of
//! externalUriOpenerService.ts and mainThreadUriOpeners.ts.

use std::path::PathBuf;

use futures::channel::oneshot;
use gpui::{AsyncWindowContext, ClipboardItem, Context, PromptLevel, WeakEntity, Window};
use serde_json::{Value, json};

use super::Workbench;
use crate::commands::{Outcome, error_value};
use crate::dialog;
use crate::notifications::Severity;
use crate::quick_input::{Entry, Item, Kind, QuickInput};
use crate::trusted_domains::{self, is_url_domain_trusted, test_url_matches_glob};
use crate::uri::{Uri, encode_uri, percent_decode};

/// opener.ts `defaultExternalUriOpenerId`.
pub const DEFAULT_OPENER_ID: &str = "default";
/// externalUriOpener/common/configuration.ts `externalUriOpenersSettingId`.
const OPENERS_SETTING: &str = "workbench.externalUriOpeners";

/// What `open` gets: a string keeps its exact text for the browser and the prompt, a uri is written out.
pub enum Target {
    Text(String),
    Uri(Uri),
}

impl Target {
    fn uri(&self) -> Result<Uri, String> {
        match self {
            Target::Text(text) => Uri::parse(text),
            Target::Uri(uri) => Ok(uri.clone()),
        }
    }

    /// `_resolveExternalUriOpenTarget` without resolvers: the port has no tunnels, and the workspace resolver of
    /// window.ts skips `openExternal` and the web links that reach it.
    fn href(&self) -> String {
        match self {
            Target::Text(text) => text.clone(),
            Target::Uri(uri) => encode_uri(&uri.to_string(true)),
        }
    }
}

/// opener.ts `OpenOptions`, the fields the port's callers set.
#[derive(Clone, Default)]
pub struct OpenOptions {
    pub open_external: bool,
    pub allow_commands: bool,
    pub contributed_openers: ContributedOpeners,
}

/// `allowContributedOpeners`: `true`, or the id of the opener to use.
#[derive(Clone, Default, PartialEq)]
pub enum ContributedOpeners {
    #[default]
    No,
    Any,
    Preferred(String),
}

impl ContributedOpeners {
    pub fn from_json(value: &Value) -> ContributedOpeners {
        match value {
            Value::Bool(true) => ContributedOpeners::Any,
            Value::String(id) => ContributedOpeners::Preferred(id.clone()),
            _ => ContributedOpeners::No,
        }
    }
}

/// An opener an extension registered with `registerExternalUriOpener`.
#[derive(Clone)]
pub struct UriOpener {
    pub id: String,
    pub schemes: Vec<String>,
    pub label: String,
}

impl Workbench {
    /// openerService.ts `open`: answers whether an opener took the target.
    pub(crate) fn open_link(&mut self, target: Target, options: OpenOptions, window: &mut Window, cx: &mut Context<Self>) -> oneshot::Receiver<Result<Value, Value>> {
        let (sender, receiver) = oneshot::channel();
        cx.spawn_in(window, async move |workbench, cx| {
            let opened = open(&workbench, target, options, cx).await;
            let _ = sender.send(Ok(json!(opened)));
        })
        .detach();
        receiver
    }

    /// trustedDomainService.ts `trustedDomains`: the product has no default trusted domains.
    pub(super) fn trusted_domains(&self) -> Vec<String> {
        let mut storage = self.profile_storage.lock().unwrap();
        storage.refresh();
        storage.get(trusted_domains::STORAGE_KEY).and_then(|domains| serde_json::from_str(domains).ok()).unwrap_or_default()
    }

    /// mainThreadWindow.ts `$openUri`: the string the extension passed when parsing it gives the same uri.
    pub(crate) fn api_open_uri(&mut self, args: &[Value], window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let uri = match Uri::from_json(&args[0]) {
            Ok(uri) => uri,
            Err(error) => return Outcome::Ready(Err(error_value(&error))),
        };
        let target = match args[1].as_str() {
            Some(text) if Uri::parse(text).is_ok_and(|parsed| parsed.to_string(false) == uri.to_string(false)) => Target::Text(text.to_owned()),
            _ => Target::Uri(uri),
        };
        let contributed_openers = ContributedOpeners::from_json(&args[2]["allowContributedOpeners"]);
        let options = OpenOptions { open_external: true, contributed_openers, ..OpenOptions::default() };
        Outcome::Pending(self.open_link(target, options, window, cx))
    }

    /// mainThreadWindow.ts: `hostService.onDidChangeFocus` reaches `ExtHostWindow.$onDidChangeWindowFocus`.
    pub fn watch_window_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The title bar, the shell and the activity bar draw the inactive look.
        cx.observe_window_activation(window, |workbench, window, cx| {
            workbench.host.send("ExtHostWindow", "$onDidChangeWindowFocus", vec![json!(window.is_window_active())]);
            cx.notify();
        })
        .detach();
    }

    /// `$registerUriOpener`.
    pub(crate) fn register_uri_opener(&mut self, args: &[Value]) -> Outcome {
        let id = args[0].as_str().unwrap_or_default().to_owned();
        if self.urls.openers.iter().any(|opener| opener.id == id) {
            return Outcome::Ready(Err(error_value(&format!("Opener with id '{id}' already registered"))));
        }
        let schemes = args[1].as_array().into_iter().flatten().filter_map(|scheme| scheme.as_str().map(str::to_owned)).collect();
        let label = args[3].as_str().unwrap_or_default().to_owned();
        self.urls.openers.push(UriOpener { id, schemes, label });
        Outcome::done()
    }
}

async fn open(workbench: &WeakEntity<Workbench>, target: Target, options: OpenOptions, cx: &mut AsyncWindowContext) -> bool {
    let Ok(uri) = target.uri() else { return false };
    if uri.scheme == "internal" {
        return false;
    }
    let external = options.open_external || matches!(uri.scheme.as_str(), "mailto" | "http" | "https" | "vsls");
    let contributed = external && options.contributed_openers != ContributedOpeners::No && options.contributed_openers != ContributedOpeners::Preferred(DEFAULT_OPENER_ID.into());
    let href = target.href();
    if contributed && open_with_contributed_openers(workbench, &href, &uri, &options.contributed_openers, cx).await {
        return true;
    }
    if !validate(workbench, &target, &uri, cx).await {
        return false;
    }
    if !options.open_external && open_folder_link(workbench, &uri, cx) {
        return true;
    }
    if uri.scheme == super::url_handler::URL_PROTOCOL {
        return super::url_handler::handle_url(workbench, uri, false, cx).await;
    }
    if external {
        let _ = workbench.update(cx, |workbench, cx| workbench.open_default_external(&href, cx));
        return true;
    }
    if uri.scheme == "command" {
        let _ = workbench.update_in(cx, |workbench, window, cx| run_command_link(workbench, &uri, options.allow_commands, window, cx));
        return true;
    }
    workbench.update_in(cx, |workbench, window, cx| open_in_editor(workbench, uri, window, cx)).unwrap_or(false)
}

impl Workbench {
    /// window.ts `setDefaultExternalOpener`: `nativeHostService.openExternal` with `workbench.externalBrowser`.
    fn open_default_external(&mut self, href: &str, cx: &mut Context<Self>) {
        let browser = self.setting("workbench.externalBrowser");
        let browser = browser.as_str().unwrap_or_default();
        if browser.is_empty() || !matches!(href.split(':').next(), Some("http" | "https")) {
            cx.open_url(href);
            return;
        }
        // The `open` package runs `open -a <app> <url>` on macOS; a path that does not exist falls back to the shell.
        if (browser.contains('/') && !std::path::Path::new(browser).exists()) || std::process::Command::new("open").args(["-a", browser, href]).status().map_or(true, |status| !status.success()) {
            eprintln!("[opener] cannot open {href} in {browser}");
            cx.open_url(href);
        }
    }
}

/// opener.contribution.ts `WorkbenchOpenerContribution`: a link to a folder of the workspace reveals it in the Explorer.
fn open_folder_link(workbench: &WeakEntity<Workbench>, uri: &Uri, cx: &mut AsyncWindowContext) -> bool {
    if uri.scheme != "file" {
        return false;
    }
    let path = PathBuf::from(&uri.path);
    workbench
        .update_in(cx, |workbench, window, cx| {
            let inside = workbench.workspace().is_some_and(|workspace| workspace.folders.iter().any(|folder| path.starts_with(&folder.path)));
            if !inside || !path.is_dir() {
                return false;
            }
            workbench.execute_command("workbench.view.explorer", Vec::new(), window, cx);
            workbench.select_explorer_item(&path, window, cx);
            true
        })
        .unwrap_or(false)
}

/// openerService.ts `CommandOpener`: without `allowCommands` a command link does nothing, and no other opener runs.
fn run_command_link(workbench: &mut Workbench, uri: &Uri, allow_commands: bool, window: &mut Window, cx: &mut Context<Workbench>) {
    if !allow_commands {
        return;
    }
    let args = serde_json::from_str::<Value>(&percent_decode(&uri.query)).or_else(|_| serde_json::from_str::<Value>(&uri.query));
    let args = match args {
        Ok(Value::Array(args)) => args,
        Ok(arg) => vec![arg],
        Err(_) => Vec::new(),
    };
    let outcome = workbench.execute_command(&uri.path, args, window, cx);
    super::keyboard::report_failure(outcome, cx);
}

/// openerService.ts `EditorOpener` with opener.ts `extractSelection`: `#L<line>,<column>-L<line>,<column>`.
fn open_in_editor(workbench: &mut Workbench, uri: Uri, window: &mut Window, cx: &mut Context<Workbench>) -> bool {
    let (selection, uri) = extract_selection(uri);
    let options = match selection {
        Some(selection) => json!({ "selection": selection }),
        None => Value::Null,
    };
    if let Err(error) = workbench.try_show_text_document(&uri.to_json(), &options, window, cx) {
        workbench.notify_error(error["message"].as_str().unwrap_or("Unable to open the editor").to_owned());
        cx.notify();
    }
    true
}

pub(super) fn extract_selection(uri: Uri) -> (Option<Value>, Uri) {
    let fragment = uri.fragment.strip_prefix('L').unwrap_or(&uri.fragment);
    let number = |text: &str| -> Option<(u64, usize)> {
        let digits = text.chars().take_while(char::is_ascii_digit).count();
        text[..digits].parse().ok().map(|number| (number, digits))
    };
    let Some((start_line, read)) = number(fragment) else { return (None, uri) };
    let mut rest = &fragment[read..];
    let mut start_column = 1;
    if let Some((column, read)) = rest.strip_prefix(',').and_then(number) {
        start_column = column;
        rest = &rest[1 + read..];
    }
    let mut end = (start_line, start_column);
    if let Some(range_end) = rest.strip_prefix('-') {
        let range_end = range_end.strip_prefix('L').unwrap_or(range_end);
        if let Some((line, read)) = number(range_end) {
            let column = range_end[read..].strip_prefix(',').and_then(number).map_or(1, |(column, _)| column);
            end = (line, column);
        }
    }
    // applyTextEditorOptions: a selection without an end is the caret at its start.
    let selection = json!({ "startLineNumber": start_line, "startColumn": start_column, "endLineNumber": end.0, "endColumn": end.1 });
    let mut uri = uri;
    uri.fragment.clear();
    (Some(selection), uri)
}

/// trustedDomainsValidator.ts `validateLink`: a web link outside the trusted domains asks first.
// shortcut: no `fromWorkspace` callers yet, so neither the command link trust request nor
// `workbench.trustedDomains.promptInTrustedWorkspace` applies; upgrade with markdown hovers in the editor.
async fn validate(workbench: &WeakEntity<Workbench>, target: &Target, uri: &Uri, cx: &mut AsyncWindowContext) -> bool {
    if uri.scheme != "http" && uri.scheme != "https" {
        return true;
    }
    let Ok(trusted) = workbench.update(cx, |workbench, _| workbench.trusted_domains()) else { return false };
    if is_url_domain_trusted(uri, &trusted) {
        return true;
    }
    let detail = match target {
        Target::Text(text) => text.clone(),
        Target::Uri(_) => formatted_link(uri),
    };
    let buttons = ["Open", "Copy", "Configure Trusted Domains", "Cancel"];
    let Ok(answer) = workbench.update_in(cx, |_, window, cx| dialog::show_message_box(window, PromptLevel::Info, "Do you want Code - OSS to open the external website?", Some(&detail), &buttons, None, cx)) else {
        return false;
    };
    match answer.await {
        Ok((0, _)) => true,
        Ok((1, _)) => {
            let text = match target {
                Target::Text(text) => text.clone(),
                Target::Uri(uri) => uri.to_string(true),
            };
            let _ = cx.update(|_, cx| cx.write_to_clipboard(ClipboardItem::new_string(text)));
            false
        }
        Ok((2, _)) => {
            let domain = format!("{}://{}", uri.scheme, uri.authority);
            let picked = configure_trusted_domains(workbench, trusted, domain, uri, cx).await;
            picked.iter().any(|domain| domain == "*") || is_url_domain_trusted(uri, &picked)
        }
        _ => false,
    }
}

/// The link as the prompt shows it: the query and fragment shortened to keep it near 60 characters.
pub(super) fn formatted_link(uri: &Uri) -> String {
    let mut link = format!("{}://{}{}", uri.scheme, uri.authority, uri.path);
    let query = if uri.query.is_empty() { String::new() } else { format!("?{}", uri.query) };
    let fragment = if uri.fragment.is_empty() { String::new() } else { format!("#{}", uri.fragment) };
    let tail: Vec<char> = format!("{query}{fragment}").chars().collect();
    let remaining = 60usize.saturating_sub(link.chars().count());
    let keep = remaining.max(5).min(tail.len());
    if keep == tail.len() {
        link.extend(tail);
    } else {
        link.push(tail[0]);
        link.push_str("...");
        link.extend(&tail[tail.len() - keep + 1..]);
    }
    link
}

/// trustedDomains.ts `configureOpenerTrustedDomainsHandler`: the trusted domains after the pick.
async fn configure_trusted_domains(workbench: &WeakEntity<Workbench>, trusted: Vec<String>, domain: String, resource: &Uri, cx: &mut AsyncWindowContext) -> Vec<String> {
    let authority = Uri::parse(&domain).map(|uri| uri.authority).unwrap_or_default();
    let segments: Vec<&str> = authority.split('.').collect();
    let domain_end = segments[segments.len().saturating_sub(2)..].join(".");
    let is_number = |text: &str| !text.is_empty() && text.chars().all(|char| char.is_ascii_digit());
    let is_ip = segments.len() == 4 && segments.iter().all(|segment| is_number(segment) || is_number(segment.split(':').next().unwrap_or_default()));
    let mut choices = vec![(format!("Trust {domain}"), Some(domain.clone()))];
    if !is_ip {
        choices.push((format!("Trust {domain_end} and all its subdomains"), Some(format!("*.{domain_end}"))));
    } else if authority.contains(':') {
        let base = authority.split(':').next().unwrap_or_default();
        choices.push((format!("Trust {base} on all ports"), Some(format!("{base}:*"))));
    }
    choices.push(("Trust all domains (disables link protection)".into(), Some("*".into())));
    choices.push(("Manage Trusted Domains".into(), None));
    let items = choices.iter().enumerate().map(|(handle, (label, _))| Entry::Item(Item { handle: handle as i64, label: label.clone(), pickable: true, ..Item::default() })).collect();
    let mut input = QuickInput::new(0, Kind::QuickPick);
    input.set_items(items);
    input.set_active_items(&[0]);
    let Ok(picked) = workbench.update_in(cx, |workbench, window, cx| workbench.choose(input, window, cx)) else { return Vec::new() };
    let Some(index) = picked.await.ok().and_then(Result::ok).and_then(|picked| picked.as_u64()) else { return Vec::new() };
    match &choices[index as usize].1 {
        None => {
            let resource = resource.clone();
            let _ = workbench.update_in(cx, |workbench, window, cx| workbench.open_trusted_domains(Some(&resource), window, cx));
            trusted
        }
        Some(to_trust) if !trusted.contains(to_trust) => {
            let mut trusted = trusted;
            trusted.push(to_trust.clone());
            let _ = workbench.update(cx, |workbench, _| {
                let mut storage = workbench.profile_storage.lock().unwrap();
                storage.delete(trusted_domains::CONTENT_STORAGE_KEY);
                storage.set(trusted_domains::STORAGE_KEY, serde_json::to_string(&trusted).expect("strings serialize"));
            });
            trusted
        }
        Some(_) => Vec::new(),
    }
}

/// externalUriOpenerService.ts `openExternal`: false lets the default opener run.
async fn open_with_contributed_openers(workbench: &WeakEntity<Workbench>, href: &str, source: &Uri, allowed: &ContributedOpeners, cx: &mut AsyncWindowContext) -> bool {
    let Ok(target) = Uri::parse(href) else { return false };
    // mainThreadUriOpeners.ts `getOpeners`: only web links, after the extensions that open them activate.
    if target.scheme != "http" && target.scheme != "https" {
        return false;
    }
    let Ok(activation) = workbench.update(cx, |workbench, _| workbench.host.activate_by_event(&format!("onOpenExternalUri:{}", target.scheme))) else { return false };
    let _ = activation.await;
    let Ok((openers, configured)) = workbench.update(cx, |workbench, _| {
        let openers: Vec<UriOpener> = workbench.urls.openers.iter().filter(|opener| opener.schemes.contains(&target.scheme)).cloned().collect();
        (openers, workbench.setting(OPENERS_SETTING))
    }) else {
        return false;
    };
    if openers.is_empty() {
        return false;
    }
    let preferred = match allowed {
        ContributedOpeners::Preferred(id) => Some(id.as_str()),
        _ => None,
    };
    if let Some(requested) = requested_opener(&openers, &target, preferred, &configured) {
        return match requested {
            Requested::Default => false,
            Requested::Opener(opener) => open_with(workbench, &opener, href, source, cx).await,
        };
    }
    let mut valid = Vec::new();
    for opener in openers {
        let Ok(answer) = workbench.update(cx, |workbench, _| workbench.host.call("ExtHostUriOpeners", "$canOpenUri", vec![json!(opener.id), source.to_json()])) else { return false };
        // ExternalUriOpenerPriority: None 0, Option 1, Default 2, Preferred 3; a failing opener is left out.
        let priority = answer.await.ok().and_then(Result::ok).and_then(|priority| priority.as_u64()).unwrap_or(0);
        if priority > 0 {
            valid.push((opener, priority));
        }
    }
    valid.sort_by(|a, b| b.1.cmp(&a.1));
    match choose_opener(&valid) {
        Choice::Default => false,
        Choice::Opener(opener) => open_with(workbench, &opener, href, source, cx).await,
        Choice::Prompt => prompt_for_opener(workbench, valid.into_iter().map(|(opener, _)| opener).collect(), href, source, cx).await,
    }
}

enum Requested {
    Default,
    Opener(UriOpener),
}

/// `getRequestedOpener`: the caller's preferred opener, else the first `workbench.externalUriOpeners` glob that
/// matches and names a registered opener or `default`.
fn requested_opener(openers: &[UriOpener], target: &Uri, preferred: Option<&str>, configured: &Value) -> Option<Requested> {
    if preferred == Some(DEFAULT_OPENER_ID) {
        return Some(Requested::Default);
    }
    if let Some(opener) = preferred.and_then(|id| openers.iter().find(|opener| opener.id == id)) {
        return Some(Requested::Opener(opener.clone()));
    }
    for (glob, id) in configured.as_object().into_iter().flatten() {
        if !test_url_matches_glob(target, glob) {
            continue;
        }
        if id == DEFAULT_OPENER_ID {
            return Some(Requested::Default);
        }
        if let Some(opener) = openers.iter().find(|opener| Some(opener.id.as_str()) == id.as_str()) {
            return Some(Requested::Opener(opener.clone()));
        }
    }
    None
}

enum Choice {
    Default,
    Opener(UriOpener),
    Prompt,
}

/// `chooseOpener`: a preferred opener wins, then a single opener of default priority; several prompt.
fn choose_opener(valid: &[(UriOpener, u64)]) -> Choice {
    if let Some((opener, 3)) = valid.first() {
        return Choice::Opener(opener.clone());
    }
    let defaults: Vec<&UriOpener> = valid.iter().filter(|(_, priority)| *priority == 2).map(|(opener, _)| opener).collect();
    match defaults.as_slice() {
        [] => Choice::Default,
        [opener] => Choice::Opener((*opener).clone()),
        _ => Choice::Prompt,
    }
}

/// `showOpenerPrompt`: the openers, the default browser, and a way to configure the default.
async fn prompt_for_opener(workbench: &WeakEntity<Workbench>, openers: Vec<UriOpener>, href: &str, source: &Uri, cx: &mut AsyncWindowContext) -> bool {
    let mut items: Vec<Entry> = openers.iter().enumerate().map(|(handle, opener)| Entry::Item(Item { handle: handle as i64, label: opener.label.clone(), pickable: true, ..Item::default() })).collect();
    let default_handle = openers.len() as i64;
    items.push(Entry::Item(Item { handle: default_handle, label: "Open in Default Browser".into(), pickable: true, ..Item::default() }));
    items.push(Entry::Separator(None));
    items.push(Entry::Item(Item { handle: default_handle + 1, label: "Configure default opener...".into(), pickable: true, ..Item::default() }));
    let mut input = QuickInput::new(0, Kind::QuickPick);
    input.placeholder = Some(format!("How would you like to open: {}", Uri::parse(href).map(|uri| uri.to_string(false)).unwrap_or_else(|_| href.to_owned())));
    input.set_items(items);
    let Ok(picked) = workbench.update_in(cx, |workbench, window, cx| workbench.choose(input, window, cx)) else { return true };
    // A dismissed prompt still cancels the default opener.
    let Some(handle) = picked.await.ok().and_then(Result::ok).and_then(|picked| picked.as_i64()) else { return true };
    if handle == default_handle {
        return false;
    }
    if handle == default_handle + 1 {
        let args = vec![json!({ "revealSetting": { "key": OPENERS_SETTING, "edit": true } })];
        let _ = workbench.update_in(cx, |workbench, window, cx| {
            let outcome = workbench.execute_command("workbench.action.openSettingsJson", args, window, cx);
            super::keyboard::report_failure(outcome, cx);
        });
        return true;
    }
    open_with(workbench, &openers[handle as usize], href, source, cx).await
}

/// mainThreadUriOpeners.ts `openExternalUri`: a failure offers the default opener.
async fn open_with(workbench: &WeakEntity<Workbench>, opener: &UriOpener, href: &str, source: &Uri, cx: &mut AsyncWindowContext) -> bool {
    let Ok(resolved) = Uri::parse(href) else { return false };
    let context = json!({ "resolvedUri": resolved.to_json(), "sourceUri": source.to_json() });
    let Ok(answer) = workbench.update(cx, |workbench, _| workbench.host.call("ExtHostUriOpeners", "$openUri", vec![json!(opener.id), context])) else { return true };
    let Ok(Err(error)) = answer.await else { return true };
    let error = error["message"].as_str().map_or_else(|| error.to_string(), |message| format!("Error: {message}"));
    let message = format!("Could not open uri with '{}': {error}", opener.id);
    let Ok(choice) = workbench.update(cx, |workbench, cx| {
        let choice = workbench.prompt_notification(Severity::Error, message, &["Open using default opener"]);
        cx.notify();
        choice
    }) else {
        return true;
    };
    if let Ok(Ok(Value::Number(_))) = choice.await {
        let options = OpenOptions { contributed_openers: ContributedOpeners::Preferred(DEFAULT_OPENER_ID.into()), ..OpenOptions::default() };
        let target = Target::Text(href.to_owned());
        let _ = workbench.update_in(cx, |workbench, window, cx| drop(workbench.open_link(target, options, window, cx)));
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selection(link: &str) -> (Option<Value>, String) {
        let (selection, uri) = extract_selection(Uri::parse(link).unwrap());
        (selection, uri.to_string(false))
    }

    // openerService.test.ts `delegate to editorService, scheme:///fff#L123`.
    #[test]
    fn a_line_fragment_becomes_the_selection() {
        assert_eq!(selection("file:///somepath"), (None, "file:///somepath".into()));
        let caret = |line, column| Some(json!({ "startLineNumber": line, "startColumn": column, "endLineNumber": line, "endColumn": column }));
        assert_eq!(selection("file:///somepath#L23"), (caret(23, 1), "file:///somepath".into()));
        assert_eq!(selection("another:///somepath#L23,45"), (caret(23, 45), "another:/somepath".into()), "uri.ts writes `//` only for `file` or an authority");
        assert_eq!(selection("file:///somepath#23,45"), (caret(23, 45), "file:///somepath".into()));
        assert_eq!(selection("file:///somepath#23,-5").0, caret(23, 1));
        let range = json!({ "startLineNumber": 73, "startColumn": 84, "endLineNumber": 83, "endColumn": 52 });
        assert_eq!(selection("file:///some/file.js#L73,84-L83,52").0, Some(range));
        let range = json!({ "startLineNumber": 73, "startColumn": 1, "endLineNumber": 83, "endColumn": 1 });
        assert_eq!(selection("file:///some/file.js#73-83").0, Some(range));
        assert_eq!(selection("file:///somepath#top").0, None);
    }

    #[test]
    fn a_long_link_keeps_the_end_of_its_query() {
        let uri = Uri::parse("https://example.com/path?a=1").unwrap();
        assert_eq!(formatted_link(&uri), "https://example.com/path?a=1");
        let uri = Uri::parse(&format!("https://example.com/{}?q={}xyz", "p".repeat(50), "0".repeat(20))).unwrap();
        assert_eq!(formatted_link(&uri), format!("https://example.com/{}?...0xyz", "p".repeat(50)));
    }

    fn opener(id: &str) -> UriOpener {
        UriOpener { id: id.into(), schemes: vec!["https".into()], label: id.into() }
    }

    #[test]
    fn a_preferred_opener_wins_and_several_defaults_prompt() {
        let pick = |valid: &[(UriOpener, u64)]| match choose_opener(valid) {
            Choice::Default => "default".to_owned(),
            Choice::Opener(opener) => opener.id,
            Choice::Prompt => "prompt".to_owned(),
        };
        assert_eq!(pick(&[]), "default");
        assert_eq!(pick(&[(opener("a"), 1)]), "default");
        assert_eq!(pick(&[(opener("a"), 2), (opener("b"), 1)]), "a");
        assert_eq!(pick(&[(opener("a"), 2), (opener("b"), 2)]), "prompt");
        assert_eq!(pick(&[(opener("a"), 3), (opener("b"), 2)]), "a");
    }

    #[test]
    fn the_configured_glob_names_the_opener() {
        let openers = [opener("a"), opener("b")];
        let target = Uri::parse("https://localhost:3000/page").unwrap();
        let requested = |preferred, configured: Value| match requested_opener(&openers, &target, preferred, &configured) {
            None => "none".to_owned(),
            Some(Requested::Default) => "default".to_owned(),
            Some(Requested::Opener(opener)) => opener.id,
        };
        assert_eq!(requested(None, json!({})), "none");
        assert_eq!(requested(Some("b"), json!({})), "b");
        assert_eq!(requested(Some("default"), json!({ "localhost:3000": "a" })), "default");
        assert_eq!(requested(None, json!({ "localhost:3000": "a" })), "a");
        assert_eq!(requested(None, json!({ "localhost:*": "default" })), "default");
        assert_eq!(requested(None, json!({ "example.com": "a", "localhost:3000": "missing" })), "none");
    }
}
