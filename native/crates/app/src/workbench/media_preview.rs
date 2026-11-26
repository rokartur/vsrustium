//! workbench/contrib/mediaPreview: the built-in image, video and audio previews, which open image and media files
//! in place of the text editor. Each draws its page in a webview panel without an extension.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{Context, Window};
use serde_json::{Value, json};

use super::Workbench;
use super::custom_editors::{Contribution, CustomEditor, RANK_BUILTIN};
use super::webview_panels::{WEBVIEW_PANEL_SCHEME, WebviewPanel};
use crate::commands::Outcome;
use crate::editor::TextDocument;
use crate::main_thread_quick_open::Owner;
use crate::quick_input::{Entry, Event, Item, Kind, QuickInput};

/// mediaPreview.ts: the editor ids keep the view types of the custom editors the `media-preview` extension had.
pub const IMAGE_PREVIEW_EDITOR_ID: &str = "imagePreview.previewEditor";
const VIDEO_PREVIEW_EDITOR_ID: &str = "vscode.videoPreview";
const AUDIO_PREVIEW_EDITOR_ID: &str = "vscode.audioPreview";
const IMAGE_PREVIEW_FILE_PATTERN: &str = "*.{jpg,jpe,jpeg,png,bmp,gif,ico,webp,avif,svg}";
const VIDEO_PREVIEW_FILE_PATTERN: &str = "*.{mp4,webm}";
const AUDIO_PREVIEW_FILE_PATTERN: &str = "*.{mp3,wav,ogg,oga}";
const SELECT_IMAGE_ZOOM_LEVEL_COMMAND_ID: &str = "imagePreview.selectZoomLevel";
/// `webviewGenericCspSource`.
const CSP_SOURCE: &str = "'self' https://*.vscode-cdn.net";
const OPEN_IN_TEXT_EDITOR: &str = "Open in Text Editor";
const IMAGE_PAGE: &str = include_str!("media_preview/image.html");
const PLAYER_PAGE: &str = include_str!("media_preview/player.html");
/// mediaPlayerEditorInput.ts `maxMediaSourceSize`.
const MAX_MEDIA_SOURCE_SIZE: u64 = 200 * 1024 * 1024;
/// mediaPreviewActions.ts `SelectImageZoomLevelAction`, `fit` as None.
const ZOOM_LEVEL_PICKS: [Option<f64>; 7] = [Some(10.), Some(5.), Some(2.), Some(1.), Some(0.5), Some(0.2), None];

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum MediaKind {
    Image,
    Video,
    Audio,
}

/// What the status bar shows of a preview (mediaPreviewStatusBar.ts), and an image's view state.
pub(super) struct MediaPreview {
    kind: MediaKind,
    path: PathBuf,
    file_size: Option<u64>,
    /// `ImageViewerScale`: a number or `"fit"`, Null while no image shows.
    scale: Value,
    dimensions: Option<(u64, u64)>,
    view_state: Value,
}

/// The built-in previews as the editor resolver ranks them (`RegisteredEditorPriority.builtin`).
pub(super) fn media_preview_contributions() -> Vec<Contribution> {
    let contribution = |view_type: &str, display_name: &str, pattern: &str| Contribution {
        view_type: view_type.to_owned(),
        display_name: display_name.to_owned(),
        provider: "Built-in".to_owned(),
        patterns: vec![pattern.to_owned()],
        rank: RANK_BUILTIN,
    };
    vec![
        contribution(IMAGE_PREVIEW_EDITOR_ID, "Image Preview", IMAGE_PREVIEW_FILE_PATTERN),
        contribution(VIDEO_PREVIEW_EDITOR_ID, "Video Preview", VIDEO_PREVIEW_FILE_PATTERN),
        contribution(AUDIO_PREVIEW_EDITOR_ID, "Audio Preview", AUDIO_PREVIEW_FILE_PATTERN),
    ]
}

pub(super) fn media_kind(editor_id: &str) -> Option<MediaKind> {
    match editor_id {
        IMAGE_PREVIEW_EDITOR_ID => Some(MediaKind::Image),
        VIDEO_PREVIEW_EDITOR_ID => Some(MediaKind::Video),
        AUDIO_PREVIEW_EDITOR_ID => Some(MediaKind::Audio),
        _ => None,
    }
}

