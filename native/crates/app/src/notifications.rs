//! The notifications model of workbench/common/notifications.ts: `NotificationsModel` and `NotificationViewItem`
//! as plain data. The workbench reacts to what each call returns where upstream fires events.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

/// base/common/severity.ts, as `$showMessage` sends it: Info 1, Warning 2, Error 3.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

impl Severity {
    pub fn from_number(value: &Value) -> Severity {
        match value.as_u64() {
            Some(3) => Severity::Error,
            Some(2) => Severity::Warning,
            _ => Severity::Info,
        }
    }

    /// notificationsViewer.ts `NotificationTemplateRenderer.toSeverityIcon`.
    pub fn codicon(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Error => "error",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Priority {
    Default,
    Silent,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Text(String),
    Link { label: String, href: String, title: Option<String> },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub raw: String,
    pub nodes: Vec<Node>,
}

impl Message {
    /// `LinkedText.toString`: the labels in place of the links.
    pub fn text(&self) -> String {
        self.nodes.iter().map(|node| match node {
            Node::Text(text) => text.as_str(),
            Node::Link { label, .. } => label.as_str(),
        }).collect()
    }
}

/// What an action does when it runs; the workbench carries it out.
#[derive(Clone, Debug, PartialEq)]
pub enum Run {
    /// `$showMessage` resolves with the extension's item handle.
    Reply(u64),
    Command(String, Vec<Value>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Action {
    pub id: String,
    pub label: String,
    pub run: Run,
}

/// `INotificationViewItemProgressState`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Progress {
    pub infinite: bool,
    pub total: Option<f64>,
    pub worked: Option<f64>,
}

impl Progress {
    pub fn set_infinite(&mut self) {
        *self = Progress { infinite: true, ..Progress::default() };
    }

    pub fn set_total(&mut self, total: f64) {
        *self = Progress { total: Some(total), worked: self.worked, ..Progress::default() };
    }

    pub fn add_worked(&mut self, worked: f64) {
        *self = Progress { total: self.total, worked: Some(self.worked.unwrap_or(0.) + worked), ..Progress::default() };
    }
}

/// `INotification`.
#[derive(Clone, Debug)]
pub struct Notification {
    pub id: Option<String>,
    pub severity: Severity,
    pub message: String,
    /// `legacyExtensionLinkParsing`: markdown links in the text become links (extension messages and progress).
    pub legacy_links: bool,
    pub source: Option<String>,
    pub source_id: Option<String>,
    pub sticky: bool,
    pub priority: Priority,
    pub primary: Vec<Action>,
    pub secondary: Vec<Action>,
    pub progress: Option<Progress>,
}

impl Notification {
    pub fn new(severity: Severity, message: impl Into<String>) -> Notification {
        Notification {
            id: None,
            severity,
            message: message.into(),
            legacy_links: false,
            source: None,
            source_id: None,
            sticky: false,
            priority: Priority::Default,
            primary: Vec::new(),
            secondary: Vec::new(),
            progress: None,
        }
    }
}

/// `NotificationViewItem`; `key` names it for the workbench, `id` is the caller's dedupe id.
#[derive(Clone, Debug)]
pub struct Item {
    pub key: u64,
    pub id: Option<String>,
    pub severity: Severity,
    sticky: bool,
    pub priority: Priority,
    pub message: Message,
    legacy_links: bool,
    pub source: Option<String>,
    pub source_id: Option<String>,
    pub primary: Vec<Action>,
    pub secondary: Vec<Action>,
    pub expanded: bool,
    pub progress: Option<Progress>,
}

const MAX_MESSAGE_LENGTH: usize = 1000;

impl Item {
    pub fn can_collapse(&self) -> bool {
        self.primary.is_empty()
    }

    /// Explicitly sticky, an error with actions, expanded without actions, or with running progress.
    pub fn sticky(&self) -> bool {
        let has_actions = !self.primary.is_empty();
        self.sticky || (has_actions && self.severity == Severity::Error) || (!has_actions && self.expanded) || self.has_active_progress()
    }

    pub fn has_active_progress(&self) -> bool {
        self.progress.as_ref().is_some_and(|state| state.infinite || state.total.is_some() || state.worked.is_some())
    }

    /// Whether the expansion changed, as `onDidChangeExpansion` fires.
    pub fn expand(&mut self) -> bool {
        if self.expanded || !self.can_collapse() {
            return false;
        }
        self.expanded = true;
        true
    }

    pub fn collapse(&mut self) -> bool {
        if !self.expanded || !self.can_collapse() {
            return false;
        }
        self.expanded = false;
        true
    }

    pub fn toggle(&mut self) -> bool {
        if self.expanded { self.collapse() } else { self.expand() }
    }

    pub fn update_message(&mut self, message: &str) -> bool {
        let Some(message) = parse_message(message, self.legacy_links) else { return false };
        if message == self.message {
            return false;
        }
        self.message = message;
        true
    }

    pub fn set_actions(&mut self, primary: Vec<Action>, secondary: Vec<Action>) {
        self.expanded = !primary.is_empty();
        self.primary = primary;
        self.secondary = secondary;
    }

    fn equals(&self, other: &Item) -> bool {
        if self.progress.is_some() || other.progress.is_some() {
            return false;
        }
        if self.id.is_some() || other.id.is_some() {
            return self.id == other.id;
        }
        if self.source != other.source || self.source_id != other.source_id || self.message != other.message {
            return false;
        }
        let actions = |item: &Item| item.primary.iter().map(|action| format!("{}{}", action.id, action.label)).collect::<Vec<_>>();
        actions(self) == actions(other)
    }
}

/// `NotificationsModel` with its filter: `do_not_disturb` is `NotificationsFilter.ERROR` globally, `muted_sources`
/// the sources filtered to errors.
#[derive(Default)]
pub struct Notifications {
    /// Newest first.
    pub items: Vec<Item>,
    pub do_not_disturb: bool,
    pub muted_sources: BTreeSet<String>,
    next_key: u64,
}

pub struct Added {
    pub key: u64,
    /// The equal notification the new one replaced, closed.
    pub replaced: Option<Item>,
}

impl Notifications {
    /// `addNotification`: `None` for a notification without a message.
    pub fn add(&mut self, notification: Notification) -> Option<Added> {
        let message = parse_message(&notification.message, notification.legacy_links)?;
        let mut priority = notification.priority;
        if priority == Priority::Default && notification.severity != Severity::Error {
            let muted = notification.source_id.as_ref().is_some_and(|id| self.muted_sources.contains(id));
            if self.do_not_disturb || muted {
                priority = Priority::Silent;
            }
        }
        self.next_key += 1;
        let mut item = Item {
            key: self.next_key,
            id: notification.id,
            severity: notification.severity,
            sticky: notification.sticky,
            priority,
            message,
            legacy_links: notification.legacy_links,
            source: notification.source,
            source_id: notification.source_id,
            primary: Vec::new(),
            secondary: Vec::new(),
            expanded: false,
            progress: notification.progress,
        };
        item.set_actions(notification.primary, notification.secondary);
        let replaced = self.items.iter().position(|other| other.equals(&item)).map(|index| self.items.remove(index));
        self.items.insert(0, item);
        Some(Added { key: self.next_key, replaced })
    }

    pub fn close(&mut self, key: u64) -> Option<Item> {
        let index = self.items.iter().position(|item| item.key == key)?;
        Some(self.items.remove(index))
    }

    pub fn item(&self, key: u64) -> Option<&Item> {
        self.items.iter().find(|item| item.key == key)
    }

    pub fn item_mut(&mut self, key: u64) -> Option<&mut Item> {
        self.items.iter_mut().find(|item| item.key == key)
    }
}

/// `parseNotificationMessage`: `None` for an empty message.
fn parse_message(input: &str, legacy_links: bool) -> Option<Message> {
    if input.is_empty() {
        return None;
    }
    let mut nodes = normalize(vec![Node::Text(input.to_owned())]);
    if legacy_links {
        nodes = parse_linked_text(&Message { raw: String::new(), nodes }.text());
    }
    Some(Message { raw: input.to_owned(), nodes })
}

/// `normalizeLinkedText`: newlines become spaces, the text stops at 1000 characters with "...", the ends trimmed.
fn normalize(input: Vec<Node>) -> Vec<Node> {
    let mut nodes: Vec<Node> = Vec::new();
    let mut length = 0;
    for node in input {
        let text = match &node {
            Node::Text(text) => text,
            Node::Link { label, .. } => label,
        };
        let count = text.chars().count();
        let value: String = text.chars().take(MAX_MESSAGE_LENGTH.saturating_sub(length)).collect();
        let value = value.replace("\r\n", " ").replace(['\n', '\r'], " ");
        if !value.is_empty() {
            match (node, nodes.last_mut()) {
                (Node::Text(_), Some(Node::Text(last))) => last.push_str(&value),
                (Node::Text(_), _) => nodes.push(Node::Text(value)),
                (Node::Link { href, title, .. }, _) => nodes.push(Node::Link { label: value, href, title }),
            }
        }
        length += count;
        if length > MAX_MESSAGE_LENGTH {
            nodes.push(Node::Text("...".into()));
            break;
        }
    }
    if let Some(Node::Text(first)) = nodes.first_mut() {
        *first = first.trim_start().to_owned();
    }
    if let Some(Node::Text(last)) = nodes.last_mut() {
        *last = last.trim_end().to_owned();
    }
    nodes.retain(|node| *node != Node::Text(String::new()));
    nodes
}

/// base/common/linkedText.ts `parseLinkedText`; the title's quote backreference is spelled as two alternatives.
pub fn parse_linked_text(text: &str) -> Vec<Node> {
    static LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?i)\[([^\]]+)\]\(((?:https?://|command:|file:)[^)\s]+)(?: "(.+?)"| '(.+?)')?\)"#).unwrap());
    let mut nodes = Vec::new();
    let mut index = 0;
    for captures in LINK.captures_iter(text) {
        let whole = captures.get(0).unwrap();
        if whole.start() > index {
            nodes.push(Node::Text(text[index..whole.start()].to_owned()));
        }
        let title = captures.get(3).or(captures.get(4)).map(|title| title.as_str().to_owned());
        nodes.push(Node::Link { label: captures[1].to_owned(), href: captures[2].to_owned(), title });
        index = whole.end();
    }
    if index < text.len() {
        nodes.push(Node::Text(text[index..].to_owned()));
    }
    nodes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &str) -> Node {
        Node::Text(value.into())
    }

    fn link(label: &str, href: &str, title: Option<&str>) -> Node {
        Node::Link { label: label.into(), href: href.into(), title: title.map(str::to_owned) }
    }

    fn action(id: &str) -> Action {
        Action { id: id.into(), label: "label".into(), run: Run::Command(id.into(), vec![]) }
    }

    fn add(model: &mut Notifications, notification: Notification) -> Item {
        let key = model.add(notification).unwrap().key;
        model.item(key).unwrap().clone()
    }

    #[test]
    fn renders_text_literally_without_legacy_parsing() {
        let mut model = Notifications::default();
        let item = add(&mut model, Notification::new(Severity::Info, "[Show Logs](command:python.viewOutput)"));
        assert_eq!(item.message.nodes, vec![text("[Show Logs](command:python.viewOutput)")]);
    }

    #[test]
    fn legacy_link_parsing_and_update() {
        let mut model = Notifications::default();
        let notification = Notification { legacy_links: true, ..Notification::new(Severity::Info, "  [Show Logs](command:python.viewOutput)\r\n[Help](https://example.com)  ") };
        let mut item = add(&mut model, notification);
        assert_eq!(item.message.nodes, vec![link("Show Logs", "command:python.viewOutput", None), text(" "), link("Help", "https://example.com", None)]);
        assert!(item.update_message(r#"[Check details](command:java.show.server.task.status "Build Status")"#));
        assert_eq!(item.message.nodes, vec![link("Check details", "command:java.show.server.task.status", Some("Build Status"))]);
        assert!(!item.update_message(r#"[Check details](command:java.show.server.task.status "Build Status")"#));
    }

    #[test]
    fn normalizes_and_truncates_messages() {
        let message = format!("  line one\r\nline two\rline three\n{}", "x".repeat(1000));
        let mut model = Notifications::default();
        let item = add(&mut model, Notification::new(Severity::Info, message.clone()));
        let shown: String = message.chars().take(1000).collect::<String>().replace("\r\n", " ").replace(['\n', '\r'], " ");
        assert_eq!(item.message.raw, message);
        assert_eq!(item.message.text(), format!("{}...", shown.trim_start()));
        let legacy = Notification { legacy_links: true, ..Notification::new(Severity::Info, format!("{}[Open](command:unexpected)", "x".repeat(995))) };
        let item = add(&mut model, legacy);
        assert_eq!(item.message.nodes, vec![text(&format!("{}[Open...", "x".repeat(995)))]);
    }

    #[test]
    fn items() {
        let mut model = Notifications::default();
        assert!(model.add(Notification::new(Severity::Error, "")).is_none());

        let error = || Notification::new(Severity::Error, "Error Message");
        let item1 = add(&mut model, error());
        let item2 = add(&mut model, error());
        let item3 = add(&mut model, Notification::new(Severity::Info, "Info Message"));
        let item4 = add(&mut model, Notification { source: Some("Source".into()), ..error() });
        let item5 = add(&mut model, Notification { primary: vec![action("id")], ..error() });
        let item6 = add(&mut model, Notification { primary: vec![action("id")], progress: Some(Progress { infinite: true, ..Progress::default() }), ..error() });
        assert!(item1.equals(&item2));
        assert!(!item1.equals(&item3));
        assert!(!item1.equals(&item4));
        assert!(!item1.equals(&item5));
        assert!(!item6.equals(&item6));
        let same = |severity, message| Notification { id: Some("same".into()), ..Notification::new(severity, message) };
        let id1 = add(&mut model, same(Severity::Info, "Info Message"));
        let id2 = add(&mut model, same(Severity::Error, "Error Message"));
        assert!(id1.equals(&id2));
        assert!(!id1.equals(&item3));

        assert!(!item1.has_active_progress());
        assert!(item6.has_active_progress());
        assert!(!item5.can_collapse());
        assert!(item5.expanded);

        let mut item1 = item1;
        assert_eq!([item1.expand(), item1.expand(), item1.collapse(), item1.collapse()], [true, false, true, false]);
        let mut progress = Progress::default();
        progress.set_infinite();
        assert!(progress.infinite);
        progress.set_total(10.);
        progress.add_worked(3.);
        progress.add_worked(2.);
        assert_eq!((progress.total, progress.worked), (Some(10.), Some(5.)));

        let filtered = |model: &mut Notifications, notification| add(model, notification).priority;
        let mut dnd = Notifications { do_not_disturb: true, ..Notifications::default() };
        assert_eq!(filtered(&mut dnd, error()), Priority::Default);
        assert_eq!(filtered(&mut dnd, Notification::new(Severity::Warning, "Error Message")), Priority::Silent);
        let mut muted = Notifications { muted_sources: BTreeSet::from(["test.source".to_owned()]), ..Notifications::default() };
        let warning = |source: &str, id: Option<&str>| Notification { source: Some(source.into()), source_id: id.map(str::to_owned), ..Notification::new(Severity::Warning, "Error Message") };
        assert_eq!(filtered(&mut muted, warning("test.source", None)), Priority::Default);
        assert_eq!(filtered(&mut muted, warning("foo", Some("test.source"))), Priority::Silent);
        assert_eq!(filtered(&mut muted, warning("foo", Some("test.source2"))), Priority::Default);
    }

    #[test]
    fn model_dedupes_and_closes() {
        let mut model = Notifications::default();
        let first = model.add(Notification::new(Severity::Error, "Error Message")).unwrap();
        assert!(first.replaced.is_none());
        let second = model.add(Notification::new(Severity::Error, "Error Message")).unwrap();
        assert_eq!(second.replaced.map(|item| item.key), Some(first.key));
        model.add(Notification::new(Severity::Info, "Info Message"));
        assert_eq!(model.items.len(), 2);
        assert_eq!(model.items[0].message.raw, "Info Message");
        assert!(model.close(second.key).is_some());
        assert!(model.close(second.key).is_none());
        assert_eq!(model.items.len(), 1);
    }

    #[test]
    fn sticky() {
        let mut model = Notifications::default();
        let error_with_action = add(&mut model, Notification { primary: vec![action("a")], ..Notification::new(Severity::Error, "e") });
        assert!(error_with_action.sticky());
        let mut info = add(&mut model, Notification::new(Severity::Info, "i"));
        assert!(!info.sticky());
        info.expand();
        assert!(info.sticky());
    }
}
