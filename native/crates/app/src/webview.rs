//! platform/webview on macOS: a WKWebView added as a child of GPUI's NSView, loading upstream's host page
//! (src/vs/workbench/contrib/webview/browser/pre/index.html) the way webviewElement.ts loads it in an iframe.
//! Two seams differ from Electron: a WKURLSchemeHandler on `https` answers what the service worker answers there
//! (the host page files and `https://*.vscode-resource.vscode-cdn.net` resources, other https through NSURLSession),
//! and a WKScriptMessageHandler stands in for the window `message` event that hands the host the MessagePort.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;
use std::sync::OnceLock;

use block2::RcBlock;
use futures::channel::mpsc::UnboundedSender;
use gpui::{Bounds, Pixels, Window};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, Bool, Imp, NSObject, NSObjectProtocol, ProtocolObject, Sel};
use objc2::{AnyThread, ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{NSView, NSWindow};
use objc2_foundation::{NSData, NSDictionary, NSError, NSHTTPURLResponse, NSObjectNSKeyValueCoding, NSOperationQueue, NSPoint, NSRect, NSSize, NSString, NSURLResponse, NSURLSession, NSURLSessionConfiguration};
use objc2_web_kit::{WKScriptMessage, WKScriptMessageHandler, WKURLSchemeHandler, WKURLSchemeTask, WKUserContentController, WKUserScript, WKUserScriptInjectionTime, WKWebView, WKWebViewConfiguration};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::theme::{Theme, ThemeKind};

/// common/webview.ts `webviewRootResourceAuthority`.
const RESOURCE_AUTHORITY: &str = "vscode-resource.vscode-cdn.net";
/// The host page's site; index.html only checks that the hostname starts with the parent origin hash.
const HOST_SITE: &str = "vscode-webview.vscode-cdn.net";
/// `window.origin` of the Electron workbench, which index.html hashes into the host name.
const PARENT_ORIGIN: &str = "vscode-file://vscode-app";
const MESSAGE_HANDLER: &str = "vscodeWebviewHost";

/// What the host page sends the workbench: the `FromWebviewMessage` channels of webviewMessages.d.ts, and
/// `webview-ready` once the page handed over its port.
#[derive(Debug)]
pub struct WebviewEvent {
    pub id: String,
    pub channel: String,
    pub data: Value,
}

pub struct Webview {
    view: Retained<WKWebView>,
    roots: Rc<RefCell<Vec<PathBuf>>>,
    /// Messages for the host page before it posted `webview-ready`, sent in order once it has.
    pending: Option<Vec<String>>,
}

/// WebKit needs AppKit's main thread, which unit tests do not run on.
pub fn supported() -> bool {
    MainThreadMarker::new().is_some()
}

impl Webview {
    /// A hidden webview over `window` loading the host page; `origin` is the webview's `this.origin`, the salt of its host name.
    pub fn new(window: &Window, id: &str, origin: &str, extension_id: &str, pre_dir: PathBuf, roots: Vec<PathBuf>, events: UnboundedSender<WebviewEvent>) -> Self {
        let mtm = MainThreadMarker::new().expect("webviews live on the main thread");
        allow_https_scheme_handler();
        let roots = Rc::new(RefCell::new(roots));
        let configuration = unsafe { WKWebViewConfiguration::new(mtm) };
        let handler = SchemeHandler::new(mtm, SchemeState { pre_dir, roots: roots.clone(), tasks: RefCell::default(), session: OnceLock::new() });
        unsafe { configuration.setURLSchemeHandler_forURLScheme(Some(ProtocolObject::from_ref(&*handler)), &NSString::from_str("https")) };
        let content = unsafe { configuration.userContentController() };
        let bridge = MessageHandler::new(mtm, MessageState { id: id.to_string(), events });
        unsafe {
            content.addScriptMessageHandler_name(ProtocolObject::from_ref(&*bridge), &NSString::from_str(MESSAGE_HANDLER));
            let script = WKUserScript::initWithSource_injectionTime_forMainFrameOnly(WKUserScript::alloc(mtm), &NSString::from_str(BRIDGE_SCRIPT), WKUserScriptInjectionTime::AtDocumentStart, true);
            content.addUserScript(&script);
        }
        let view = unsafe { WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), NSRect::ZERO, &configuration) };
        let no = objc2_foundation::NSNumber::new_bool(false);
        let no: &objc2::runtime::AnyObject = &no;
        unsafe {
            // The page background shows until the host page paints, as the transparent iframe does upstream.
            view.setValue_forKey(Some(no), &NSString::from_str("drawsBackground"));
            view.setInspectable(true);
        }
        view.setHidden(true);
        gpui_view(window).addSubview(&view);

        let params = [
            ("id", id),
            ("parentId", "1"),
            ("origin", origin),
            ("swVersion", "1"),
            ("extensionId", extension_id),
            ("platform", "electron"),
            ("vscode-resource-base-authority", RESOURCE_AUTHORITY),
            ("parentOrigin", PARENT_ORIGIN),
            ("mountId", id),
            ("disableServiceWorker", "true"),
        ];
        let query: Vec<String> = params.iter().map(|(key, value)| format!("{key}={}", query_escape(value))).collect();
        let url = format!("https://{}.{HOST_SITE}/index.html?{}", parent_origin_hash(PARENT_ORIGIN, origin), query.join("&"));
        let url = objc2_foundation::NSURL::URLWithString(&NSString::from_str(&url)).expect("host page URL");
        let request = objc2_foundation::NSURLRequest::requestWithURL(&url);
        unsafe { view.loadRequest(&request) };
        // The scheme and message handlers are retained by the configuration the view copied.
        Webview { view, roots, pending: Some(Vec::new()) }
    }

    /// `webviewElement.ts` `_send`: posts `{ channel, args }` on the host page's port.
    pub fn send(&mut self, channel: &str, data: Value) {
        let script = format!("globalThis.__vscodeWebviewHost.post({}, {})", json!(channel), data);
        match &mut self.pending {
            Some(pending) => pending.push(script),
            None => self.evaluate(&script),
        }
    }

    /// The host page posted `webview-ready`: messages queued before go out now.
    pub fn did_become_ready(&mut self) {
        for script in self.pending.take().unwrap_or_default() {
            self.evaluate(&script);
        }
    }

    pub fn set_local_resource_roots(&self, roots: Vec<PathBuf>) {
        *self.roots.borrow_mut() = roots;
    }

    /// The native view, for `place` from a paint callback.
    pub fn view(&self) -> Retained<WKWebView> {
        self.view.clone()
    }

    pub fn hide(&self) {
        if self.view.isHidden() {
            return;
        }
        self.release_keyboard();
        self.view.setHidden(true);
    }

    /// `webviewElement.ts` `focus`: the view takes the keyboard and the host page focuses its content frame.
    pub fn focus(&mut self) {
        self.take_keyboard();
        self.send("focus", Value::Null);
    }

    /// Makes the view the first responder, as a click into it does.
    pub fn take_keyboard(&self) {
        if let Some(window) = self.view.window().filter(|window| !self.is_first_responder(window)) {
            window.makeFirstResponder(Some(&self.view));
        }
    }

    /// Gives the keyboard back to GPUI's view, the view's superview.
    pub fn release_keyboard(&self) {
        // SAFETY: the view is alive; its superview is GPUI's view or none.
        let (Some(window), Some(mut parent)) = (self.view.window(), unsafe { self.view.superview() }) else { return };
        if let Some(clip) = parent.downcast_ref::<ClipView>() {
            let Some(gpui) = (unsafe { clip.superview() }) else { return };
            parent = gpui;
        }
        if self.is_first_responder(&window) {
            window.makeFirstResponder(Some(&parent));
        }
    }

    fn is_first_responder(&self, window: &NSWindow) -> bool {
        // WKWebView hands first responder to an internal subview while editing; any descendant counts.
        let Some(responder) = window.firstResponder() else { return false };
        let Ok(view) = responder.downcast::<NSView>() else { return false };
        view.isDescendantOf(&self.view)
    }

    fn evaluate(&self, script: &str) {
        unsafe { self.view.evaluateJavaScript_completionHandler(&NSString::from_str(script), None) };
    }
}

