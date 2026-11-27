//! url.contribution.ts `OpenUrlAction`, and trustedDomains.ts `manageTrustedDomainSettingsCommand` with the
//! `trustedDomains:` document of trustedDomainsFileSystemProvider.ts.

use std::path::{Path, PathBuf};

use futures::channel::oneshot;
use gpui::{Context, Window};
use serde_json::{Value, json};

use super::Workbench;
use super::opener::{ContributedOpeners, OpenOptions, Target};
use crate::commands::Outcome;
use crate::editor::TextDocument;
use crate::quick_input::{Kind, QuickInput};
use crate::smart_snippet_inserter::offset_to_position;
use crate::trusted_domains;
use crate::uri::Uri;
use config::json::{ParseOptions, Scanner, SyntaxKind};

const OPEN_URL_STORAGE_KEY: &str = "workbench.action.url.openUrl.lastInput";
pub const TRUSTED_DOMAINS_SCHEME: &str = "trustedDomains";
const TRUSTED_DOMAINS_PATH: &str = "/Trusted Domains";

const CONFIG_HELP_TEXT_PRE: &str = r#"// Links matching one or more entries in the list below can be opened without link protection.
// The following examples show what entries can look like:
// - "https://microsoft.com": Matches this specific domain using https
// - "https://microsoft.com:8080": Matches this specific domain on this port using https
// - "https://microsoft.com:*": Matches this specific domain on any port using https
// - "https://microsoft.com/foo": Matches https://microsoft.com/foo and https://microsoft.com/foo/bar,
//   but not https://microsoft.com/foobar or https://microsoft.com/bar
// - "https://*.microsoft.com": Match all domains ending in "microsoft.com" using https
// - "microsoft.com": Match this specific domain using either http or https
// - "*.microsoft.com": Match all domains ending in "microsoft.com" using either http or https
// - "http://192.168.0.1: Matches this specific IP using http
// - "http://192.168.0.*: Matches all IP's with this prefix using http
// - "*": Match all domains using either http or https
//
"#;
const CONFIG_HELP_TEXT_AFTER: &str = r#"//
// You can use the "Manage Trusted Domains" command to open this file.
// Save this file to apply the trusted domains rules.
"#;
const CONFIG_PLACEHOLDER_TEXT: &str = "[\n\t// \"https://microsoft.com\"\n]";

impl Workbench {
    pub(super) fn register_url_commands(&mut self) {
        self.commands.register("workbench.action.url.openUrl", |workbench, _, window, cx| {
            workbench.open_url_action(window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.manageTrustedDomain", |workbench, _, window, cx| {
            workbench.open_trusted_domains(None, window, cx);
            Outcome::done()
        });
    }

    fn open_url_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let last = self.workspace_storage.lock().unwrap().get(OPEN_URL_STORAGE_KEY).unwrap_or_default().to_owned();
        let mut input = QuickInput::new(0, Kind::InputBox);
        input.prompt = Some("URL to open".into());
        input.set_value(last);
        let answer = self.ask_input(input, window, cx);
        cx.spawn_in(window, async move |workbench, cx| {
            let Ok(Ok(Value::String(text))) = answer.await else { return };
            if text.is_empty() {
                return;
            }
            let _ = workbench.update_in(cx, |workbench, window, cx| {
                let options = OpenOptions { contributed_openers: ContributedOpeners::Any, ..OpenOptions::default() };
                drop(workbench.open_link(Target::Text(text.clone()), options, window, cx));
                workbench.workspace_storage.lock().unwrap().set(OPEN_URL_STORAGE_KEY, text);
            });
        })
        .detach();
    }

    /// trustedDomains.ts `openInEditor`: pinned, as jsonc, the caret on the first entry of the list.
    pub(super) fn open_trusted_domains(&mut self, configuring: Option<&Uri>, window: &mut Window, cx: &mut Context<Self>) {
        let path = PathBuf::from(TRUSTED_DOMAINS_PATH);
        self.leave_locked_group(TRUSTED_DOMAINS_SCHEME, &path);
        if self.editors.model(TRUSTED_DOMAINS_SCHEME, &path).is_none() {
            let content = self.trusted_domains_content(configuring.map(|uri| uri.to_string(false)));
            let settings = self.settings.clone();
            let settings = settings.lock().unwrap();
            let document = TextDocument::from_bytes(TRUSTED_DOMAINS_SCHEME, &path, content.as_bytes(), None, &self.languages, &settings);
            self.editors.open(document, false);
            self.editors.active_mut().expect("just opened").set_language("jsonc", &self.languages, &settings);
        }
        let model = self.editors.model(TRUSTED_DOMAINS_SCHEME, &path).expect("opened above").document();
        let text = model.lines.join(&model.eol);
        let mut options = json!({ "pinned": true });
        if let Some(offset) = first_entry_offset(&text) {
            let position = offset_to_position(&model.lines, &model.eol, offset);
            let (line, column) = (position.line + 1, position.column + 1);
            options["selection"] = json!({ "startLineNumber": line, "startColumn": column, "endLineNumber": line, "endColumn": column });
        }
        drop(model);
        let uri = json!({ "$mid": 1, "scheme": TRUSTED_DOMAINS_SCHEME, "path": TRUSTED_DOMAINS_PATH });
        if let Err(error) = self.try_show_text_document(&uri, &options, window, cx) {
            self.notify_error(error["message"].as_str().unwrap_or_default().to_owned());
        }
    }

