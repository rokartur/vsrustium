//! workbench/contrib/imageCarousel: the Explorer's "Open in Images Preview", an editor that shows a folder's images
//! and videos one at a time with a thumbnail strip (imageCarouselEditor.ts), drawn as a page in a webview panel.
//! The chat parts (attachment images, markup) are AI and removed.

use std::path::{Path, PathBuf};

use gpui::{Context, Window};
use serde_json::{Value, json};

use super::Workbench;
use super::webview_panels::{WEBVIEW_PANEL_SCHEME, WebviewPanel};
use crate::commands::Outcome;
use crate::editor::TextDocument;
use crate::file_dialog::FileDialog;
use crate::notifications::{Notification, Severity};

/// `ImageCarouselEditor.ID`, the editor id the host sees for the tab.
pub(super) const IMAGE_CAROUSEL_EDITOR_ID: &str = "workbench.editor.imageCarousel";
const PAGE: &str = include_str!("image_carousel/carousel.html");
/// imageCarousel.contribution.ts `MEDIA_EXTENSION_REGEX`.
const MEDIA_EXTENSIONS: [&str; 12] = ["png", "jpg", "jpeg", "jpe", "gif", "webp", "svg", "bmp", "ico", "mp4", "webm", "mov"];

impl Workbench {
    pub(super) fn register_image_carousel_commands(&mut self) {
        self.commands.register("workbench.action.openImagesInCarousel", |workbench, args, window, cx| {
            let resource = args.first().and_then(|arg| arg["path"].as_str()).map(PathBuf::from);
            workbench.open_images_in_carousel(resource, window, cx);
            Outcome::done()
        });
    }

    /// `OpenImagesInCarouselFromExplorerAction.run`: one media file opens its folder's media at it; a selection
    /// opens its files and its folders' media; no selection opens the right-clicked folder or the first root.
    fn open_images_in_carousel(&mut self, resource: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let context = self.folder.as_ref().map(|tree| tree.context(true, false)).unwrap_or_default();
        let mut start = None;
        let images = match self.carousel_media(&context, resource, &mut start) {
            Ok(images) => images,
            Err(_) => {
                self.notify_error("Could not read folder contents.");
                cx.notify();
                return;
            }
        };
        if images.is_empty() {
            self.notify(Notification::new(Severity::Info, "No images found in this folder."));
            cx.notify();
            return;
        }
        let start_index = start.and_then(|start| images.iter().position(|path| *path == start)).unwrap_or(0);
        self.open_carousel_editor(images, start_index, window, cx);
    }

    /// The media files of the Explorer context, and in `start` the one to show first.
    fn carousel_media(&self, context: &[PathBuf], resource: Option<PathBuf>, start: &mut Option<PathBuf>) -> std::io::Result<Vec<PathBuf>> {
        if context.is_empty() {
            let folder = resource.or_else(|| self.workspace().and_then(|workspace| workspace.folders.first()).map(|folder| folder.path.clone()));
            return folder.map_or(Ok(Vec::new()), |folder| media_in_folder(&folder));
        }
        if let [file] = context
            && !file.is_dir()
            && is_media(file)
        {
            *start = Some(file.clone());
            return file.parent().map_or(Ok(Vec::new()), media_in_folder);
        }
        let mut images: Vec<PathBuf> = Vec::new();
        for item in context {
            if item.is_dir() {
                for path in media_in_folder(item)? {
                    if !images.contains(&path) {
                        images.push(path);
                    }
                }
            } else if is_media(item) && !images.contains(item) {
                start.get_or_insert_with(|| item.clone());
                images.push(item.clone());
            }
        }
        Ok(images)
    }