impl Drop for Webview {
    fn drop(&mut self) {
        let content = unsafe { self.view.configuration().userContentController() };
        unsafe { content.removeScriptMessageHandlerForName(&NSString::from_str(MESSAGE_HANDLER)) };
        self.release_keyboard();
        self.view.removeFromSuperview();
    }
}

/// Shows `view` over `bounds`, in GPUI window coordinates.
pub fn place(view: &NSView, window: &Window, bounds: Bounds<Pixels>) {
    let parent = gpui_view(window);
    let (x, width, height) = (f64::from(f32::from(bounds.origin.x)), f64::from(f32::from(bounds.size.width)), f64::from(f32::from(bounds.size.height)));
    let top = f64::from(f32::from(bounds.origin.y));
    let y = if parent.isFlipped() { top } else { parent.bounds().size.height - top - height };
    let frame = NSRect::new(NSPoint::new(x, y), NSSize::new(width, height));
    if view.frame() != frame {
        view.setFrame(frame);
    }
    if view.isHidden() {
        view.setHidden(false);
    }
}

/// The notebook's back layer webview (backLayerWebView.ts) lies behind the cells upstream; here it floats over GPUI's
/// layer, so a clip view holds it: it shows the webview inside the notebook list only, and presses reach the
/// webview over its outputs and markdown previews only, GPUI's cells everywhere else.
pub struct ClipState {
    hit_rects: RefCell<Vec<NSRect>>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "CodeGpuiWebviewClip"]
    #[ivars = ClipState]
    pub struct ClipView;

    impl ClipView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(hitTest:))]
        // The view `hitTest:` returns is not owned (+0), so it passes through as a pointer.
        fn hit_test(&self, point: NSPoint) -> *mut NSView {
            // SAFETY: the clip view is GPUI's view's subview while it is hit tested.
            let local = self.convertPoint_fromView(point, unsafe { self.superview() }.as_deref());
            let inside = |rect: &NSRect| local.x >= rect.origin.x && local.y >= rect.origin.y && local.x < rect.origin.x + rect.size.width && local.y < rect.origin.y + rect.size.height;
            if !self.ivars().hit_rects.borrow().iter().any(inside) {
                return std::ptr::null_mut();
            }
            unsafe { msg_send![super(self), hitTest: point] }
        }
    }
);

