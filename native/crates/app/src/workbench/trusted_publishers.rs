//! The trusted publishers of extensionManagementService.ts: `checkForTrustedPublishers` asks before a gallery install
//! from a publisher the user has not trusted, `extensions.trustedPublishers` (application storage) keeps the trusted
//! ones, and `workbench.extensions.action.manageTrustedPublishers` (extensions.contribution.ts) untrusts them.

use exthost::gallery::{ExtensionInfo, Gallery, GalleryExtension};
use futures::channel::oneshot;
use gpui::Context;
use serde_json::{Map, Value, json};

use super::Workbench;
use super::dialog_view::{CustomDialog, DialogButton};
use crate::commands::Outcome;
use crate::quick_input::{Item, Kind, QuickInput};

const STORAGE_KEY: &str = "extensions.trustedPublishers";
const LEARN_MORE: &str = "https://aka.ms/vscode-extension-security";
const UNVERIFIED_LINK: &str = "https://aka.ms/vscode-verify-publisher";
/// `CancellationError`'s message: a canceled install shows no error.
pub(super) const CANCELED: &str = "Canceled";

/// `IPublisherInfo` with the parts of `IGalleryExtension` the dialog shows.
#[derive(Clone)]
pub(super) struct Publisher {
    publisher: String,
    display_name: String,
    link: Option<String>,
    domain: Option<(String, bool)>,
}

impl Publisher {
    fn of(extension: &GalleryExtension) -> Self {
        Publisher { publisher: extension.publisher.clone(), display_name: extension.publisher_display_name.clone(), link: extension.publisher_link.clone(), domain: extension.publisher_domain.clone() }
    }
}

impl Workbench {
    pub(super) fn register_trusted_publisher_commands(&mut self) {
        self.commands.register("workbench.extensions.action.manageTrustedPublishers", |workbench, _, window, cx| {
            let trusted = workbench.trusted_publishers();
            let mut items: Vec<(String, String)> = trusted.iter().map(|(publisher, info)| (publisher.clone(), info["publisherDisplayName"].as_str().unwrap_or_default().to_owned())).collect();
            items.sort_by(|a, b| crate::command_palette::locale_compare(&a.1, &b.1));
            let mut input = QuickInput::new(0, Kind::QuickPick);
            input.can_select_many = true;
            input.title = Some("Manage Trusted Extension Publishers".into());
            input.placeholder = Some("Choose which publishers to trust".into());
            input.set_items(items.iter().enumerate().map(|(handle, (publisher, label))| crate::quick_input::Entry::Item(Item { handle: handle as i64, label: label.clone(), description: Some(publisher.clone()), pickable: true, picked: true, ..Item::default() })).collect());
            let picked = workbench.choose(input, window, cx);
            cx.spawn(async move |workbench, cx| {
                let Ok(Ok(Value::Array(picked))) = picked.await else { return };
                let untrusted: Vec<String> = items.iter().enumerate().filter(|(handle, _)| !picked.contains(&json!(handle))).map(|(_, (publisher, _))| publisher.clone()).collect();
                let _ = workbench.update(cx, |workbench, _| workbench.untrust_publishers(&untrusted));
            })
            .detach();
            Outcome::done()
        });
    }

    /// `getTrustedPublishersFromStorage`: lower-case publisher to its `IPublisherInfo`; an array (an old format) reads
    /// as none.
    fn trusted_publishers(&self) -> Map<String, Value> {
        match self.application_storage.lock().unwrap().get(STORAGE_KEY).and_then(|raw| serde_json::from_str::<Value>(raw).ok()) {
            Some(Value::Object(trusted)) => trusted.into_iter().map(|(publisher, info)| (publisher.to_lowercase(), info)).collect(),
            _ => Map::new(),
        }
    }

    /// `isPublisherUserTrusted` for each lower-case publisher; the port's product has no `trustedExtensionPublishers`.
    pub(super) fn user_trusted_publishers(&self) -> Vec<String> {
        self.trusted_publishers().keys().cloned().collect()
    }

    /// `trustPublishers`.
    pub(super) fn trust_publishers(&self, publishers: &[(String, String)]) {
        let mut trusted = self.trusted_publishers();
        for (publisher, display_name) in publishers {
            trusted.insert(publisher.to_lowercase(), json!({ "publisher": publisher, "publisherDisplayName": display_name }));
        }
        self.application_storage.lock().unwrap().set(STORAGE_KEY, Value::Object(trusted).to_string());
    }

