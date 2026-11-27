//! Protocol urls (`code-oss://`): app.ts `handleProtocolUrl` for the urls that open a file or folder, then the
//! window's url handlers: extensionUrlHandler.ts `ExtensionUrlHandler` and the handlers of mainThreadUrls.ts.

use std::path::PathBuf;
use std::time::{Duration, Instant};

pub use exthost::session::URL_PROTOCOL;
use gpui::{AsyncWindowContext, Context, PromptLevel, WeakEntity, Window};
use serde_json::{Value, json};

use super::Workbench;
use super::opener::UriOpener;
use super::workspace_actions::Openable;
use crate::commands::Outcome;
use crate::dialog;
use crate::quick_input::path_label;
use crate::uri::Uri;

/// extensionUrlHandler.ts `USER_TRUSTED_EXTENSIONS_STORAGE_KEY`, in the profile storage.
const CONFIRMED_EXTENSIONS_KEY: &str = "extensionUrlHandler.confirmedExtensions";
/// Buffered urls of an extension that never registers a handler are dropped after five minutes.
const BUFFER_LIFETIME: Duration = Duration::from_secs(5 * 60);

#[derive(Default)]
pub struct Urls {
    pub openers: Vec<UriOpener>,
    handlers: Vec<UriHandler>,
    /// Urls for extensions that were activated to register their handler.
    buffer: Vec<(String, Instant, Uri)>,
}

struct UriHandler {
    handle: u64,
    /// `ExtensionIdentifier.toKey`.
    extension: String,
    display_name: String,
}

impl Workbench {
    /// `$registerUriHandler`: urls buffered while the extension activated go to it now.
    pub(crate) fn register_uri_handler(&mut self, handle: u64, extension_id: &str, display_name: &str) {
        let extension = extension_id.to_lowercase();
        self.urls.handlers.retain(|handler| handler.extension != extension);
        self.urls.handlers.push(UriHandler { handle, extension: extension.clone(), display_name: display_name.to_owned() });
        let buffered = std::mem::take(&mut self.urls.buffer);
        for (owner, time, uri) in buffered {
            if owner != extension {
                self.urls.buffer.push((owner, time, uri));
            } else if time.elapsed() < BUFFER_LIFETIME {
                self.host.send("ExtHostUrls", "$handleExternalUri", vec![json!(handle), uri.to_json()]);
            }
        }
    }

    pub(crate) fn unregister_uri_handler(&mut self, handle: u64) {
        self.urls.handlers.retain(|handler| handler.handle != handle);
    }

    /// `ElectronURLListener`: a url the system hands the app, or `--open-url`.
    pub fn open_protocol_url(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Ok(uri) = Uri::parse(url) else { return };
        if uri.scheme != URL_PROTOCOL {
            return;
        }
        cx.spawn_in(window, async move |workbench, cx| {
            handle_url(&workbench, uri, false, cx).await;
        }).detach();
    }
}

/// electron-main app.ts `open-url`: the last active window handles a url the system hands the running app; with no
/// window, a new one opens with it.
pub(crate) fn open_system_url(url: String, cx: &mut gpui::App) {
    // The stack is ordered front to back but can be empty (no window ordered in yet): any window then.
    let last_active = cx.window_stack().unwrap_or_default().into_iter().chain(cx.windows()).find_map(|window| window.downcast::<Workbench>());
    match last_active {
        Some(handle) => drop(handle.update(cx, |workbench, window, cx| {
            window.activate_window();
            workbench.open_protocol_url(&url, window, cx);
        })),
        None => crate::open_window_or_log(crate::WindowRequest { open_url: Some(url), ..crate::WindowRequest::default() }, cx),
    }
}

/// urlService.ts `open`: answers whether a handler took the url.
pub(super) async fn handle_url(workbench: &WeakEntity<Workbench>, uri: Uri, trusted: bool, cx: &mut AsyncWindowContext) -> bool {
    let mut uri = uri;
    // Support 'workspace' URLs (https://github.com/microsoft/vscode/issues/124263)
    if uri.path == "workspace" {
        let path = Uri::parse(&uri.query).map(|inner| inner.path).unwrap_or_default();
        let Ok(file_url) = uri.with(|uri| {
            uri.authority = "file".into();
            uri.path = path;
            uri.query.clear();
        }) else {
            return false;
        };
        uri = file_url;
    }
    let query: Vec<&str> = uri.query.split('&').filter(|param| !param.is_empty()).collect();
    let new_window = query.contains(&"windowId=_blank");
    uri.query = query.into_iter().filter(|param| *param != "windowId=_blank").collect::<Vec<_>>().join("&");
    if uri.authority == "file" && !uri.path.is_empty() {
        return open_file_url(workbench, &uri.path, new_window, cx).await;
    }
    handle_extension_url(workbench, uri, trusted, cx).await
}