impl ClipView {
    /// A hidden clip view over `window` holding `webview`.
    pub fn new(window: &Window, webview: &Webview) -> Retained<Self> {
        let mtm = MainThreadMarker::new().expect("webviews live on the main thread");
        let this = Self::alloc(mtm).set_ivars(ClipState { hit_rects: RefCell::default() });
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
        this.setClipsToBounds(true);
        this.setHidden(true);
        webview.view.removeFromSuperview();
        this.addSubview(&webview.view);
        webview.view.setHidden(false);
        gpui_view(window).addSubview(&this);
        this
    }

    /// Shows the clip over `bounds` in GPUI window coordinates, its webview at `webview` (relative to the clip's
    /// origin, as `hit_rects` are).
    pub fn place(&self, webview: &WKWebView, window: &Window, bounds: Bounds<Pixels>, webview_frame: Bounds<Pixels>, hit_rects: Vec<Bounds<Pixels>>) {
        place(self, window, bounds);
        let frame = ns_rect(webview_frame);
        if webview.frame() != frame {
            webview.setFrame(frame);
        }
        *self.ivars().hit_rects.borrow_mut() = hit_rects.into_iter().map(ns_rect).collect();
    }
}

fn ns_rect(bounds: Bounds<Pixels>) -> NSRect {
    let value = |pixels: Pixels| f64::from(f32::from(pixels));
    NSRect::new(NSPoint::new(value(bounds.origin.x), value(bounds.origin.y)), NSSize::new(value(bounds.size.width), value(bounds.size.height)))
}

fn gpui_view(window: &Window) -> Retained<NSView> {
    let handle = HasWindowHandle::window_handle(window).expect("an open window has a handle");
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else { unreachable!("GPUI on macOS has AppKit handles") };
    // SAFETY: GPUI's AppKit handle points at its NSView, alive while the window is.
    let view: &NSView = unsafe { handle.ns_view.cast().as_ref() };
    view.retain()
}

/// base/browser/iframe.ts `parentOriginHash`: SHA-256 of `{"parentOrigin":…,"salt":…}` in base 32, 52 digits.
pub fn parent_origin_hash(parent_origin: &str, salt: &str) -> String {
    let data = json!({ "parentOrigin": parent_origin, "salt": salt }).to_string();
    let digest = Sha256::digest(data.as_bytes());
    // 52 base-32 digits hold 260 bits; the 256-bit hash fills them from the low end, as BigInt.toString(32).padStart does.
    let mut digits = Vec::with_capacity(52);
    for index in 0..52 {
        let bit = index * 5;
        let mut value = 0u8;
        for offset in 0..5 {
            let position = bit + offset;
            if position < 256 && digest[31 - position / 8] >> (position % 8) & 1 == 1 {
                value |= 1 << offset;
            }
        }
        digits.push(char::from_digit(value.into(), 32).unwrap());
    }
    digits.iter().rev().collect()
}