impl Workbench {
    pub(super) fn register_media_preview_commands(&mut self) {
        self.commands.register("imagePreview.zoomIn", |workbench, _, _, _| workbench.post_to_active_image(json!({ "type": "zoom-in" })));
        self.commands.register("imagePreview.zoomOut", |workbench, _, _, _| workbench.post_to_active_image(json!({ "type": "zoom-out" })));
        self.commands.register(SELECT_IMAGE_ZOOM_LEVEL_COMMAND_ID, |workbench, _, window, cx| {
            if workbench.active_media_preview().is_some_and(|media| media.kind == MediaKind::Image) {
                workbench.show_zoom_level_picker(window, cx);
            }
            Outcome::done()
        });
        // mediaPreviewActions.ts: reopening as text or as preview are offered for SVG files.
        self.commands.register("imagePreview.reopenAsText", |workbench, _, window, cx| workbench.reopen_active_editor_with(Some("default".into()), window, cx));
        self.commands.register("imagePreview.reopenAsPreview", |workbench, _, window, cx| {
            workbench.reopen_active_editor_with(Some(IMAGE_PREVIEW_EDITOR_ID.into()), window, cx)
        });
    }

    /// mediaPreview.contribution.ts `createEditorInput`: one preview per file and editor, opened in the active group.
    pub(super) fn open_media_preview(&mut self, kind: MediaKind, view_type: &str, resource: &Value, options: &Value, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let path = PathBuf::from(resource["path"].as_str().unwrap_or_default());
        if resource["scheme"] != "file" {
            // shortcut: previews read `file` resources only; other file systems wait for MainThreadFileSystem reads.
            return Outcome::error(format!("Unable to preview {}", path.display()));
        }
        let shows = |panel: &WebviewPanel| panel.view_type == view_type && panel.custom.as_ref().is_some_and(|custom| custom.resource == *resource);
        let handle = match self.webview_panels.panels.iter().find(|panel| shows(panel)) {
            Some(panel) => panel.handle.clone(),
            None => {
                let handle = format!("media-preview-{}", self.webview_panels.panels.len() + self.editors.editors.len() + nanos() as usize);
                let placeholder = PathBuf::from(format!("webview-panel/webview-{view_type}-{handle}"));
                let document = TextDocument::from_text(WEBVIEW_PANEL_SCHEME, &placeholder, "", "utf8".into(), None, &self.languages, &self.settings.lock().unwrap());
                let title = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
                let roots = json!({ "enableScripts": true, "localResourceRoots": [{ "scheme": "file", "path": path.parent().unwrap_or(Path::new("/")) }] });
                // mediaPlayerEditorInput.ts keeps the webview while hidden, so playback goes on.
                let retain = kind != MediaKind::Image;
                let custom = Some(CustomEditor { resource: resource.clone(), resolved: true, backup_id: None });
                let mut panel = WebviewPanel::new(handle.clone(), WEBVIEW_PANEL_SCHEME.into(), placeholder, view_type, title, &Value::Null, roots, retain, custom);
                let file_size = std::fs::metadata(&path).ok().map(|metadata| metadata.len());
                let media = MediaPreview { kind, path, file_size, scale: Value::Null, dimensions: None, view_state: Value::Null };
                panel.html = media.page(&self.setting("mediaPreview.video.autoPlay"), &self.setting("mediaPreview.video.loop"));
                panel.media = Some(media);
                self.webview_panels.panels.push(panel);
                self.editors.open(document, options["pinned"] != true);
                handle
            }
        };
        let index = self.webview_editor_index(&handle).expect("the preview's editor is open");
        self.editors.activate(index);
        if options["pinned"] == true {
            self.editors.pin(index);
        }
        self.editors_changed(cx);
        if options["preserveFocus"] != true {
            self.focus_webview_panel(&handle, window, cx);
            if let Some(webview) = self.webview_panel_mut(&handle).and_then(|panel| panel.webview.as_mut()) {
                webview.send("message", json!({ "message": { "type": "focus" }, "buffers": [] }));
            }
        }
        Outcome::done()
    }