/// `getWindowOpenableFromProtocolUrl` and `shouldBlockOpenable` for a `file` url, opened in `gotoLineMode`.
async fn open_file_url(workbench: &WeakEntity<Workbench>, raw_path: &str, new_window: bool, cx: &mut AsyncWindowContext) -> bool {
    let (path, line) = parse_line_and_column(raw_path);
    let path = PathBuf::from(path);
    let openable = Openable::of(path.clone());
    // A protocol url opens a workspace file, or a file or folder (`fileUri`).
    let what = match path.extension().is_some_and(|extension| extension == "code-workspace") {
        true => "this workspace file",
        false => "this file or folder",
    };
    let label = path_label(&path, None);
    let ask = workbench.update(cx, |workbench, _| workbench.setting("security.promptForLocalFileProtocolHandling") != json!(false)).unwrap_or(true);
    if ask {
        let message = format!("An external application wants to open '{label}' in Code - OSS. Do you want to open {what}?");
        let detail = "If you did not initiate this request, it may represent an attempted attack on your system. Unless you took an explicit action to initiate this request, you should press 'No'";
        let Ok(answer) = workbench.update_in(cx, |_, window, cx| dialog::show_message_box(window, PromptLevel::Warning, &message, Some(detail), &["Yes", "No"], Some("Allow opening local paths without asking"), cx)) else {
            return true;
        };
        let Ok((0, checked)) = answer.await else { return true };
        if checked {
            let _ = workbench.update(cx, |workbench, cx| {
                workbench.update_setting("security.promptForLocalFileProtocolHandling", json!(false));
                cx.notify();
            });
        }
    }
    let _ = workbench.update_in(cx, |workbench, window, cx| {
        let is_file = matches!(openable, Openable::File(_));
        if workbench.open_window(vec![openable], new_window, window, cx) && is_file {
            if let Some((line, column)) = line {
                let selection = json!({ "startLineNumber": line, "startColumn": column, "endLineNumber": line, "endColumn": column });
                let uri = Uri { scheme: "file".into(), path: path.to_string_lossy().into_owned(), ..Uri::default() };
                drop(workbench.try_show_text_document(&uri.to_json(), &json!({ "selection": selection }), window, cx));
            }
        }
    });
    true
}

/// extpath.ts `parseLineAndColumnAware`: the column defaults to 1 when there is a line.
pub(super) fn parse_line_and_column(raw_path: &str) -> (String, Option<(f64, f64)>) {
    let mut path: Option<String> = None;
    let (mut line, mut column) = (None, None);
    for segment in raw_path.split(':') {
        // `Number('')` is 0, so an empty segment counts as a number too.
        let number = match segment.trim() {
            "" => Some(0.),
            trimmed => trimmed.parse::<f64>().ok().filter(|number| !number.is_nan()),
        };
        match number {
            None => path = Some(path.map_or(segment.to_owned(), |path| format!("{path}:{segment}"))),
            Some(number) if line.is_none() => line = Some(number),
            Some(number) if column.is_none() => column = Some(number),
            Some(_) => {}
        }
    }
    (path.unwrap_or_default(), line.map(|line| (line, column.unwrap_or(1.))))
}

/// `ExtensionUrlHandler.handleURL` followed by the extension's own handler.
async fn handle_extension_url(workbench: &WeakEntity<Workbench>, uri: Uri, trusted: bool, cx: &mut AsyncWindowContext) -> bool {
    if !is_extension_id(&uri.authority) {
        return false;
    }
    let extension = uri.authority.to_lowercase();
    let Ok((handler, display_name, confirmed)) = workbench.update(cx, |workbench, _| {
        let handler = workbench.urls.handlers.iter().find(|handler| handler.extension == extension);
        let installed = workbench.extensions.iter().find(|installed| installed.id.eq_ignore_ascii_case(&extension));
        let installed_name = installed.map(|installed| installed.description["displayName"].as_str().unwrap_or(&installed.id).to_owned());
        let display_name = handler.map(|handler| handler.display_name.clone()).or(installed_name);
        (handler.map(|handler| handler.handle), display_name, workbench.did_user_trust_extension(&extension))
    }) else {
        return false;
    };
    let Some(display_name) = display_name else {
        if install(workbench, &uri, cx).await {
            Box::pin(handle_extension_url(workbench, uri, true, cx)).await;
        }
        return true;
    };
    if !trusted && !confirmed {
        let text = uri.to_string(false);
        let chars: Vec<char> = text.chars().collect();
        let label = match chars.len() > 40 {
            true => format!("{}...{}", chars[..30].iter().collect::<String>(), chars[chars.len() - 5..].iter().collect::<String>()),
            false => text,
        };
        let message = format!("Allow '{display_name}' extension to open this URI?");
        let Ok(answer) = workbench.update_in(cx, |_, window, cx| dialog::show_message_box(window, PromptLevel::Info, &message, Some(&label), &["Open", "Cancel"], Some("Do not ask me again for this extension"), cx)) else {
            return true;
        };
        let Ok((0, checked)) = answer.await else { return true };
        if checked {
            let _ = workbench.update(cx, |workbench, _| workbench.confirm_extension(&extension));
        }
    }
    let _ = workbench.update(cx, |workbench, _| match handler {
        Some(handle) => workbench.host.send("ExtHostUrls", "$handleExternalUri", vec![json!(handle), uri.to_json()]),
        None => {
            workbench.urls.buffer.push((extension.clone(), Instant::now(), uri));
            // ActivationKind.Immediate
            drop(workbench.host.call("ExtHostExtensionService", "$activateByEvent", vec![json!(format!("onUri:{extension}")), json!(1)]));
        }
    });
    true
}