fn query_escape(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'*' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// The local file an `asWebviewUri` URL names: `https://file+.vscode-resource.vscode-cdn.net/<path>`.
/// shortcut: only the `file` scheme loads; resources of extension file system providers answer 404, port with MainThreadFileSystem reads.
pub fn resource_path(host: &str, decoded_path: &str) -> Option<PathBuf> {
    let authority = host.strip_suffix(RESOURCE_AUTHORITY)?.strip_suffix('.')?;
    let (scheme, encoded_authority) = authority.split_once('+')?;
    if scheme != "file" || !encoded_authority.is_empty() {
        return None;
    }
    Some(PathBuf::from(decoded_path))
}

/// resourceLoading.ts containment: the path, without `..` escapes, under a root both as written and resolved.
pub fn is_contained(path: &Path, roots: &[PathBuf]) -> bool {
    if path.components().any(|component| component == Component::ParentDir) {
        return false;
    }
    let Some(root) = roots.iter().find(|root| path.starts_with(root)) else { return false };
    match (std::fs::canonicalize(path), std::fs::canonicalize(root)) {
        (Ok(real), Ok(real_root)) => real.starts_with(real_root),
        _ => true,
    }
}

/// platform/webview/common/mimeTypes.ts `getWebviewContentMimeType`.
pub fn mime_type(path: &Path) -> &'static str {
    let extension = path.extension().and_then(|extension| extension.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
    match extension.as_str() {
        "svg" => "image/svg+xml",
        "txt" => "text/plain",
        "css" => "text/css",
        "js" | "cjs" | "mjs" => "application/javascript",
        "json" => "application/json",
        "html" | "htm" => "text/html",
        "xhtml" => "application/xhtml+xml",
        "oft" | "otf" => "font/otf",
        "xml" => "application/xml",
        "wasm" => "application/wasm",
        // base/common/mime.ts `mapExtToMediaMimes`.
        "aac" => "audio/x-aac",
        "avi" => "video/x-msvideo",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "flv" => "video/x-flv",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "jpe" | "jpeg" | "jpg" => "image/jpg",
        "m1v" | "m2v" | "mpe" | "mpeg" | "mpg" => "video/mpeg",
        "m2a" | "m3a" | "mp2" | "mp2a" | "mp3" | "mpga" => "audio/mpeg",
        "mid" | "midi" => "audio/midi",
        "mk3d" | "mks" | "mkv" => "video/x-matroska",
        "mov" | "qt" => "video/quicktime",
        "movie" => "video/x-sgi-movie",
        "mp4" | "mp4v" | "mpg4" => "video/mp4",
        "mp4a" => "audio/mp4",
        "oga" | "ogg" | "spx" => "audio/ogg",
        "opus" => "audio/opus",
        "ogv" => "video/ogg",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "psd" => "image/vnd.adobe.photoshop",
        "tga" => "image/x-tga",
        "tif" | "tiff" => "image/tiff",
        "ttf" => "font/ttf",
        "wav" => "audio/x-wav",
        "webm" => "video/webm",
        "webp" => "image/webp",
        "wma" => "audio/x-ms-wma",
        "wmv" => "video/x-ms-wmv",
        "woff" => "application/font-woff",
        "woff2" => "font/woff2",
        _ => "application/unknown",
    }
}

/// A `Range: bytes=start-end` header over `len` bytes, as the inclusive byte range to send.
pub fn byte_range(header: &str, len: u64) -> Option<(u64, u64)> {
    let (start, end) = header.strip_prefix("bytes=")?.split_once('-')?;
    let start: u64 = start.trim().parse().ok()?;
    let end = match end.trim() {
        "" => len.checked_sub(1)?,
        end => end.parse::<u64>().ok()?.min(len.checked_sub(1)?),
    };
    (start <= end).then_some((start, end))
}

/// themeing.ts `getWebviewThemeData`: the `styles` message of the host page.
pub fn theme_data(theme: &Theme, theme_label: &str, editor: &Value, underline_links: bool) -> Value {
    let mut styles = serde_json::Map::new();
    let font = |key: &str| editor[key].as_str().filter(|value| !value.is_empty()).map(str::to_string);
    styles.insert("vscode-font-family".into(), json!("-apple-system, BlinkMacSystemFont, sans-serif"));
    styles.insert("vscode-font-weight".into(), json!("normal"));
    styles.insert("vscode-font-size".into(), json!("13px"));
    styles.insert("vscode-editor-font-family".into(), json!(font("fontFamily").unwrap_or("Menlo, Monaco, 'Courier New', monospace".into())));
    styles.insert("vscode-editor-font-weight".into(), json!(font("fontWeight").unwrap_or("normal".into())));
    let font_size = editor["fontSize"].as_f64().filter(|size| *size != 0.).unwrap_or(12.);
    styles.insert("vscode-editor-font-size".into(), json!(format!("{font_size}px")));
    styles.insert("text-link-decoration".into(), json!(if underline_links { "underline" } else { "none" }));
    for id in include_str!("webview/color_ids.txt").lines() {
        if let Some(color) = theme.color(id) {
            // colorUtils.ts `asCssVariableName`: every dot.
            styles.insert(format!("vscode-{}", id.replace('.', "-")), json!(css_color(color)));
        }
    }
    for (id, value) in SIZES {
        styles.insert(format!("vscode-{}", id.replace('.', "-")), json!(value));
    }
    styles.insert("vscode-editor-font-feature-settings".into(), json!(font_ligatures(&editor["fontLigatures"])));
    let active_theme = match theme.kind {
        ThemeKind::Light => "vscode-light",
        ThemeKind::Dark => "vscode-dark",
        ThemeKind::HighContrastDark => "vscode-high-contrast",
        ThemeKind::HighContrastLight => "vscode-high-contrast-light",
    };
    json!({ "styles": styles, "activeTheme": active_theme, "themeId": theme.settings_id, "themeLabel": theme_label, "reduceMotion": false, "screenReader": false })
}

/// editorOptions.ts `EditorFontLigatures.validate`.
fn font_ligatures(value: &Value) -> String {
    match value {
        Value::String(text) if text == "false" => "\"liga\" off, \"calt\" off".into(),
        Value::String(text) if text == "true" => "\"liga\" on, \"calt\" on".into(),
        Value::String(text) => text.clone(),
        Value::Bool(true) => "\"liga\" on, \"calt\" on".into(),
        _ => "\"liga\" off, \"calt\" off".into(),
    }
}

/// color.ts `Color.Format.CSS.format`: `#rrggbb` when opaque, else `rgba(…)` with the alpha to 2 decimals.
pub fn css_color(color: gpui::Rgba) -> String {
    let byte = |value: f32| (value * 255.).round() as u8;
    let (r, g, b) = (byte(color.r), byte(color.g), byte(color.b));
    if color.a >= 1. {
        return format!("#{r:02x}{g:02x}{b:02x}");
    }
    let alpha = (f64::from(color.a) * 100.).round() / 100.;
    format!("rgba({r}, {g}, {b}, {alpha})")
}

/// The size registry (platform/theme/common/sizes): every size is the same for all themes.
const SIZES: [(&str, &str); 49] = [
    ("bodyFontSize", "13px"),
    ("bodyFontSize.small", "12px"),
    ("bodyFontSize.xSmall", "11px"),
    ("fontSize.heading1", "26px"),
    ("fontSize.heading2", "18px"),
    ("fontSize.heading3", "13px"),
    ("fontSize.body1", "13px"),
    ("fontSize.body2", "11px"),
    ("fontSize.label1", "12px"),
    ("fontSize.label2", "11px"),
    ("fontSize.label3", "10px"),
    ("fontWeight.regular", "400"),
    ("fontWeight.semiBold", "600"),
    ("codiconFontSize", "16px"),
    ("codiconFontSize.compact", "12px"),
    ("cornerRadius.medium", "6px"),
    ("cornerRadius.xSmall", "2px"),
    ("cornerRadius.small", "4px"),
    ("cornerRadius.large", "8px"),
    ("cornerRadius.xLarge", "12px"),
    ("cornerRadius.circle", "9999px"),
    ("strokeThickness", "1px"),
    ("spacing.sizeNone", "0px"),
    ("spacing.size20", "2px"),
    ("spacing.size40", "4px"),
    ("spacing.size60", "6px"),
    ("spacing.size80", "8px"),
    ("spacing.size100", "10px"),
    ("spacing.size120", "12px"),
    ("spacing.size160", "16px"),
    ("spacing.size200", "20px"),
    ("spacing.size240", "24px"),
    ("spacing.size280", "28px"),
    ("spacing.size320", "32px"),
    ("spacing.size360", "36px"),
    ("spacing.size400", "40px"),
    ("panels.cornerRadius", "8px"),
    ("agentsWindow.panels.cornerRadius", "8px"),
    ("toolsPanel.cornerRadius", "8px"),
    ("primarySideBar.cornerRadius", "8px"),
    ("secondarySideBar.cornerRadius", "8px"),
    ("editorArea.cornerRadius", "8px"),
    ("activityBar.cornerRadius", "8px"),
    ("agentsWindow.chatPanels.cornerRadius", "8px"),
    ("agentsWindow.editorArea.cornerRadius", "8px"),
    ("agentsWindow.toolsPanel.cornerRadius", "8px"),
    ("agentsWindow.secondarySideBar.cornerRadius", "8px"),
    ("panels.spacing", "4px"),
    ("agentsWindow.panels.spacing", "4px"),
];

/// Runs before index.html's own script, which captures `window.parent.postMessage` (the top window here) to post
/// `webview-ready` with its port. The port's messages then reach the native handler as JSON.
/// Extension messages travel as extHostWebviewMessaging.ts serializes them: JSON text with
/// `$$vscode_array_buffer_reference$$` objects, the ArrayBuffers beside it as base64.
const BRIDGE_SCRIPT: &str = r#"(() => {
  const native = window.webkit.messageHandlers.vscodeWebviewHost;
  const viewTypes = [, Int8Array, Uint8Array, Uint8ClampedArray, Int16Array, Uint16Array, Int32Array, Uint32Array, Float32Array, Float64Array, BigInt64Array, BigUint64Array];
  const toBase64 = buffer => {
    const bytes = new Uint8Array(buffer);
    let text = '';
    for (let i = 0; i < bytes.length; i += 0x8000) {
      text += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
    }
    return btoa(text);
  };
  const fromBase64 = text => Uint8Array.from(atob(text), c => c.charCodeAt(0)).buffer;
  const serialize = message => {
    const buffers = [];
    const add = buffer => {
      const index = buffers.indexOf(buffer);
      return index >= 0 ? index : buffers.push(buffer) - 1;
    };
    const text = JSON.stringify(message, (_key, value) => {
      if (value instanceof ArrayBuffer) {
        return { $$vscode_array_buffer_reference$$: true, index: add(value) };
      }
      const type = ArrayBuffer.isView(value) ? viewTypes.indexOf(value.constructor) : -1;
      if (type > 0) {
        return { $$vscode_array_buffer_reference$$: true, index: add(value.buffer), view: { type, byteLength: value.byteLength, byteOffset: value.byteOffset } };
      }
      return value;
    });
    return { message: text, buffers: buffers.map(toBase64) };
  };
  const deserialize = ({ message, buffers }) => {
    const transfer = buffers.map(fromBase64);
    const revive = (_key, value) => {
      if (!value || typeof value !== 'object' || !value.$$vscode_array_buffer_reference$$) {
        return value;
      }
      const buffer = transfer[value.index];
      const type = value.view && viewTypes[value.view.type];
      return type ? new type(buffer, value.view.byteOffset, value.view.byteLength / type.BYTES_PER_ELEMENT) : buffer;
    };
    return { message: JSON.parse(message, transfer.length ? revive : undefined), transfer };
  };
  let port;
  window.postMessage = (message, _origin, transfer) => {
    if (port || !message || message.channel !== 'webview-ready' || !transfer || !transfer[0]) {
      return;
    }
    port = transfer[0];
    port.onmessage = e => {
      const { channel, data } = e.data;
      native.postMessage(JSON.stringify({ channel, data: channel === 'onmessage' ? serialize(data.message) : data }));
    };
    native.postMessage(JSON.stringify({ channel: 'webview-ready', data: message.data }));
  };
  globalThis.__vscodeWebviewHost = {
    post: (channel, args) => {
      if (channel !== 'message') {
        port.postMessage({ channel, args });
        return;
      }
      const data = deserialize(args);
      port.postMessage({ channel, args: data }, data.transfer);
    },
  };
})();"#;