    /// A message a preview's page posted: mediaPlayerEditorInput.ts `webview.onMessage`, and the image viewer's
    /// `onDidChangeScale` with the dimensions it decoded.
    pub(super) fn handle_media_preview_message(&mut self, handle: &str, message: &Value, window: &mut Window, cx: &mut Context<Self>) {
        let Some(panel) = self.webview_panel_mut(handle) else { return };
        let Some(media) = &mut panel.media else { return };
        match message["type"].as_str().unwrap_or_default() {
            "image-info" => {
                media.scale = message["scale"].clone();
                media.dimensions = message["width"].as_u64().zip(message["height"].as_u64());
                media.view_state = message["viewState"].clone();
                let page = media.page(&Value::Null, &Value::Null);
                panel.html = page;
                cx.notify();
            }
            // shortcut: no codec or WebM type answers, mediaCodecs.ts (reading the container's tracks) is not ported;
            // the page then warns of no codec and plays what WebKit plays.
            "get-codecs" => {}
            "get-webm-type" => {
                if let Some(webview) = &mut panel.webview {
                    webview.send("message", json!({ "message": { "type": "webm-type" }, "buffers": [] }));
                }
            }
            "reopen-as-text" => drop(self.reopen_active_editor_with(Some("default".into()), window, cx)),
            "open-externally" => {
                let path = media.path.clone();
                cx.open_with_system(&path);
            }
            _ => {}
        }
    }

    /// mediaPreviewStatusBar.ts: the zoom (Select Zoom Level), dimensions and file size entries of the active preview.
    pub(super) fn media_preview_status_entries(&self) -> Vec<(String, &'static str, &'static str)> {
        let Some(media) = self.active_media_preview() else { return Vec::new() };
        let mut entries = Vec::new();
        if !media.scale.is_null() {
            entries.push((zoom_label(media.scale.as_f64()), "Select Zoom Level", SELECT_IMAGE_ZOOM_LEVEL_COMMAND_ID));
        }
        if let Some((width, height)) = media.dimensions {
            entries.push((format!("{width}x{height}"), "", ""));
        }
        if let Some(size) = media.file_size {
            entries.push((format_size(size), "", ""));
        }
        entries
    }

    fn active_media_preview(&self) -> Option<&MediaPreview> {
        let editor = self.editors.active()?;
        self.webview_panels.panels.iter().find(|panel| editor.document().is(&panel.scheme, &panel.path))?.media.as_ref()
    }

    fn post_to_active_image(&mut self, message: Value) -> Outcome {
        let Some(editor) = self.editors.active() else { return Outcome::done() };
        let (scheme, path) = (editor.document().scheme.clone(), editor.document().path.clone());
        let panel = self.webview_panels.panels.iter_mut().find(|panel| panel.scheme == scheme && panel.path == path);
        if let Some(webview) = panel.filter(|panel| panel.media.as_ref().is_some_and(|media| media.kind == MediaKind::Image)).and_then(|panel| panel.webview.as_mut()) {
            webview.send("message", json!({ "message": message, "buffers": [] }));
        }
        Outcome::done()
    }

    fn show_zoom_level_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let entries = ZOOM_LEVEL_PICKS.iter().enumerate().map(|(handle, scale)| Entry::Item(Item { handle: handle as i64, label: zoom_label(*scale), pickable: true, ..Item::default() })).collect();
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some("Select zoom level".into());
        input.set_items(entries);
        let id = self.quick_inputs.add(input, Owner::ImageZoom);
        self.show_quick_input(id, window, cx);
        self.flush_quick_input_events(window, cx);
    }

    pub fn image_zoom_picker_event(&mut self, id: i64, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            Event::Accept(_) => {
                let Some(&handle) = self.quick_inputs.get(id).and_then(|input| input.selected_handles().first().copied()).as_ref() else { return };
                self.hide_quick_input_by_id(id, window, cx);
                let scale = ZOOM_LEVEL_PICKS[handle as usize].map_or(json!("fit"), |scale| json!(scale));
                self.post_to_active_image(json!({ "type": "set-scale", "scale": scale }));
            }
            Event::Hide => self.quick_inputs.remove(id),
            _ => {}
        }
    }
}