    /// `editorService.openEditor(new ImageCarouselEditorInput(collection, startIndex), { pinned: true })`.
    fn open_carousel_editor(&mut self, images: Vec<PathBuf>, start_index: usize, window: &mut Window, cx: &mut Context<Self>) {
        static OPENED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let handle = format!("image-carousel-{}", OPENED.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
        let placeholder = PathBuf::from(format!("webview-panel/webview-{IMAGE_CAROUSEL_EDITOR_ID}-{handle}"));
        let document = TextDocument::from_text(WEBVIEW_PANEL_SCHEME, &placeholder, "", "utf8".into(), None, &self.languages, &self.settings.lock().unwrap());
        let mut roots: Vec<Value> = Vec::new();
        for parent in images.iter().filter_map(|path| path.parent()) {
            let root = json!({ "scheme": "file", "path": parent });
            if !roots.contains(&root) {
                roots.push(root);
            }
        }
        let options = json!({ "enableScripts": true, "localResourceRoots": roots });
        let mut panel = WebviewPanel::new(handle.clone(), WEBVIEW_PANEL_SCHEME.into(), placeholder, IMAGE_CAROUSEL_EDITOR_ID, "Images Preview".into(), &Value::Null, options, false, None);
        panel.html = carousel_page(&self.app_root, &images, start_index);
        panel.carousel = Some(images);
        self.webview_panels.panels.push(panel);
        self.editors.open(document, false);
        self.editors_changed(cx);
        self.focus_webview_panel(&handle, window, cx);
    }

    /// The page's messages: Download saves the image as `downloadFile` does, through the save dialog.
    pub(super) fn handle_carousel_message(&mut self, handle: &str, message: &Value, cx: &mut Context<Self>) {
        if message["type"] != "download" {
            return;
        }
        let Some(source) = self.webview_panel_mut(handle).and_then(|panel| panel.carousel.as_ref()?.get(message["index"].as_u64()? as usize).cloned()) else { return };
        let name = source.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let default_path = self.default_file_path().join(&name);
        let dialog = FileDialog { title: Some("Download".into()), default_path: Some(default_path), ..Default::default() };
        let picked = self.show_save_dialog(dialog, cx);
        cx.spawn(async move |workbench, cx| {
            let Some(target) = picked.await.ok().flatten() else { return };
            if let Err(error) = std::fs::copy(&source, &target) {
                let _ = workbench.update(cx, |workbench, cx| {
                    workbench.notify_error(format!("Failed to download {name}: {error}"));
                    cx.notify();
                });
            }
        })
        .detach();
    }
}

/// `collectImageFilesFromFolder`: the folder's media files, by name (`localeCompare`).
fn media_in_folder(folder: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut images: Vec<PathBuf> = std::fs::read_dir(folder)?.flatten().map(|entry| entry.path()).filter(|path| path.is_file() && is_media(path)).collect();
    let name = |path: &PathBuf| path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    images.sort_by(|a, b| crate::command_palette::locale_compare(&name(a), &name(b)));
    Ok(images)
}

fn is_media(path: &Path) -> bool {
    path.extension().is_some_and(|extension| MEDIA_EXTENSIONS.contains(&extension.to_string_lossy().to_lowercase().as_str()))
}

fn is_video(path: &Path) -> bool {
    path.extension().is_some_and(|extension| matches!(extension.to_string_lossy().to_lowercase().as_str(), "mp4" | "webm" | "mov"))
}

fn carousel_page(app_root: &Path, images: &[PathBuf], start_index: usize) -> String {
    let entries: Vec<Value> = images
        .iter()
        .map(|path| {
            let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            json!({ "name": name, "src": format!("https://file+.vscode-resource.vscode-cdn.net{}", super::media_preview::encode_path(path)), "video": is_video(path) })
        })
        .collect();
    let mut icons = serde_json::Map::new();
    for name in ["chevron-left", "chevron-right", "play", "warning", "cloud-download"] {
        icons.insert(name.into(), json!(std::fs::read_to_string(crate::codicons::svg_path(app_root, name)).unwrap_or_default()));
    }
    // `<` escaped so a file named `</script>` stays inside the script's strings.
    let script_json = |value: Value| value.to_string().replace('<', "\\u003c");
    PAGE.replace("{{csp_source}}", "'self' https://*.vscode-cdn.net")
        .replace("{{nonce}}", &format!("{:032x}", rand_suffix()))
        .replace("{{images}}", &script_json(Value::Array(entries)))
        .replace("{{icons}}", &script_json(Value::Object(icons)))
        .replace("{{start_index}}", &start_index.to_string())
}

fn rand_suffix() -> usize {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("the clock is past 1970").subsec_nanos() as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_a_folders_media_by_name() {
        let dir = std::env::temp_dir().join(format!("code-gpui-carousel-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["b.PNG", "a.mp4", "c.txt", "d.svg"] {
            std::fs::write(dir.join(name), "").unwrap();
        }
        let names: Vec<String> = media_in_folder(&dir).unwrap().iter().map(|path| path.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["a.mp4", "b.PNG", "d.svg"]);
        let page = carousel_page(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").as_path(), &[dir.join("</script>.png")], 0);
        assert!(!page.contains("{{"));
        assert!(!page.contains("\"</script>"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