struct MessageState {
    id: String,
    events: UnboundedSender<WebviewEvent>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "CodeGpuiWebviewMessageHandler"]
    #[ivars = MessageState]
    struct MessageHandler;

    unsafe impl NSObjectProtocol for MessageHandler {}

    unsafe impl WKScriptMessageHandler for MessageHandler {
        #[unsafe(method(userContentController:didReceiveScriptMessage:))]
        fn did_receive(&self, _controller: &WKUserContentController, message: &WKScriptMessage) {
            let body = unsafe { message.body() };
            let Ok(text) = body.downcast::<NSString>() else { return };
            let Ok(Value::Object(mut message)) = serde_json::from_str(&text.to_string()) else { return };
            let state = self.ivars();
            let channel = message.remove("channel").and_then(|channel| channel.as_str().map(str::to_string)).unwrap_or_default();
            let data = message.remove("data").unwrap_or(Value::Null);
            let _ = state.events.unbounded_send(WebviewEvent { id: state.id.clone(), channel, data });
        }
    }
);

impl MessageHandler {
    fn new(mtm: MainThreadMarker, state: MessageState) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(state);
        unsafe { msg_send![super(this), init] }
    }
}

struct SchemeState {
    /// src/vs/workbench/contrib/webview/browser/pre, the host page's files.
    pre_dir: PathBuf,
    roots: Rc<RefCell<Vec<PathBuf>>>,
    /// Network loads in flight; a task WebKit stopped is dropped here and must get no more callbacks.
    tasks: RefCell<HashMap<usize, Retained<ProtocolObject<dyn WKURLSchemeTask>>>>,
    session: OnceLock<Retained<NSURLSession>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "CodeGpuiWebviewSchemeHandler"]
    #[ivars = SchemeState]
    struct SchemeHandler;

    unsafe impl NSObjectProtocol for SchemeHandler {}

    unsafe impl WKURLSchemeHandler for SchemeHandler {
        #[unsafe(method(webView:startURLSchemeTask:))]
        fn start_task(&self, _web_view: &WKWebView, task: &ProtocolObject<dyn WKURLSchemeTask>) {
            self.start(task);
        }

        #[unsafe(method(webView:stopURLSchemeTask:))]
        fn stop_task(&self, _web_view: &WKWebView, task: &ProtocolObject<dyn WKURLSchemeTask>) {
            self.ivars().tasks.borrow_mut().remove(&(task as *const _ as usize));
        }
    }
);

