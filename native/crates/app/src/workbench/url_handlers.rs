//! mainThreadUrls.ts and extensionUrlHandler.ts: extensions' URI handlers and opening `<urlProtocol>:` URIs
//! (product.json `urlProtocol`) with them, as a webview's `vscode:` style links do.

use gpui::{Context, PromptLevel, Window};
use serde_json::{Value, json};

use super::Workbench;
use super::webview_panels::percent_decode;
use crate::commands::Outcome;
use crate::dialog;

/// extensionUrlHandler.ts `USER_TRUSTED_EXTENSIONS_STORAGE_KEY`, StorageScope.PROFILE.
const USER_TRUSTED_EXTENSIONS_STORAGE_KEY: &str = "extensionUrlHandler.confirmedExtensions";

#[derive(Default)]
pub struct UrlHandlers {
    /// `$registerUriHandler`: handle, extension id key (lower case), display name.
    handlers: Vec<(u64, String, String)>,
    /// extensionUrlHandler.ts `uriBuffer`: URIs for extensions not yet activated, by extension id key.
    buffered: Vec<(String, Value)>,
}

impl Workbench {
    pub(crate) fn handle_urls_request(&mut self, method: &str, args: &[Value]) -> Outcome {
        match method {
            "$registerUriHandler" => {
                let handle = args[0].as_u64().unwrap_or_default();
                let id = args[1]["value"].as_str().or(args[1].as_str()).unwrap_or_default().to_lowercase();
                let name = args[2].as_str().unwrap_or_default().to_owned();
                // registerExtensionHandler: URIs that waited for the activation go to the new handler.
                let (waiting, rest) = std::mem::take(&mut self.url_handlers.buffered).into_iter().partition(|(key, _)| *key == id);
                self.url_handlers.buffered = rest;
                for (_, uri) in waiting {
                    self.host.send("ExtHostUrls", "$handleExternalUri", vec![json!(handle), uri]);
                }
                self.url_handlers.handlers.push((handle, id, name));
            }
            "$unregisterUriHandler" => {
                let handle = args[0].as_u64().unwrap_or_default();
                self.url_handlers.handlers.retain(|(registered, ..)| *registered != handle);
            }
            "$createAppUri" => return Outcome::Ready(Ok(exthost::session::app_uri(&args[0]))),
            _ => {}
        }
        Outcome::done()
    }

    /// extensionUrlHandler.ts `handleURL` for an untrusted open: asks before an extension gets the URI.
    /// shortcut: a URI for an extension that is not installed is dropped; upstream offers to install it from the gallery (milestone 18).
    pub(crate) fn open_app_uri(&mut self, link: &str, window: &mut Window, cx: &mut Context<Self>) {
        let uri = parse_uri(link);
        let id = uri["authority"].as_str().unwrap_or_default().to_lowercase();
        let handler = self.url_handlers.handlers.iter().find(|(_, key, _)| *key == id).cloned();
        let display_name = match &handler {
            Some((_, _, name)) => name.clone(),
            None => match self.extensions.iter().find(|extension| extension.id.to_lowercase() == id) {
                Some(extension) => extension.description["displayName"].as_str().unwrap_or(&extension.id).to_owned(),
                None => return,
            },
        };
        let trusted = trusted_extensions(self.profile_storage.lock().unwrap().get(USER_TRUSTED_EXTENSIONS_STORAGE_KEY).unwrap_or("[]")).contains(&id);
        if trusted {
            self.handle_url_by_extension(id, uri);
            return;
        }
        let label = if link.chars().count() > 40 {
            let chars: Vec<char> = link.chars().collect();
            format!("{}...{}", chars[..30].iter().collect::<String>(), chars[chars.len() - 5..].iter().collect::<String>())
        } else {
            link.to_owned()
        };
        let message = format!("Allow '{display_name}' extension to open this URI?");
        let answer = dialog::show_message_box(window, PromptLevel::Info, &message, Some(&label), &["Open", "Cancel"], Some("Do not ask me again for this extension"), cx);
        cx.spawn(async move |workbench, cx| {
            let Ok((0, remember)) = answer.await else { return };
            workbench
                .update(cx, |workbench, _| {
                    if remember {
                        let mut storage = workbench.profile_storage.lock().unwrap();
                        let mut ids = trusted_extensions(storage.get(USER_TRUSTED_EXTENSIONS_STORAGE_KEY).unwrap_or("[]"));
                        ids.push(id.clone());
                        storage.set(USER_TRUSTED_EXTENSIONS_STORAGE_KEY, json!(ids).to_string());
                    }
                    workbench.handle_url_by_extension(id, uri);
                })
                .ok();
        })
        .detach();
    }

    /// The registered handler gets the URI; without one the extension activates on `onUri` and the URI waits.
    fn handle_url_by_extension(&mut self, id: String, uri: Value) {
        if let Some((handle, ..)) = self.url_handlers.handlers.iter().find(|(_, key, _)| *key == id) {
            self.host.send("ExtHostUrls", "$handleExternalUri", vec![json!(handle), uri]);
            return;
        }
        drop(self.host.activate_by_event(&format!("onUri:{id}")));
        self.url_handlers.buffered.push((id, uri));
    }
}

fn trusted_extensions(stored: &str) -> Vec<String> {
    serde_json::from_str(stored).unwrap_or_default()
}

/// `URI.parse` of `scheme://authority/path?query#fragment` as UriComponents, components percent-decoded.
pub fn parse_uri(text: &str) -> Value {
    let (scheme, rest) = text.split_once(':').unwrap_or(("", text));
    let (rest, fragment) = rest.split_once('#').unwrap_or((rest, ""));
    let (rest, query) = rest.split_once('?').unwrap_or((rest, ""));
    let (authority, path) = match rest.strip_prefix("//") {
        Some(rest) => rest.find('/').map_or((rest, ""), |slash| rest.split_at(slash)),
        None => ("", rest),
    };
    json!({
        "$mid": 1,
        "scheme": scheme,
        "authority": percent_decode(authority),
        "path": percent_decode(path),
        "query": percent_decode(query),
        "fragment": percent_decode(fragment),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_app_uris_as_uri_parse_does() {
        let uri = parse_uri("code-oss://pub.ext/a%20b?x=1&y=%7B%7D#frag");
        assert_eq!(uri, json!({ "$mid": 1, "scheme": "code-oss", "authority": "pub.ext", "path": "/a b", "query": "x=1&y={}", "fragment": "frag" }));
        assert_eq!(parse_uri("code-oss://pub.ext")["path"], "");
        assert_eq!(trusted_extensions("[\"a.b\"]"), vec!["a.b".to_owned()]);
        assert!(trusted_extensions("oops").is_empty());
    }
}
