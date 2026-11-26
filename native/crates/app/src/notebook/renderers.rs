//! Notebook output renderers and static preloads (notebookOutputRenderer.ts, the renderer parts of
//! notebookServiceImpl.ts): `contributes.notebookRenderer` and `contributes.notebookPreload`, and which renderer and
//! mime type shows an output (`getOutputMimeTypeInfo`, cellOutputViewModel.ts `pickedMimeType`).

use std::path::PathBuf;

use exthost::scan::ScannedExtension;
use files::glob::{Options, match_pattern};

/// notebookCommon.ts `NOTEBOOK_DISPLAY_ORDER`.
pub const DISPLAY_ORDER: [&str; 9] = ["application/json", "application/javascript", "text/html", "image/svg+xml", "text/latex", "text/markdown", "image/png", "image/jpeg", "text/plain"];

#[derive(Clone, Debug)]
pub struct Renderer {
    pub id: String,
    pub entrypoint: PathBuf,
    /// The renderer an extension renderer extends (`entrypoint.extends`).
    pub extends: Option<String>,
    pub mime_types: Vec<String>,
    pub has_dependencies: bool,
    /// `requiresMessaging` other than `never`.
    pub messaging: bool,
    pub is_builtin: bool,
}

/// `contributes.notebookPreload`: a script the notebook webview of `view_type` loads first.
#[derive(Clone, Debug)]
pub struct StaticPreload {
    pub view_type: String,
    pub entrypoint: PathBuf,
    pub roots: Vec<PathBuf>,
}

pub fn renderers(extensions: &[ScannedExtension]) -> Vec<Renderer> {
    let mut renderers = Vec::new();
    for extension in extensions {
        for contribution in extension.description["contributes"]["notebookRenderer"].as_array().into_iter().flatten() {
            let (Some(id), Some(mime_types)) = (contribution["id"].as_str(), contribution["mimeTypes"].as_array()) else { continue };
            let entrypoint = &contribution["entrypoint"];
            let (path, extends) = match entrypoint.as_str() {
                Some(path) => (path, None),
                None => (entrypoint["path"].as_str().unwrap_or_default(), entrypoint["extends"].as_str().map(str::to_owned)),
            };
            renderers.push(Renderer {
                id: id.to_owned(),
                entrypoint: extension.location.join(path),
                extends,
                mime_types: mime_types.iter().filter_map(|mime| mime.as_str()).map(str::to_owned).collect(),
                has_dependencies: contribution["dependencies"].as_array().is_some_and(|list| !list.is_empty()) || contribution["optionalDependencies"].as_array().is_some_and(|list| !list.is_empty()),
                messaging: contribution["requiresMessaging"].as_str().is_some_and(|messaging| messaging != "never"),
                is_builtin: extension.description["isBuiltin"] == true,
            });
        }
    }
    renderers
}

pub fn static_preloads(extensions: &[ScannedExtension]) -> Vec<StaticPreload> {
    let mut preloads = Vec::new();
    for extension in extensions {
        for contribution in extension.description["contributes"]["notebookPreload"].as_array().into_iter().flatten() {
            let (Some(view_type), Some(entrypoint)) = (contribution["type"].as_str(), contribution["entrypoint"].as_str()) else { continue };
            let roots = contribution["localResourceRoots"].as_array().into_iter().flatten().filter_map(|root| root.as_str()).map(|root| extension.location.join(root)).collect();
            preloads.push(StaticPreload { view_type: view_type.to_owned(), entrypoint: extension.location.join(entrypoint), roots });
        }
    }
    preloads
}

impl Renderer {
    fn matches_mime(&self, mime: &str) -> bool {
        let options = Options { ignore_case: true, ..Options::default() };
        self.extends.is_none() && self.mime_types.iter().any(|pattern| pattern == mime || match_pattern(pattern, mime, options))
    }
}

/// The mime type and renderer that show an output with items of `mimes`: the mimes in `notebook.displayOrder` and
/// then the default order, each with its best renderer (`findBestRenderers` without a kernel: built-in renderers
/// after the others, a renderer with kernel dependencies first). None when no renderer takes any of them.
pub fn pick_output_renderer<'a>(display_order: &[String], mimes: &[&'a str], renderers: &'a [Renderer]) -> Option<(&'a str, &'a Renderer)> {
    for mime in sort_mimes(display_order, mimes) {
        let mut candidates: Vec<&Renderer> = renderers.iter().filter(|renderer| renderer.matches_mime(mime)).collect();
        // `ReuseOrder` (other renderers before built-in ones), then `NotebookRendererMatch` (kernel dependencies first).
        candidates.sort_by_key(|renderer| (renderer.is_builtin, !renderer.has_dependencies));
        if let Some(renderer) = candidates.first() {
            return Some((mime, renderer));
        }
    }
    None
}

/// `MimeTypeDisplayOrder.sort`: each pattern of the order takes the first remaining mime it matches; the rest
/// follow by their place in the default order.
fn sort_mimes<'a>(display_order: &[String], mimes: &[&'a str]) -> Vec<&'a str> {
    let options = Options { ignore_case: false, ..Options::default() };
    let mut remaining: Vec<&str> = mimes.to_vec();
    let mut sorted = Vec::new();
    for pattern in display_order.iter().map(String::as_str).chain(DISPLAY_ORDER) {
        if let Some(index) = remaining.iter().position(|mime| *mime == pattern || match_pattern(pattern, mime, options)) {
            sorted.push(remaining.remove(index));
        }
    }
    let rank = |mime: &&str| DISPLAY_ORDER.iter().position(|default| default == mime).map_or(-1, |index| index as i64);
    remaining.sort_by_key(rank);
    sorted.extend(remaining);
    sorted
}

#[cfg(test)]
mod tests {
    use super::*;

    fn renderer(id: &str, mimes: &[&str], builtin: bool) -> Renderer {
        Renderer { id: id.into(), entrypoint: PathBuf::new(), extends: None, mime_types: mimes.iter().map(|mime| mime.to_string()).collect(), has_dependencies: false, messaging: false, is_builtin: builtin }
    }

    #[test]
    fn picks_the_mime_first_in_display_order_with_a_renderer() {
        let renderers = [renderer("vscode.builtin-renderer", &["text/plain", "image/png", "application/vnd.code.notebook.stdout"], true), renderer("other", &["image/*"], false)];
        let pick = |order: &[String], mimes: &[&'static str]| pick_output_renderer(order, mimes, &renderers).map(|(mime, renderer)| (mime.to_owned(), renderer.id.clone()));
        assert_eq!(pick(&[], &["text/plain", "image/png"]), Some(("image/png".into(), "other".into())));
        assert_eq!(pick(&["text/plain".into()], &["image/png", "text/plain"]), Some(("text/plain".into(), "vscode.builtin-renderer".into())));
        assert_eq!(pick(&[], &["application/vnd.code.notebook.stdout"]), Some(("application/vnd.code.notebook.stdout".into(), "vscode.builtin-renderer".into())));
        assert_eq!(pick(&[], &["application/x-unknown"]), None);
    }
}