impl SchemeHandler {
    fn new(mtm: MainThreadMarker, state: SchemeState) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(state);
        unsafe { msg_send![super(this), init] }
    }

    fn start(&self, task: &ProtocolObject<dyn WKURLSchemeTask>) {
        let request = unsafe { task.request() };
        let Some(url) = request.URL() else { return respond(task, 404, &[], Vec::new()) };
        let host = url.host().map(|host| host.to_string().to_ascii_lowercase()).unwrap_or_default();
        let path = url.path().map(|path| path.to_string()).unwrap_or_default();
        let state = self.ivars();
        if host.ends_with(HOST_SITE) {
            let name = path.trim_start_matches('/');
            let file = state.pre_dir.join(name);
            return match (name.contains(".."), std::fs::read(&file)) {
                (false, Ok(bytes)) => respond(task, 200, &[("Content-Type", mime_type(&file))], bytes),
                _ => respond(task, 404, &[], Vec::new()),
            };
        }
        if host.ends_with(RESOURCE_AUTHORITY) {
            let range = request.valueForHTTPHeaderField(&NSString::from_str("Range")).map(|range| range.to_string());
            return load_resource(task, &host, &path, &state.roots.borrow(), range.as_deref());
        }
        self.fetch(task);
    }

    /// Requests to the network, which the service worker leaves to the browser upstream.
    fn fetch(&self, task: &ProtocolObject<dyn WKURLSchemeTask>) {
        let state = self.ivars();
        let session = state.session.get_or_init(|| unsafe {
            NSURLSession::sessionWithConfiguration_delegate_delegateQueue(&NSURLSessionConfiguration::defaultSessionConfiguration(), None, Some(&NSOperationQueue::mainQueue()))
        });
        let key = task as *const _ as usize;
        state.tasks.borrow_mut().insert(key, task.retain());
        let this = self.retain();
        let completion = RcBlock::new(move |data: *mut NSData, response: *mut NSURLResponse, error: *mut NSError| {
            let Some(task) = this.ivars().tasks.borrow_mut().remove(&key) else { return };
            // SAFETY: NSURLSession passes valid objects or null, on the main queue it was given.
            unsafe {
                match (response.as_ref(), error.as_ref()) {
                    (Some(response), None) => {
                        task.didReceiveResponse(response);
                        if let Some(data) = data.as_ref() {
                            task.didReceiveData(data);
                        }
                        task.didFinish();
                    }
                    (_, Some(error)) => task.didFailWithError(error),
                    (None, None) => respond(&task, 502, &[], Vec::new()),
                }
            }
        });
        let request = unsafe { task.request() };
        let data_task = unsafe { session.dataTaskWithRequest_completionHandler(&request, &completion) };
        data_task.resume();
    }
}