    fn untrust_publishers(&self, publishers: &[String]) {
        let mut trusted = self.trusted_publishers();
        for publisher in publishers {
            trusted.remove(&publisher.to_lowercase());
        }
        self.application_storage.lock().unwrap().set(STORAGE_KEY, Value::Object(trusted).to_string());
    }

    /// `checkForTrustedPublishers` of one extension with its pack and dependencies: `None` when its publisher is
    /// trusted, else the dialog's answer, true once "Trust Publisher & Install" trusted every publisher it names.
    pub(super) fn check_trusted_publishers(&mut self, extension: &GalleryExtension, others: Vec<Publisher>, cx: &mut Context<Self>) -> Option<oneshot::Receiver<bool>> {
        if extension.private || self.user_trusted_publishers().contains(&extension.publisher.to_lowercase()) {
            return None;
        }
        let mut publishers = vec![Publisher::of(extension)];
        publishers.extend(others.iter().cloned());
        let install = if publishers.len() > 1 { "Trust Publishers & Install" } else { "Trust Publisher & Install" };
        let dialog = CustomDialog {
            icon: "warning",
            message: title(&publishers),
            detail: None,
            details: details(extension, &others, &publishers),
            checkbox: None,
            buttons: [install, "Learn More", "Cancel"].map(|label| DialogButton { label: label.into(), detail: None }).into(),
            cancel: Some(2),
        };
        let answer = self.show_custom_dialog(dialog, cx);
        let (sender, receiver) = oneshot::channel();
        cx.spawn(async move |workbench, cx| {
            let button = answer.await.map_or(2, |result| result.button);
            let trusted = button == 0;
            let _ = workbench.update(cx, |workbench, cx| match button {
                0 => workbench.trust_publishers(&publishers.iter().map(|publisher| (publisher.publisher.clone(), publisher.display_name.clone())).collect::<Vec<_>>()),
                1 => cx.open_url(LEARN_MORE),
                _ => {}
            });
            let _ = sender.send(trusted);
        })
        .detach();
        Some(receiver)
    }
}

/// `getOtherUntrustedPublishers`: the publishers of the extension's pack and dependencies, recursively, that are
/// neither its own nor trusted, one per display name. Runs in the background: it queries the gallery.
pub(super) fn other_untrusted_publishers(gallery: &Gallery, extension: &GalleryExtension, trusted: &[String]) -> Vec<Publisher> {
    let mut ids: Vec<String> = Vec::new();
    for id in extension.extension_pack.iter().chain(&extension.dependencies) {
        let publisher = id.split('.').next().unwrap_or_default().to_lowercase();
        if publisher != extension.publisher.to_lowercase() && !trusted.contains(&publisher) && !ids.contains(&id.to_lowercase()) {
            ids.push(id.to_lowercase());
        }
    }
    let mut found: Vec<GalleryExtension> = Vec::new();
    // `getDependenciesAndPackedExtensionsRecursively`
    while !ids.is_empty() {
        let infos: Vec<ExtensionInfo> = ids.iter().map(|id| ExtensionInfo { id: id.clone(), ..ExtensionInfo::default() }).collect();
        let extensions = gallery.get_extensions(&infos, false).unwrap_or_default();
        ids.clear();
        for extension in extensions {
            for id in extension.extension_pack.iter().chain(&extension.dependencies) {
                let id = id.to_lowercase();
                if !found.iter().any(|found| found.id == id) && !ids.contains(&id) {
                    ids.push(id);
                }
            }
            if !found.iter().any(|found| found.id == extension.id) {
                found.push(extension);
            }
        }
        ids.retain(|id| !found.iter().any(|found| found.id == *id));
    }
    let mut publishers: Vec<Publisher> = Vec::new();
    for extension in found {
        if extension.private || trusted.contains(&extension.publisher.to_lowercase()) {
            continue;
        }
        match publishers.iter_mut().find(|publisher| publisher.display_name == extension.publisher_display_name) {
            Some(publisher) => *publisher = Publisher::of(&extension),
            None => publishers.push(Publisher::of(&extension)),
        }
    }
    publishers
}