    /// `readFile`: the stored text while it still holds the help and every trusted domain, else a fresh one.
    fn trusted_domains_content(&self, configuring: Option<String>) -> String {
        let trusted = self.trusted_domains();
        let stored = self.application_storage.lock().unwrap().get(trusted_domains::CONTENT_STORAGE_KEY).map(str::to_owned);
        if let Some(stored) = stored {
            let current = stored.contains(CONFIG_HELP_TEXT_PRE)
                && stored.contains(CONFIG_HELP_TEXT_AFTER)
                && stored.contains(configuring.as_deref().unwrap_or_default())
                && trusted.iter().all(|domain| stored.contains(domain.as_str()));
            if current {
                return stored;
            }
        }
        compute_trusted_domain_content(&trusted, configuring.as_deref())
    }

    /// `writeFile`: the text as typed, and its list; text that holds no value stores an empty list string.
    pub(super) fn save_trusted_domains(&mut self, path: &Path, cx: &mut Context<Self>) -> oneshot::Receiver<bool> {
        let (sender, receiver) = oneshot::channel();
        let model = self.editors.model(TRUSTED_DOMAINS_SCHEME, path).expect("saving an open model");
        let version = model.alternative_version_id();
        let text = model.document().lines.join(&model.document().eol);
        let domains = config::json::parse(&text, &mut Vec::new(), ParseOptions::default()).map(|domains| domains.to_string());
        let mut storage = self.application_storage.lock().unwrap();
        storage.set(trusted_domains::CONTENT_STORAGE_KEY, text);
        storage.set(trusted_domains::STORAGE_KEY, domains.unwrap_or_default());
        drop(storage);
        let _ = sender.send(self.finish_save(TRUSTED_DOMAINS_SCHEME, path, version, Ok(()), cx));
        receiver
    }
}

/// `computeTrustedDomainContent` with no default trusted domains.
fn compute_trusted_domain_content(trusted: &[String], configuring: Option<&str>) -> String {
    let mut content = CONFIG_HELP_TEXT_PRE.to_owned();
    content.push_str("// By default, VS Code trusts \"localhost\".\n");
    content.push_str(CONFIG_HELP_TEXT_AFTER);
    if let Some(configuring) = configuring {
        content.push_str(&format!("\n// Currently configuring trust for {configuring}\n"));
    }
    if trusted.is_empty() {
        content.push_str(CONFIG_PLACEHOLDER_TEXT);
    } else {
        content.push_str(&serde_json::to_string_pretty(trusted).expect("strings serialize"));
    }
    content
}

/// The first token after `[`, or just after `[` for an empty list.
fn first_entry_offset(text: &str) -> Option<usize> {
    let mut scanner = Scanner::new(text, true);
    loop {
        match scanner.scan() {
            SyntaxKind::Eof => return None,
            SyntaxKind::OpenBracketToken => break,
            _ => {}
        }
    }
    let after_bracket = scanner.token_offset() + scanner.token_length();
    match scanner.scan() {
        SyntaxKind::Eof | SyntaxKind::CloseBracketToken => Some(after_bracket),
        _ => Some(scanner.token_offset()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_caret_goes_after_the_bracket_of_an_empty_list() {
        let content = compute_trusted_domain_content(&[], None);
        assert_eq!(first_entry_offset(&content), Some(content.find('[').unwrap() + 1));
    }

    #[test]
    fn the_caret_goes_on_the_first_domain() {
        let content = compute_trusted_domain_content(&["https://a.com".into()], Some("https://b.com/"));
        assert!(content.contains("// Currently configuring trust for https://b.com/\n"));
        assert_eq!(first_entry_offset(&content), Some(content.find("\"https://a.com\"").unwrap()));
    }
}