fn load_resource(task: &ProtocolObject<dyn WKURLSchemeTask>, host: &str, path: &str, roots: &[PathBuf], range: Option<&str>) {
    let Some(file) = resource_path(host, path) else { return respond(task, 404, &[], Vec::new()) };
    if !is_contained(&file, roots) {
        return respond(task, 401, &[], Vec::new());
    }
    let Ok(bytes) = std::fs::read(&file) else { return respond(task, 404, &[], Vec::new()) };
    let mime = mime_type(&file);
    // service-worker.js answers with these headers; media elements ask for byte ranges.
    let headers = [("Content-Type", mime), ("Access-Control-Allow-Origin", "*"), ("Cache-Control", "no-cache")];
    let Some((start, end)) = range.and_then(|range| byte_range(range, bytes.len() as u64)) else {
        return respond(task, 200, &headers, bytes);
    };
    let content_range = format!("bytes {start}-{end}/{}", bytes.len());
    let mut headers = headers.to_vec();
    headers.push(("Content-Range", &content_range));
    headers.push(("Accept-Ranges", "bytes"));
    respond(task, 206, &headers, bytes[start as usize..=end as usize].to_vec());
}

fn respond(task: &ProtocolObject<dyn WKURLSchemeTask>, status: isize, headers: &[(&str, &str)], body: Vec<u8>) {
    let request = unsafe { task.request() };
    let url = request.URL().expect("a scheme task has a URL");
    let length = body.len().to_string();
    let keys: Vec<Retained<NSString>> = headers.iter().map(|(key, _)| NSString::from_str(key)).chain([NSString::from_str("Content-Length")]).collect();
    let values: Vec<Retained<NSString>> = headers.iter().map(|(_, value)| NSString::from_str(value)).chain([NSString::from_str(&length)]).collect();
    let fields = NSDictionary::from_slices(&keys.iter().map(|key| &**key).collect::<Vec<_>>(), &values.iter().map(|value| &**value).collect::<Vec<_>>());
    let response = NSHTTPURLResponse::initWithURL_statusCode_HTTPVersion_headerFields(NSHTTPURLResponse::alloc(), &url, status, Some(&NSString::from_str("HTTP/1.1")), Some(&fields)).expect("an HTTP response");
    unsafe {
        task.didReceiveResponse(&response);
        task.didReceiveData(&NSData::from_vec(body));
        task.didFinish();
    }
}