impl MediaPreview {
    /// The preview's page: imagePreviewEditor.ts and imageViewer.ts for images, mediaPlayerEditorInput.ts `_getHtml`
    /// for video and audio.
    fn page(&self, auto_play: &Value, looped: &Value) -> String {
        let name = self.path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        // `asWebviewUri`, with mediaPlayerEditorInput.ts' `version` query against the cache.
        let src = format!("https://file+.vscode-resource.vscode-cdn.net{}?version={}", encode_path(&self.path), nanos() / 1_000_000);
        let nonce = format!("{:032x}", nanos());
        let page = match self.kind {
            MediaKind::Image => {
                // The Git LFS pointer messages are for `git` resources only, which the previews do not read.
                IMAGE_PAGE
                    .replace("{{aria_label}}", &escape(&format!("Image Preview: {name}")))
                    .replace("{{src}}", &json!(src).to_string())
                    // The view state comes back from the page: `<` escaped so a `</script>` in it stays a string.
                    .replace("{{view_state}}", &self.view_state.to_string().replace('<', "\\u003c"))
                    .replace("{{load_error}}", &json!("An error occurred while loading the image.").to_string())
            }
            MediaKind::Audio | MediaKind::Video => {
                let audio = self.kind == MediaKind::Audio;
                let media = match audio {
                    true => format!("<audio src=\"{}\" controls></audio>", escape(&src)),
                    false => {
                        let auto_play = if *auto_play == true { " autoplay muted" } else { "" };
                        let looped = if *looped == true { " loop" } else { "" };
                        format!("<video src=\"{}\" controls playsinline{auto_play}{looped}></video>", escape(&src))
                    }
                };
                let error = match audio {
                    true => "An error occurred while loading the audio file.",
                    false => "An error occurred while loading the video file.",
                };
                PLAYER_PAGE
                    .replace("{{media}}", &media)
                    .replace("{{error_message}}", &escape(error))
                    .replace("{{max_media_source_size}}", &MAX_MEDIA_SOURCE_SIZE.to_string())
            }
        };
        page.replace("{{csp_source}}", CSP_SOURCE).replace("{{nonce}}", &nonce).replace("{{open_in_text_editor}}", &escape(OPEN_IN_TEXT_EDITOR))
    }
}

/// mediaPreviewStatusBar.ts `getImageZoomLabel`, None for `fit`.
fn zoom_label(scale: Option<f64>) -> String {
    match scale {
        Some(scale) => format!("{}%", (scale * 100.).round()),
        None => "Whole Image".into(),
    }
}

/// files.ts `ByteSize.formatSize`.
fn format_size(size: u64) -> String {
    const KB: f64 = 1024.;
    let size = size as f64;
    match size {
        size if size < KB => format!("{size}B"),
        size if size < KB * KB => format!("{:.2}KB", size / KB),
        size if size < KB * KB * KB => format!("{:.2}MB", size / KB / KB),
        size if size < KB * KB * KB * KB => format!("{:.2}GB", size / KB / KB / KB),
        size => format!("{:.2}TB", size / KB / KB / KB / KB),
    }
}

/// strings.ts `escape`, then htmlContent.ts `escapeDoubleQuotes`.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// `URI.toString` path encoding of a file path.
fn encode_path(path: &Path) -> String {
    let mut encoded = String::new();
    for byte in path.to_string_lossy().bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => encoded.push(byte as char),
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

fn nanos() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).expect("the clock is past 1970").as_nanos()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_labels_as_the_status_bar_does() {
        assert_eq!(zoom_label(None), "Whole Image");
        assert_eq!(zoom_label(Some(0.5)), "50%");
        assert_eq!(zoom_label(Some(1.0 / 3.0)), "33%");
        assert_eq!(format_size(1000), "1000B");
        assert_eq!(format_size(1536), "1.50KB");
        assert_eq!(format_size(5 * 1024 * 1024), "5.00MB");
        assert_eq!(encode_path(Path::new("/a b/c#.png")), "/a%20b/c%23.png");
    }

    #[test]
    fn fills_every_placeholder_of_the_pages() {
        for kind in [MediaKind::Image, MediaKind::Video, MediaKind::Audio] {
            let media = MediaPreview { kind, path: PathBuf::from("/w/a.png"), file_size: None, scale: Value::Null, dimensions: None, view_state: json!({ "x": "</script>" }) };
            let page = media.page(&json!(true), &json!(false));
            assert!(!page.contains("{{"), "{page}");
            assert!(!page.contains("\"</script>"), "{page}");
        }
    }
}