/// `handleUnhandledURL` up to the install: true once the extension runs, so the URI is handled again as trusted.
/// A cancelled install prompt ends here; a failed one shows its error.
async fn install(workbench: &WeakEntity<Workbench>, uri: &Uri, cx: &mut AsyncWindowContext) -> bool {
    let justification = json!({ "reason": format!("This extension wants to open a URI:\n{}", uri.to_string(false)), "action": "Open URI" });
    let args = vec![json!(uri.authority), json!({ "justification": justification, "enable": true, "installPreReleaseVersion": true })];
    let Ok(outcome) = workbench.update_in(cx, |workbench, window, cx| workbench.execute_command("workbench.extensions.installExtension", args, window, cx)) else {
        return false;
    };
    let installed = match outcome {
        Outcome::Ready(result) => result.is_ok(),
        Outcome::Pending(pending) => matches!(pending.await, Ok(Ok(_))),
    };
    installed && workbench.update(cx, |workbench, _| workbench.extensions.iter().any(|extension| extension.id.eq_ignore_ascii_case(&uri.authority))).unwrap_or(false)
}

fn is_extension_id(value: &str) -> bool {
    let part = |part: &str| part.chars().next().is_some_and(|first| first.is_ascii_alphanumeric()) && part.chars().all(|char| char.is_ascii_alphanumeric() || char == '-');
    value.split_once('.').is_some_and(|(publisher, name)| part(publisher) && part(name))
}

impl Workbench {
    /// `didUserTrustExtension`: confirmed once with the checkbox, or listed in `extensions.confirmedUriHandlerExtensionIds`.
    fn did_user_trust_extension(&self, extension: &str) -> bool {
        let confirmed = self.confirmed_extensions();
        let configured = self.setting("extensions.confirmedUriHandlerExtensionIds");
        confirmed.iter().any(|id| id == extension) || configured.as_array().is_some_and(|ids| ids.iter().any(|id| id == extension))
    }

    fn confirmed_extensions(&self) -> Vec<String> {
        let storage = self.profile_storage.lock().unwrap();
        storage.get(CONFIRMED_EXTENSIONS_KEY).and_then(|ids| serde_json::from_str(ids).ok()).unwrap_or_default()
    }

    fn confirm_extension(&mut self, extension: &str) {
        let mut confirmed = self.confirmed_extensions();
        confirmed.push(extension.to_owned());
        let value = Value::from(confirmed).to_string();
        self.profile_storage.lock().unwrap().set(CONFIRMED_EXTENSIONS_KEY, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_line_and_column_matches_extpath() {
        assert_eq!(parse_line_and_column("/a/b.ts:10:5"), ("/a/b.ts".into(), Some((10., 5.))));
        assert_eq!(parse_line_and_column("/a/b.ts:10"), ("/a/b.ts".into(), Some((10., 1.))));
        assert_eq!(parse_line_and_column("/a/b.ts"), ("/a/b.ts".into(), None));
        assert_eq!(parse_line_and_column("/a:b/c.ts:3"), ("/a:b/c.ts".into(), Some((3., 1.))));
    }

    #[gpui::test]
    fn a_url_the_system_hands_the_app_opens_in_the_last_active_window(cx: &mut gpui::TestAppContext) {
        let app_root = std::path::Path::new(env!("VSCODE_SRC")).to_path_buf();
        let theme = crate::theme::load_theme(&[exthost::scan::scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap()], "Dark Modern").unwrap();
        let dir = std::env::temp_dir().join(format!("vsrustium-open-url-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("User")).unwrap();
        let file = dir.join("notes.txt");
        std::fs::write(&file, "one\ntwo\n").unwrap();
        let settings = std::sync::Arc::new(std::sync::Mutex::new(config::service::Settings::load(config::registry::Registry::with_extensions(&[]), dir.join("User"), None)));
        let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), None, crate::languages::Languages::new(&[]), settings, crate::host_sync::HostSync::new(std::sync::mpsc::channel().0), cx));
        workbench.update(cx, |workbench, _| workbench.update_setting("security.promptForLocalFileProtocolHandling", json!(false)));
        cx.cx.update(|cx| open_system_url(format!("{URL_PROTOCOL}://file{}:2:1", file.display()), cx));
        cx.run_until_parked();
        let opened = workbench.read_with(cx, |workbench, _| workbench.editors.active().map(|editor| (editor.document().path.clone(), editor.selections[0].start().line)));
        assert_eq!(opened, Some((file, 1)));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn extension_ids_are_publisher_dot_name() {
        assert!(is_extension_id("vscode.git"));
        assert!(is_extension_id("ms-python.python"));
        assert!(!is_extension_id("file"));
        assert!(!is_extension_id("a.b.c"));
        assert!(!is_extension_id("-a.b"));
    }
}