static ORIGINAL_HANDLES_URL_SCHEME: OnceLock<Imp> = OnceLock::new();

/// `-[WKWebViewConfiguration setURLSchemeHandler:forURLScheme:]` refuses schemes `+[WKWebView handlesURLScheme:]`
/// claims; answering NO for https lets the handler stand in for the service worker on `https://*.vscode-cdn.net`.
fn allow_https_scheme_handler() {
    ORIGINAL_HANDLES_URL_SCHEME.get_or_init(|| {
        let method = WKWebView::class().class_method(sel!(handlesURLScheme:)).expect("+[WKWebView handlesURLScheme:]");
        let replacement: extern "C-unwind" fn(&AnyClass, Sel, &NSString) -> Bool = handles_url_scheme;
        // SAFETY: the replacement has the method's signature, `+ (BOOL)handlesURLScheme:(NSString *)`.
        unsafe { method.set_implementation(std::mem::transmute::<extern "C-unwind" fn(&AnyClass, Sel, &NSString) -> Bool, Imp>(replacement)) }
    });
}

extern "C-unwind" fn handles_url_scheme(class: &AnyClass, selector: Sel, scheme: &NSString) -> Bool {
    if scheme.to_string() == "https" {
        return Bool::NO;
    }
    let original = *ORIGINAL_HANDLES_URL_SCHEME.get().expect("swizzled");
    // SAFETY: the original implementation of the same method.
    let original: extern "C-unwind" fn(&AnyClass, Sel, &NSString) -> Bool = unsafe { std::mem::transmute(original) };
    original(class, selector, scheme)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_the_parent_origin_as_index_html_checks_it() {
        // index.html's inlined `parentOriginHash` run under Bun with the same arguments.
        assert_eq!(parent_origin_hash(PARENT_ORIGIN, "abc"), "003bfr8fddskskjr7sckuom7b7mci25b6n20ugb6ls6e92cu23f0");
    }

    #[test]
    fn maps_resource_urls_to_files_inside_the_roots() {
        let host = "file+.vscode-resource.vscode-cdn.net";
        assert_eq!(resource_path(host, "/w/a b.png"), Some(PathBuf::from("/w/a b.png")));
        assert_eq!(resource_path("vscode-remote+wsl-002bx.vscode-resource.vscode-cdn.net", "/a"), None);
        assert_eq!(resource_path("example.com", "/a"), None);
        let roots = [PathBuf::from("/nonexistent-root/media")];
        assert!(is_contained(Path::new("/nonexistent-root/media/a.png"), &roots));
        assert!(!is_contained(Path::new("/nonexistent-root/mediaX/a.png"), &roots));
        assert!(!is_contained(Path::new("/nonexistent-root/media/../secret"), &roots));
    }

    #[test]
    fn answers_byte_ranges_and_mime_types() {
        assert_eq!(byte_range("bytes=0-", 10), Some((0, 9)));
        assert_eq!(byte_range("bytes=2-4", 10), Some((2, 4)));
        assert_eq!(byte_range("bytes=2-40", 10), Some((2, 9)));
        assert_eq!(byte_range("bytes=12-", 10), None);
        assert_eq!(mime_type(Path::new("a.JS")), "application/javascript");
        assert_eq!(mime_type(Path::new("a.png")), "image/png");
        assert_eq!(mime_type(Path::new("a")), "application/unknown");
    }

    #[test]
    fn formats_colors_as_css_does() {
        assert_eq!(css_color(gpui::Rgba { r: 1., g: 0., b: 0.5, a: 1. }), "#ff0080");
        assert_eq!(css_color(gpui::Rgba { r: 1., g: 1., b: 1., a: 0.333 }), "rgba(255, 255, 255, 0.33)");
        assert_eq!(font_ligatures(&json!(false)), "\"liga\" off, \"calt\" off");
    }
}