fn title(publishers: &[Publisher]) -> String {
    match publishers {
        [one] => format!("Do you trust the publisher \"{}\"?", one.display_name),
        [first, second] => format!("Do you trust publishers \"{}\" and \"{}\"?", first.display_name, second.display_name),
        [first, rest @ ..] => format!("Do you trust the publisher \"{}\" and {} others?", first.display_name, rest.len()),
        [] => unreachable!("the dialog names at least the extension's publisher"),
    }
}

/// The dialog's markdown, one paragraph per `appendText('\n')`.
fn details(extension: &GalleryExtension, others: &[Publisher], publishers: &[Publisher]) -> Vec<String> {
    let mut first = format!("The extension {} is published by {}.", extension_link(extension), publisher_link(&Publisher::of(extension)));
    if others.is_empty() {
        first.push_str(" This is the first extension you're installing from this publisher.");
    } else {
        let kind = if extension.extension_pack.is_empty() { "dependencies" } else { "extensionPack" };
        let command = format!("command:extension.open?{}", urlencoding(&json!([extension.id, kind]).to_string()));
        let links: Vec<String> = others.iter().map(publisher_link).collect();
        let named = match links.as_slice() {
            [one] => one.clone(),
            [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
            [] => unreachable!(),
        };
        first.push_str(&format!("&nbsp;Installing this extension will also install [extensions]({command}) published by {named}.&nbsp;This is the first time you're installing extensions from these publishers."));
    }
    let mut paragraphs = vec![first];
    let verified: Vec<&Publisher> = publishers.iter().filter(|publisher| publisher.domain.as_ref().is_some_and(|(_, verified)| *verified)).collect();
    let unverified: Vec<&Publisher> = publishers.iter().filter(|publisher| !publisher.domain.as_ref().is_some_and(|(_, verified)| *verified)).collect();
    if !verified.is_empty() || unverified.len() == 1 {
        for publisher in &verified {
            let (link, _) = publisher.domain.as_ref().expect("a verified publisher has a domain");
            let authority = link.split("://").nth(1).unwrap_or(link).split('/').next().unwrap_or_default();
            let domain = format!("$(link-external) {}", escape_label(authority));
            let domain = gallery_href(link).map_or(domain.clone(), |link| format!("[{domain}]({link})"));
            paragraphs.push(format!("$(verified-filled)&nbsp;{} has verified ownership of {domain}.", publisher_link(publisher)));
        }
        match unverified.as_slice() {
            [] => {}
            [one] => paragraphs.push(format!("$(unverified)&nbsp;{} is [**not** verified]({UNVERIFIED_LINK}).", publisher_link(one))),
            [rest @ .., last] => paragraphs.push(format!("$(unverified)&nbsp;{} and {} are [**not** verified]({UNVERIFIED_LINK}).", rest.iter().map(|publisher| publisher_link(publisher)).collect::<Vec<_>>().join(", "), publisher_link(last))),
        }
    } else {
        paragraphs.push(format!("$(unverified)&nbsp;All publishers are [**not** verified]({UNVERIFIED_LINK})."));
    }
    let publisher = if publishers.len() > 1 { "publishers" } else { "publisher" };
    paragraphs.push(format!("{} has no control over the behavior of third-party extensions, including how they manage your personal data. Proceed only if you trust the {publisher}.", super::menubar::NAME_LONG));
    paragraphs
}

/// `getPublisherLink`: a link when the gallery has a publisher page.
fn publisher_link(publisher: &Publisher) -> String {
    let label = escape_label(&publisher.display_name);
    match publisher.link.as_deref().and_then(gallery_href) {
        Some(link) => format!("[{label}]({link})"),
        None => label,
    }
}

fn extension_link(extension: &GalleryExtension) -> String {
    let name = if extension.display_name.is_empty() { &extension.name } else { &extension.display_name };
    match gallery_href(&extension.detail_link) {
        Some(link) => format!("[{}]({link})", escape_label(name)),
        None => escape_label(name),
    }
}

/// A gallery link as a markdown href: only http(s), with the parentheses and spaces that would end it encoded, so
/// gallery data cannot add a `command:` link to the dialog.
fn gallery_href(link: &str) -> Option<String> {
    if !(link.starts_with("https://") || link.starts_with("http://")) {
        return None;
    }
    let mut href = String::new();
    for char in link.chars() {
        match char {
            '(' | ')' | '[' | ']' => href.push_str(&format!("%{:02X}", char as u32)),
            char if char.is_whitespace() => href.push_str(&urlencoding(&char.to_string())),
            char => href.push(char),
        }
    }
    Some(href)
}

/// `escapeLabel`: whitespace runs to one space, then markdown syntax, `$(`, `<`, `:`, `.` and `@` escaped.
fn escape_label(label: &str) -> String {
    let label = label.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut escaped = String::new();
    let mut chars = label.chars().peekable();
    while let Some(char) = chars.next() {
        if "\\`*_{}[]()#+-!~<:.@".contains(char) || (char == '$' && chars.peek() == Some(&'(')) {
            escaped.push('\\');
        }
        escaped.push(char);
    }
    escaped
}

fn urlencoding(text: &str) -> String {
    let mut encoded = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')' => encoded.push(byte as char),
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notifications::parse_linked_text;

    fn extension(publisher: &str, display_name: &str, verified: Option<bool>) -> GalleryExtension {
        GalleryExtension {
            id: format!("{publisher}.pack"),
            name: "pack".into(),
            display_name: "Deutsch Language Pack".into(),
            publisher: publisher.into(),
            publisher_display_name: display_name.into(),
            detail_link: format!("https://open-vsx.org/extension/{publisher}/pack"),
            publisher_link: (publisher != "MS-CEINTL").then(|| format!("https://open-vsx.org/namespace/{publisher}")),
            publisher_domain: verified.map(|verified| ("https://example.com".to_owned(), verified)),
            ..GalleryExtension::default()
        }
    }

    #[test]
    fn one_unverified_publisher_names_the_extension_and_the_publisher() {
        let extension = extension("MS-CEINTL", "MS-CEINTL", None);
        let publishers = [Publisher::of(&extension)];
        assert_eq!(title(&publishers), "Do you trust the publisher \"MS-CEINTL\"?");
        assert_eq!(
            details(&extension, &[], &publishers),
            [
                "The extension [Deutsch Language Pack](https://open-vsx.org/extension/MS-CEINTL/pack) is published by MS\\-CEINTL. This is the first extension you're installing from this publisher.",
                "$(unverified)&nbsp;MS\\-CEINTL is [**not** verified](https://aka.ms/vscode-verify-publisher).",
                "VSRustium has no control over the behavior of third-party extensions, including how they manage your personal data. Proceed only if you trust the publisher.",
            ]
        );
    }

    #[test]
    fn gallery_links_cannot_add_command_links() {
        assert_eq!(gallery_href("command:workbench.action.quit"), None);
        let href = gallery_href("https://a.example/x)\t[y](command:workbench.action.quit").unwrap();
        assert_eq!(parse_linked_text(&format!("[label]({href})")).len(), 1);
        assert_eq!(href, "https://a.example/x%29%09%5By%5D%28command:workbench.action.quit");
    }

    #[test]
    fn a_pack_with_other_publishers_lists_them_and_their_verification() {
        let mut pack = extension("alpha", "Alpha", Some(true));
        pack.extension_pack = vec!["beta.one".into(), "gamma.two".into()];
        let others = [Publisher::of(&extension("beta", "Beta", Some(false))), Publisher::of(&extension("gamma", "Gamma Inc.", None))];
        let mut publishers = vec![Publisher::of(&pack)];
        publishers.extend(others.iter().cloned());
        assert_eq!(title(&publishers), "Do you trust the publisher \"Alpha\" and 2 others?");
        let details = details(&pack, &others, &publishers);
        assert!(details[0].contains("Installing this extension will also install [extensions](command:extension.open?%5B%22alpha.pack%22%2C%22extensionPack%22%5D) published by [Beta](https://open-vsx.org/namespace/beta) and [Gamma Inc\\.](https://open-vsx.org/namespace/gamma)."));
        assert_eq!(details[1], "$(verified-filled)&nbsp;[Alpha](https://open-vsx.org/namespace/alpha) has verified ownership of [$(link-external) example\\.com](https://example.com).");
        assert_eq!(details[2], "$(unverified)&nbsp;[Beta](https://open-vsx.org/namespace/beta) and [Gamma Inc\\.](https://open-vsx.org/namespace/gamma) are [**not** verified](https://aka.ms/vscode-verify-publisher).");
        assert!(details[3].ends_with("Proceed only if you trust the publishers."));
    }
}
