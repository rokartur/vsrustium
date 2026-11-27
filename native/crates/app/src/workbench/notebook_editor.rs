//! The notebook editor of a `vscode-notebook-editor` tab (notebookEditorWidget.ts with notebookCellList.ts and the
//! cell parts): the notebook toolbar, then the cells one below the other in a list that scrolls as a whole. A code
//! cell is a code editor over its cell's text model with its status bar and execution order, its outputs below; a
//! markup cell shows its rendered preview until it is edited. Outputs and previews are drawn by the back layer
//! webview (backLayerWebView.ts with the upstream webviewPreloads.ts and the contributed renderers), which spans the
//! list's content and moves with its scroll; the focused cell's model is the one the group's code editor edits.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui::{Bounds, ClickEvent, Context, Div, FocusHandle, Modifiers, MouseButton, MouseDownEvent, Pixels, ScrollDelta, ScrollWheelEvent, Window, canvas, div, point, prelude::*, px, size};
use objc2::rc::Retained;
use serde_json::{Value, json};

use super::Workbench;
use super::hover::{HoverContent, HoverOptions};
use super::host_requests::uri_parts;
use super::list_scroll::{ListId, ListScroller};
use crate::editor::{LINE_HEIGHT, OpenEditor};
use crate::notebook::kernels::ExecutionState;
use crate::notebook::layout::{self, CellInput, CellLayout};
use crate::notebook::outline;
use crate::notebook::renderers::{self, Renderer};
use crate::notebook::{self, CODE, MARKUP, NOTEBOOK_EDITOR_SCHEME, NotebookModel};
use crate::webview::{ClipView, Webview};

/// The webview ids of notebook editors, followed by the tab's model id.
pub(super) const WEBVIEW_ID_PREFIX: &str = "notebook-";
/// The notebook toolbar (notebookEditorToolbar.ts) above the list.
const TOOLBAR_HEIGHT: f32 = 26.;
/// The notebook editor's `Memento` (`NotebookEditor.ID`) and its view state key (`NOTEBOOK_EDITOR_VIEW_STATE_PREFERENCE_KEY`).
const VIEW_STATE_STORAGE_KEY: &str = "memento/workbench.editor.notebook";
const VIEW_STATE_MEMENTO_KEY: &str = "NotebookEditorViewState";
const BACK_LAYER_HTML: &str = include_str!("../notebook/back_layer.html");
const WEBVIEW_PRELOADS: &str = include_str!("../notebook/webview_preloads.js");

/// webviewThemeMapping.ts `transformWebviewThemeVars`: the generic `--theme-*` variables renderers use.
const THEME_VARIABLES: [(&str, &str); 50] = [
    ("theme-font-family", "vscode-font-family"),
    ("theme-font-weight", "vscode-font-weight"),
    ("theme-font-size", "vscode-font-size"),
    ("theme-code-font-family", "vscode-editor-font-family"),
    ("theme-code-font-weight", "vscode-editor-font-weight"),
    ("theme-code-font-size", "vscode-editor-font-size"),
    ("theme-scrollbar-background", "vscode-scrollbarSlider-background"),
    ("theme-scrollbar-hover-background", "vscode-scrollbarSlider-hoverBackground"),
    ("theme-scrollbar-active-background", "vscode-scrollbarSlider-activeBackground"),
    ("theme-quote-background", "vscode-textBlockQuote-background"),
    ("theme-quote-border", "vscode-textBlockQuote-border"),
    ("theme-code-foreground", "vscode-textPreformat-foreground"),
    ("theme-code-background", "vscode-textPreformat-background"),
    ("theme-background", "vscode-editor-background"),
    ("theme-foreground", "vscode-editor-foreground"),
    ("theme-ui-foreground", "vscode-foreground"),
    ("theme-link", "vscode-textLink-foreground"),
    ("theme-link-active", "vscode-textLink-activeForeground"),
    ("theme-button-background", "vscode-button-background"),
    ("theme-button-hover-background", "vscode-button-hoverBackground"),
    ("theme-button-foreground", "vscode-button-foreground"),
    ("theme-button-secondary-background", "vscode-button-secondaryBackground"),
    ("theme-button-secondary-hover-background", "vscode-button-secondaryHoverBackground"),
    ("theme-button-secondary-foreground", "vscode-button-secondaryForeground"),
    ("theme-button-hover-foreground", "vscode-button-foreground"),
    ("theme-button-focus-foreground", "vscode-button-foreground"),
    ("theme-button-secondary-hover-foreground", "vscode-button-secondaryForeground"),
    ("theme-button-secondary-focus-foreground", "vscode-button-secondaryForeground"),
    ("theme-input-background", "vscode-input-background"),
    ("theme-input-foreground", "vscode-input-foreground"),
    ("theme-input-placeholder-foreground", "vscode-input-placeholderForeground"),
    ("theme-input-focus-border-color", "vscode-focusBorder"),
    ("theme-menu-background", "vscode-menu-background"),
    ("theme-menu-foreground", "vscode-menu-foreground"),
    ("theme-menu-hover-background", "vscode-menu-selectionBackground"),
    ("theme-menu-focus-background", "vscode-menu-selectionBackground"),
    ("theme-menu-hover-foreground", "vscode-menu-selectionForeground"),
    ("theme-menu-focus-foreground", "vscode-menu-selectionForeground"),
    ("theme-error-background", "vscode-inputValidation-errorBackground"),
    ("theme-error-foreground", "vscode-foreground"),
    ("theme-warning-background", "vscode-inputValidation-warningBackground"),
    ("theme-warning-foreground", "vscode-foreground"),
    ("theme-info-background", "vscode-inputValidation-infoBackground"),
    ("theme-info-foreground", "vscode-foreground"),
    ("theme-notebook-output-background", "vscode-notebook-outputContainerBackgroundColor"),
    ("theme-notebook-output-border", "vscode-notebook-outputContainerBorderColor"),
    ("theme-notebook-cell-selected-background", "vscode-notebook-selectedCellBackground"),
    ("theme-notebook-symbol-highlight-background", "vscode-notebook-symbolHighlightBackground"),
    ("theme-notebook-diff-removed-background", "vscode-diffEditor-removedTextBackground"),
    ("theme-notebook-diff-inserted-background", "vscode-diffEditor-insertedTextBackground"),
];

/// The notebook editor views by their tab's model id.
pub struct NotebookViews {
    views: HashMap<u32, NotebookView>,
    pub(super) status_bars: super::notebook_status_bar::CellStatusBars,
    /// The cell list of the active notebook editor (notebookCellList.ts), focused when no cell editor is.
    pub(super) list_focus: FocusHandle,
    /// notebookServiceImpl.ts `_displayOrder` after Change Presentation picks, with the `notebook.displayOrder`
    /// value it started from (a settings change starts over).
    mime_order: Option<(Value, Vec<String>)>,
    /// `NotebookOutputRendererInfoStore.setPreferred`: the renderer picked by notebook type and mime.
    preferred_renderers: HashMap<(String, String), String>,
    /// Notebook editors the editor part restored, opened once their notebooks resolve (notebooks.rs).
    pub(super) restores: Vec<super::notebooks::NotebookRestore>,
    /// The active notebook editor's tab and the cell whose editor was `activeCodeEditor` when it became active.
    status_editor: Option<(u32, Option<Value>)>,
    pub(super) find: find::NotebookFind,
    /// notebookEditor.ts `_editorMemento`: `INotebookEditorViewState` by notebook URI, in workspace storage.
    view_states: crate::editor::view_state::ViewStates,
    /// notebookServiceImpl.ts `setToCopy`: the cells Copy Cell or Cut Cell took, and whether they were copied.
    cell_clipboard: Option<(Vec<Value>, bool)>,
    /// `CellViewModel.lineNumbers` other than `inherit`, by cell URI: whether the cell shows line numbers.
    cell_line_numbers: HashMap<String, bool>,
}

impl NotebookViews {
    pub(super) fn new(cx: &mut Context<Workbench>) -> Self {
        NotebookViews {
            views: HashMap::new(),
            status_bars: Default::default(),
            list_focus: cx.focus_handle(),
            mime_order: None,
            preferred_renderers: HashMap::new(),
            restores: Vec::new(),
            status_editor: None,
            cell_clipboard: None,
            find: find::NotebookFind::new(cx),
            view_states: Default::default(),
            cell_line_numbers: HashMap::new(),
        }
    }
}

/// One notebook editor's list and its back layer webview.
#[derive(Default)]
struct NotebookView {
    scroll_top: f32,
    webview: Option<BackLayer>,
    /// The list's bounds as last laid out, in window coordinates.
    bounds: Rc<std::cell::Cell<Bounds<Pixels>>>,
    /// Markup cells in edit mode, by handle.
    editing: HashSet<u64>,
    hovered: Option<u64>,
    /// Outputs Show Empty Outputs made visible (cellOutputViewModel.ts `alwaysShow`), by id.
    shown_empty: HashSet<String>,
    /// The cell whose toolbar `...` dropdown is open (`cell-toolbar-dropdown-active`).
    toolbar_menu: Option<u64>,
    /// Header cells whose section is collapsed (foldingModel.ts), by handle.
    collapsed: HashSet<u64>,
    /// A cell editor had the keyboard at the last render (`editorFocused` of the view state).
    editor_focused: bool,
    /// The restored view state had the focus in the cell editor: the editor takes it when the notebook shows.
    restore_editor_focus: bool,
    /// A dragged cell's drop place (cellDnd.ts): the dragged cell, the cell under the pointer and whether below it.
    drop_target: Option<(u64, u64, bool)>,
}

/// backLayerWebView.ts: the webview, what it was sent and what it measured.
struct BackLayer {
    webview: Webview,
    clip: Retained<ClipView>,
    /// The preloads posted `initialized`.
    initialized: bool,
    /// Output heights by output id and preview heights by cell handle, from `dimension` messages.
    output_heights: HashMap<String, f32>,
    preview_heights: HashMap<u64, f32>,
    /// What the webview shows: outputs by id with their cell handle, mime and bytes, previews by handle with their
    /// text.
    outputs: HashMap<String, (u64, String, Vec<u8>)>,
    previews: HashMap<u64, String>,
    hidden_previews: HashSet<u64>,
    last_scroll: Value,
    /// `_getResourceRootsCache`, to which the bound kernel's extension folder is added.
    resource_roots: Vec<std::path::PathBuf>,
    /// backLayerWebView.ts `_currentKernel` (its id) and `_preloadsCache` (webview URIs).
    kernel: Option<String>,
    preloaded: HashSet<String>,
}

impl Drop for BackLayer {
    fn drop(&mut self) {
        self.clip.removeFromSuperview();
    }
}

/// How a cell range is brought into the list's viewport (`NotebookEditorRevealType`).
#[derive(Clone, Copy)]
pub(super) enum CellReveal {
    InView,
    Center,
    CenterIfOutside,
    Top,
}

/// A cell as the list draws it.
pub(super) struct CellRow {
    handle: u64,
    index: usize,
    kind: u64,
    editing: bool,
    layout: CellLayout,
    /// In a collapsed section: not drawn.
    hidden: bool,
    /// A header with a section (`CellFoldingState`): whether it is collapsed, and how many cells it folds.
    fold: Option<(bool, usize)>,
}

impl Workbench {
    /// The tab model id and notebook of the active notebook editor.
    pub(super) fn active_notebook_tab(&self) -> Option<(u32, String)> {
        let tab = self.editors.active_tab().filter(|tab| tab.document().scheme == NOTEBOOK_EDITOR_SCHEME)?;
        Some((tab.model_id, tab.document().path.to_string_lossy().into_owned()))
    }

    /// Runs each render: the active notebook editor has its webview, the others release theirs, and a covered webview
    /// hides (native views draw over GPUI's quick inputs and menus).
    pub(super) fn update_notebook_views(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tabs: Vec<u32> = self.host.notebooks.editors.iter().map(|editor| editor.tab_model_id).collect();
        self.notebook_views.views.retain(|tab, _| tabs.contains(tab));
        self.notebook_find_follow_edits(cx);
        let editor_focused = self.editor_focus.is_focused(window);
        if let Some(view) = self.active_notebook_tab().and_then(|(tab, _)| self.notebook_views.views.get_mut(&tab)) {
            view.editor_focused = editor_focused;
        }
        let active = self.active_notebook_tab().filter(|(_, notebook)| self.host.notebooks.model(notebook).is_some());
        let covered = self.quick_inputs.current().is_some() || self.keybindings_editor.open || self.context_menu.is_some();
        // The list records its bounds in the view, webview or not.
        if let Some((tab, _)) = &active {
            self.notebook_views.views.entry(*tab).or_default();
        }
        for (tab, view) in &mut self.notebook_views.views {
            if active.as_ref().is_none_or(|(active, _)| active != tab) {
                view.webview = None;
            } else if let Some(back_layer) = &view.webview
                && covered
            {
                back_layer.clip.setHidden(true);
            }
        }
        let Some((tab, notebook)) = active.filter(|_| crate::webview::supported()) else { return };
        if self.notebook_views.views.get(&tab).is_none_or(|view| view.webview.is_none()) {
            let back_layer = self.create_back_layer(tab, &notebook, window, cx);
            self.notebook_views.views.entry(tab).or_default().webview = Some(back_layer);
        }
        if !self.load_kernel_preloads(tab, &notebook) {
            let back_layer = self.create_back_layer(tab, &notebook, window, cx);
            self.notebook_views.views.entry(tab).or_default().webview = Some(back_layer);
            self.load_kernel_preloads(tab, &notebook);
        }
        self.sync_back_layer(tab, &notebook);
    }

    /// backLayerWebView.ts `createWebview`: the host page with the notebook content as its html.
    fn create_back_layer(&mut self, tab: u32, notebook: &str, window: &mut Window, cx: &mut Context<Self>) -> BackLayer {
        let events = self.webview_events(window, cx);
        let pre = self.app_root.join("src/vs/workbench/contrib/webview/browser/pre");
        let model = self.host.notebooks.model(notebook).expect("an active notebook editor shows an open notebook");
        let renderers = renderers::renderers(&self.extensions);
        let preloads: Vec<renderers::StaticPreload> = renderers::static_preloads(&self.extensions).into_iter().filter(|preload| preload.view_type == model.view_type).collect();
        let base = uri_parts(&model.uri).ok().filter(|(scheme, _)| *scheme == "file").and_then(|(_, path)| path.parent().map(std::path::Path::to_path_buf));
        // `_getResourceRootsCache`: the renderers' and preloads' folders, the workspace folders, the notebook's folder.
        let mut roots: Vec<std::path::PathBuf> = renderers.iter().filter_map(|renderer| renderer.entrypoint.parent().map(std::path::Path::to_path_buf)).collect();
        roots.extend(preloads.iter().flat_map(|preload| preload.entrypoint.parent().map(std::path::Path::to_path_buf).into_iter().chain(preload.roots.clone())));
        roots.extend(self.workspace().into_iter().flat_map(|workspace| workspace.folders.iter().map(|folder| folder.path.clone())));
        roots.extend(base.clone());
        let id = format!("{WEBVIEW_ID_PREFIX}{tab}");
        let mut webview = Webview::new(window, &id, &format!("notebook-{}", model.view_type), "", pre, roots.clone(), events);
        let styles = self.notebook_webview_styles();
        webview.send("styles", styles);
        let html = self.back_layer_html(&renderers, &preloads, base.as_deref());
        webview.send(
            "content",
            json!({
                "contents": html,
                "title": "Notebook webview content",
                "options": { "allowMultipleAPIAcquire": true, "allowScripts": true, "allowForms": true },
                "state": null,
                "cspSource": "'self' https://*.vscode-cdn.net",
                "confirmBeforeClose": "keyboardOnly",
            }),
        );
        let clip = ClipView::new(window, &webview);
        BackLayer { webview, clip, initialized: false, output_heights: HashMap::new(), preview_heights: HashMap::new(), outputs: HashMap::new(), previews: HashMap::new(), hidden_previews: HashSet::new(), last_scroll: Value::Null, resource_roots: roots, kernel: None, preloaded: HashSet::new() }
    }

    /// notebookEditorWidget.ts `_loadKernelPreloads` with backLayerWebView.ts `updateKernelPreloads`: the bound
    /// kernel's preloads load into the webview with its extension folder as a resource root; leaving a kernel that had
    /// preloads reloads the webview (the back layer is made again). Whether the back layer stays.
    fn load_kernel_preloads(&mut self, tab: u32, notebook: &str) -> bool {
        let Some(model) = self.host.notebooks.model(notebook) else { return true };
        let kernel = self.host.notebooks.kernels.matching(&model.view_type, notebook).1.cloned();
        let had_preloads = |id: &str| self.host.notebooks.kernels.kernels.iter().any(|kernel| kernel.id == id && !kernel.preloads.is_empty());
        let Some(view) = self.notebook_views.views.get_mut(&tab) else { return false };
        let Some(back_layer) = view.webview.as_mut() else { return false };
        if back_layer.kernel == kernel.as_ref().map(|kernel| kernel.id.clone()) {
            return true;
        }
        if back_layer.kernel.as_deref().is_some_and(had_preloads) {
            view.webview = None;
            return false;
        }
        back_layer.kernel = kernel.as_ref().map(|kernel| kernel.id.clone());
        let Some(kernel) = kernel else { return true };
        let mut roots = back_layer.resource_roots.clone();
        roots.extend(uri_parts(&kernel.extension_location).ok().map(|(_, path)| path));
        back_layer.webview.set_local_resource_roots(roots);
        let mut resources = Vec::new();
        for preload in &kernel.preloads {
            let Ok((scheme, path)) = uri_parts(preload) else { continue };
            let uri = match scheme {
                "http" | "https" => notebook::uri_string(preload),
                _ => webview_uri(&path),
            };
            if back_layer.preloaded.insert(uri.clone()) {
                resources.push(json!({ "uri": uri, "originalUri": notebook::uri_string(preload) }));
            }
        }
        if !resources.is_empty() {
            back_layer.post(json!({ "type": "preload", "resources": resources }));
        }
        true
    }

    /// The webview's theme with `transformWebviewThemeVars`.
    fn notebook_webview_styles(&self) -> Value {
        let mut data = self.webview_styles();
        let styles = data["styles"].as_object_mut().expect("theme_data has styles");
        for (name, source) in THEME_VARIABLES {
            if let Some(value) = styles.get(source).cloned() {
                styles.insert(name.into(), value);
            }
        }
        for (name, value) in [("theme-input-border-width", "1px"), ("theme-button-primary-hover-shadow", "none"), ("theme-button-secondary-hover-shadow", "none"), ("theme-input-border-color", "transparent")] {
            styles.insert(name.into(), json!(value));
        }
        data
    }

    /// backLayerWebView.ts `generateContent` with `preloadsScriptStr`.
    fn back_layer_html(&self, renderers: &[Renderer], preloads: &[renderers::StaticPreload], base: Option<&std::path::Path>) -> String {
        let options = self.notebook_webview_options();
        let context = json!({
            "style": { "outputNodePadding": layout::OUTPUT_PADDING, "outputNodeLeftPadding": layout::OUTPUT_PADDING, "tokenizationCss": "" },
            "options": { "dragAndDropEnabled": false },
            "renderOptions": options["renderOptions"],
            "rendererData": renderers.iter().map(|renderer| json!({
                "id": renderer.id,
                "entrypoint": { "extends": renderer.extends, "path": webview_uri(&renderer.entrypoint) },
                "mimeTypes": renderer.mime_types,
                "messaging": renderer.messaging,
                "isBuiltin": renderer.is_builtin,
            })).collect::<Vec<_>>(),
            "staticPreloadsData": preloads.iter().map(|preload| json!({ "entrypoint": webview_uri(&preload.entrypoint) })).collect::<Vec<_>>(),
            // An untrusted workspace's outputs render without scripts (webviewPreloads.ts `isWorkspaceTrusted`).
            "isWorkspaceTrusted": !self.workspace_trust.enabled || self.workspace_trust.is_trusted(),
            "nonce": exthost::main_thread_storage::generate_uuid(),
        });
        let encoded = query_component(&context.to_string());
        // The bundled webviewPreloads.ts defines `webviewPreloads`, which `preloadsScriptStr` calls with the context.
        let script = format!("{WEBVIEW_PRELOADS}\nconst __import = (x) => import(x);\nwebviewPreloads(JSON.parse(decodeURIComponent(\"{encoded}\")));\n//# sourceURL=notebookWebviewPreloads.js\n");
        let color = |key: &str| self.theme.color(key).map(crate::webview::css_color).unwrap_or_default();
        BACK_LAYER_HTML
            .replace("{{BASE_URL}}", &base.map(webview_uri).unwrap_or_default())
            .replace("{{NONCE}}", &context["nonce"].as_str().unwrap_or_default().to_owned())
            .replace("{{FIND_MATCH_HIGHLIGHT}}", &color("editor.findMatchHighlightBackground"))
            .replace("{{CURRENT_HIGHLIGHT}}", &color("editor.findMatchBackground"))
            .replace("{{PRELOAD_SCRIPT}}", &script)
    }

    /// notebookOptions.ts `computeWebviewOptions` and backLayerWebView.ts `_generateStyles`, `renderOptions`.
    fn notebook_webview_options(&self) -> Value {
        let font_size = self.setting("editor.fontSize").as_f64().unwrap_or(12.);
        let setting_number = |key: &str| self.setting(key).as_f64().filter(|value| *value > 0.);
        let output_font_size = setting_number("notebook.output.fontSize").unwrap_or(font_size);
        let output_line_height = setting_number("notebook.output.lineHeight").map_or(LINE_HEIGHT as f64, |height| if height < 8. { height * output_font_size } else { height }).round();
        let line_limit = self.setting("notebook.output.textLineLimit").as_f64().unwrap_or(30.);
        let editor_font = self.setting("editor.fontFamily");
        let output_font = self.setting("notebook.output.fontFamily").as_str().filter(|font| !font.is_empty()).map(str::to_owned).or(editor_font.as_str().map(str::to_owned)).unwrap_or_default();
        let markup_font_size = setting_number("notebook.markup.fontSize");
        let markdown_line_height = setting_number("notebook.markdown.lineHeight");
        let left_margin = layout::CODE_CELL_LEFT_MARGIN;
        let gutter = layout::CELL_RUN_GUTTER;
        json!({
            "styles": {
                "notebook-output-left-margin": format!("{}px", left_margin + gutter),
                "notebook-output-width": format!("calc(100% - {}px)", left_margin + layout::CELL_RIGHT_MARGIN + gutter),
                "notebook-output-node-padding": format!("{}px", layout::OUTPUT_PADDING),
                "notebook-run-gutter": format!("{gutter}px"),
                "notebook-preview-node-padding": format!("{}px", layout::MARKDOWN_PREVIEW_PADDING),
                "notebook-markdown-left-margin": format!("{}px", layout::MARKDOWN_CELL_GUTTER + layout::MARKDOWN_CELL_LEFT_MARGIN),
                "notebook-output-node-left-padding": format!("{}px", layout::OUTPUT_PADDING),
                "notebook-markdown-min-height": format!("{}px", layout::MARKDOWN_PREVIEW_PADDING * 2.),
                "notebook-markup-font-size": markup_font_size.map_or(format!("calc({font_size}px * 1.2)"), |size| format!("{size}px")),
                "notebook-markdown-line-height": markdown_line_height.map_or("normal".to_owned(), |height| format!("{height}px")),
                "notebook-cell-output-font-size": format!("{output_font_size}px"),
                "notebook-cell-output-line-height": format!("{output_line_height}px"),
                "notebook-cell-output-max-height": format!("{}px", output_line_height * line_limit + 2.),
                "notebook-cell-output-font-family": output_font,
                "notebook-cell-markup-empty-content": "Empty markdown cell, double-click or press enter to edit.",
                "notebook-cell-renderer-not-found-error": "No renderer found for '$0'",
                "notebook-cell-renderer-fallbacks-exhausted": "Could not render content for '$0'",
                "notebook-markup-font-family": self.setting("notebook.markup.fontFamily").as_str().unwrap_or_default(),
            },
            "renderOptions": {
                "lineLimit": line_limit,
                "outputScrolling": self.setting("notebook.output.scrolling") == true,
                "outputWordWrap": self.setting("notebook.output.wordWrap") == true,
                "linkifyFilePaths": self.setting("notebook.output.linkifyFilePaths") != false,
                "minimalError": self.setting("notebook.output.minimalErrorRendering") == true,
            },
        })
    }

    /// The webview's events for the notebook editor of tab `tab` (backLayerWebView.ts `_initialize` handlers).
    pub(super) fn handle_notebook_webview_event(&mut self, tab: u32, channel: &str, data: &Value, window: &mut Window, cx: &mut Context<Self>) {
        let Some(back_layer) = self.notebook_views.views.get_mut(&tab).and_then(|view| view.webview.as_mut()) else { return };
        match channel {
            "webview-ready" => back_layer.webview.did_become_ready(),
            "did-focus" => {
                back_layer.webview.take_keyboard();
                window.blur(cx);
            }
            "did-keydown" => {
                // Deferred: the keystroke interceptors update the workbench, which this handler is updating.
                if let Some(keystroke) = super::webview_panels::keystroke(data) {
                    window.defer(cx, move |window, cx| {
                        window.dispatch_keystroke(keystroke, cx);
                    });
                }
            }
            "fatal-error" => eprintln!("[notebook webview] {}", data["message"].as_str().unwrap_or_default()),
            "onmessage" => {
                let message: Value = data["message"].as_str().and_then(|text| serde_json::from_str(text).ok()).unwrap_or_default();
                if message["__vscode_notebook_message"] == true {
                    self.handle_notebook_message(tab, &message, window, cx);
                }
            }
            _ => {}
        }
    }

    fn handle_notebook_message(&mut self, tab: u32, message: &Value, window: &mut Window, cx: &mut Context<Self>) {
        let options = self.notebook_webview_options();
        let view = self.notebook_views.views.get_mut(&tab).expect("an event of a live webview");
        let back_layer = view.webview.as_mut().expect("an event of a live webview");
        match message["type"].as_str().unwrap_or_default() {
            "initialized" => {
                back_layer.initialized = true;
                back_layer.post(json!({ "type": "notebookStyles", "styles": options["styles"] }));
                back_layer.post(json!({ "type": "notebookOptions", "options": { "dragAndDropEnabled": false }, "renderOptions": options["renderOptions"] }));
            }
            "dimension" => {
                let mut acks = Vec::new();
                for update in message["updates"].as_array().into_iter().flatten() {
                    let (Some(id), Some(height)) = (update["id"].as_str(), update["height"].as_f64()) else { continue };
                    match update["isOutput"] == true {
                        true => {
                            back_layer.output_heights.insert(id.to_owned(), height as f32);
                            if let Some((handle, _, _)) = back_layer.outputs.get(id) {
                                acks.push(json!({ "cellId": cell_id(*handle), "outputId": id, "height": height }));
                            }
                        }
                        false => {
                            if let Some(handle) = parse_cell_id(id) {
                                back_layer.preview_heights.insert(handle, height as f32);
                            }
                        }
                    }
                }
                if !acks.is_empty() {
                    back_layer.post(json!({ "type": "ack-dimension", "updates": acks }));
                }
                cx.notify();
            }
            // backLayerWebView.ts `triggerScroll`: the list scrolls as for its own wheel. DOM deltas point the other way.
            "did-scroll-wheel" => {
                let payload = &message["payload"];
                let delta = point(-payload["deltaX"].as_f64().unwrap_or_default() as f32, -payload["deltaY"].as_f64().unwrap_or_default() as f32);
                let delta = match payload["deltaMode"].as_u64() {
                    Some(1) => ScrollDelta::Lines(delta),
                    _ => ScrollDelta::Pixels(point(px(delta.x), px(delta.y))),
                };
                let modifiers = Modifiers { shift: payload["shiftKey"] == true, ..Default::default() };
                let event = ScrollWheelEvent { position: window.mouse_position(), delta, modifiers, ..Default::default() };
                self.list_wheel(ListId::Notebook(tab), &event, window, cx);
            }
            "mouseLeaveMarkupCell" => {
                if message["cellId"].as_str().and_then(parse_cell_id).is_some_and(|handle| view.hovered == Some(handle)) {
                    view.hovered = None;
                    cx.notify();
                }
            }
            "clickMarkupCell" | "focus-editor" | "mouseEnterMarkupCell" => {
                let Some(handle) = message["cellId"].as_str().and_then(parse_cell_id) else { return };
                if message["type"] == "mouseEnterMarkupCell" {
                    view.hovered = Some(handle);
                    return cx.notify();
                }
                // The press focused the webview ("did-focus"); upstream the list then takes the focus from the iframe.
                if let Some(back_layer) = &view.webview {
                    back_layer.webview.release_keyboard();
                }
                self.focus_notebook_cell(tab, handle, false, window, cx);
            }
            "toggleMarkupPreview" => {
                let Some(handle) = message["cellId"].as_str().and_then(parse_cell_id) else { return };
                view.editing.insert(handle);
                self.focus_notebook_cell(tab, handle, true, window, cx);
            }
            "clicked-link" => {
                if let Some(href) = message["href"].as_str() {
                    self.open_webview_link(href, &json!(false), window, cx);
                }
            }
            "customRendererMessage" => {
                let Some(editor) = self.host.notebooks.editor(tab) else { return };
                let editor_id = editor.id.clone();
                let renderer_id = message["rendererId"].as_str().unwrap_or_default().to_owned();
                self.post_renderer_message(editor_id, renderer_id, message["message"].clone(), cx);
            }
            "didFind" => self.notebook_find_did_find(tab, message["matches"].as_array().map_or(&[], Vec::as_slice), cx),
            "didFindHighlightCurrent" => self.notebook_find_did_highlight(tab, message["offset"].as_f64().unwrap_or_default() as f32, cx),
            _ => {}
        }
    }

    /// editorStatus.ts `updateStatusBar` runs on `onDidActiveEditorChange` and `onDidChangeControl` (once the notebook
    /// editor's input is set), so it keeps the cell editor that was `activeCodeEditor` then; a cell focused later
    /// does not move it.
    pub(super) fn capture_notebook_status_editor(&mut self) {
        let tab = self.active_notebook_editor().map(|editor| editor.tab_model_id);
        if tab == self.notebook_views.status_editor.as_ref().map(|(tab, _)| *tab) {
            return;
        }
        let cell = self.active_notebook_editor().filter(|_| self.active_cell_has_editor()).and_then(|editor| Some(self.host.notebooks.model(&editor.notebook)?.cells.get(editor.focus)?.uri.clone()));
        self.notebook_views.status_editor = tab.map(|tab| (tab, cell));
    }

    pub(super) fn notebook_status_editor(&self) -> Option<&crate::editor::OpenEditor> {
        let (tab, cell) = self.notebook_views.status_editor.as_ref()?;
        if self.active_notebook_editor()?.tab_model_id != *tab {
            return None;
        }
        self.cell_model(cell.as_ref()?)
    }

    /// notebookRendererMessagingServiceImpl.ts `postMessage`: a renderer's messages to the host wait, in order, for
    /// its `onRenderer:<id>` activation.
    fn post_renderer_message(&mut self, editor_id: String, renderer_id: String, message: Value, cx: &mut Context<Self>) {
        if let Some(queue) = self.host.notebooks.renderer_messages.get_mut(&renderer_id) {
            queue.push((editor_id, message));
            return;
        }
        let activation = self.host.activate_by_event(&format!("onRenderer:{renderer_id}"));
        if activation.peek().is_some() {
            self.host.send("ExtHostNotebookRenderers", "$postRendererMessage", vec![json!(editor_id), json!(renderer_id), message]);
            return;
        }
        self.host.notebooks.renderer_messages.insert(renderer_id.clone(), vec![(editor_id, message)]);
        cx.spawn(async move |workbench, cx| {
            let _ = activation.await;
            workbench.update(cx, |workbench, _| {
                for (editor_id, message) in workbench.host.notebooks.renderer_messages.remove(&renderer_id).expect("queued above") {
                    workbench.host.send("ExtHostNotebookRenderers", "$postRendererMessage", vec![json!(editor_id), json!(renderer_id), message]);
                }
            })
        })
        .detach();
    }

    /// Focuses cell `handle` of the notebook editor of `tab`; with `editor` the code editor takes the keyboard.
    pub(super) fn focus_notebook_cell(&mut self, tab: u32, handle: u64, editor: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(notebook_editor) = self.host.notebooks.editors.iter_mut().find(|notebook_editor| notebook_editor.tab_model_id == tab) else { return };
        notebook_editor.input_focused = false;
        let Some(index) = self.host.notebooks.models.iter().find(|model| model.uri_string() == notebook_editor.notebook).and_then(|model| model.index_of_handle(handle)) else { return };
        notebook_editor.focus = index;
        notebook_editor.selections = vec![(index, index + 1)];
        self.editors_changed(cx);
        match editor {
            true => window.focus(&self.editor_focus, cx),
            false => window.focus(&self.notebook_views.list_focus, cx),
        }
        cx.notify();
    }

    /// listCommands.ts focus moves while the cell list has the focus; whether the list took the command.
    pub(super) fn notebook_list_command(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.notebook_views.list_focus.is_focused(window) {
            return false;
        }
        let delta = match id {
            "list.focusDown" | "list.focusAnyDown" => 1,
            "list.focusUp" | "list.focusAnyUp" => -1,
            "list.focusPageDown" => 10,
            "list.focusPageUp" => -10,
            "list.focusFirst" | "list.focusAnyFirst" => isize::MIN,
            "list.focusLast" | "list.focusAnyLast" => isize::MAX,
            _ => return true,
        };
        self.move_notebook_focus(delta, false, window, cx);
        true
    }

    /// layoutActions.ts and cellEditorOptions.ts: the Customize Notebook... submenu's commands.
    pub(super) fn register_notebook_layout_commands(&mut self) {
        let forward = [("notebook.toggleLineNumbersFromEditorTitle", "notebook.toggleLineNumbers"), ("breadcrumbs.toggleFromEditorTitle", "breadcrumbs.toggle")];
        for (id, target) in forward {
            self.commands.register(id, move |workbench, _, window, cx| workbench.execute_command(target, Vec::new(), window, cx));
        }
        for id in ["workbench.notebook.layout.configure", "workbench.notebook.layout.configure.editorTitle"] {
            self.commands.register(id, |workbench, _, window, cx| workbench.execute_command("workbench.action.openSettings", vec![json!({ "query": "@tag:notebookLayout" })], window, cx));
        }
        self.commands.register("notebook.toggleLineNumbers", |workbench, _, _, cx| {
            let on = workbench.setting("notebook.lineNumbers") == "on";
            workbench.update_setting("notebook.lineNumbers", json!(if on { "off" } else { "on" }));
            cx.notify();
            crate::commands::Outcome::done()
        });
    }

    /// The notebook's own navigation (arrow.ts `FocusNextCellAction`, `FocusPreviousCellAction`,
    /// `NOTEBOOK_FOCUS_TOP`/`BOTTOM`, editActions.ts `EditCellAction`, `QuitEditCellAction`).
    pub(super) fn register_notebook_list_commands(&mut self) {
        self.commands.register("notebook.focusTop", |workbench, _, window, cx| {
            workbench.move_notebook_focus(isize::MIN, false, window, cx);
            crate::commands::Outcome::done()
        });
        self.commands.register("notebook.focusBottom", |workbench, _, window, cx| {
            workbench.move_notebook_focus(isize::MAX, false, window, cx);
            crate::commands::Outcome::done()
        });
        self.commands.register("notebook.focusNextEditor", |workbench, _, window, cx| {
            workbench.move_notebook_focus(1, true, window, cx);
            crate::commands::Outcome::done()
        });
        self.commands.register("notebook.focusPreviousEditor", |workbench, _, window, cx| {
            workbench.move_notebook_focus(-1, true, window, cx);
            crate::commands::Outcome::done()
        });
        self.commands.register("notebook.cell.edit", |workbench, args, window, cx| {
            let Some((tab, handle)) = workbench.cell_target(&args) else { return crate::commands::Outcome::done() };
            workbench.notebook_views.views.entry(tab).or_default().editing.insert(handle);
            workbench.focus_notebook_cell(tab, handle, true, window, cx);
            crate::commands::Outcome::done()
        });
        self.commands.register("notebook.cell.quitEdit", |workbench, args, window, cx| {
            let Some((tab, handle)) = workbench.cell_target(&args) else { return crate::commands::Outcome::done() };
            if let Some(view) = workbench.notebook_views.views.get_mut(&tab) {
                view.editing.remove(&handle);
            }
            workbench.focus_notebook_cell(tab, handle, false, window, cx);
            crate::commands::Outcome::done()
        });
    }

    /// notebookContextKeys.ts and notebookCellList.ts keys of the active notebook editor: focus in the list or a cell
    /// editor, the focused cell's type and edit mode, and where the cell editor's cursor sits.
    pub(super) fn notebook_context(&self, cell_editor: &OpenEditor, editor_focused: bool, window: &Window, context: &mut HashMap<String, Value>) {
        let list_focused = self.notebook_views.list_focus.is_focused(window);
        // An interactive window or REPL editor: `isCompositeNotebook` inside it; its input editor is no cell editor.
        let composite = self.active_interactive();
        if let Some(interactive) = composite {
            context.insert("activeEditor".into(), json!(if interactive.repl { "workbench.editor.repl" } else { "workbench.editor.interactive" }));
            context.insert("isCompositeNotebook".into(), json!(list_focused || editor_focused));
            if editor_focused && uri_parts(&interactive.input).is_ok_and(|(scheme, path)| cell_editor.document().is(scheme, &path)) {
                context.insert("inputFocus".into(), json!(true));
                context.insert("notebookEditorFocused".into(), json!(false));
                context.insert("interactiveInputCursorAtBoundary".into(), json!(cursor_boundaries(cell_editor).0));
                return;
            }
        }
        // notebookFind.ts: the find widget lies inside the notebook editor, its visibility is `findWidgetVisible`.
        let find_focused = self.notebook_find_focused(window);
        context.insert("notebookEditorFocused".into(), json!(list_focused || editor_focused || find_focused));
        context.insert("findWidgetVisible".into(), json!(self.notebook_find_visible()));
        context.insert("notebookFindWidgetFocused".into(), json!(find_focused));
        context.insert("notebookCellListFocused".into(), json!(list_focused || editor_focused));
        context.insert("notebookCellEditorFocused".into(), json!(editor_focused));
        let Some(editor) = self.active_notebook_editor() else { return };
        let Some(model) = self.host.notebooks.model(&editor.notebook) else { return };
        context.insert("notebookType".into(), json!(model.view_type));
        self.notebook_kernel_context(model, context);
        // notebookEditorWidget.ts and cellContextKeys.ts: `!isReadOnly`; only the interactive window's and REPL
        // editor's lists are read-only.
        context.insert("notebookEditable".into(), json!(composite.is_none()));
        let Some(cell) = model.cells.get(editor.focus) else { return };
        context.insert("notebookCellEditable".into(), json!(composite.is_none()));
        context.insert("notebookCellType".into(), json!(if cell.kind == MARKUP { "markup" } else { "code" }));
        let editing = self.notebook_views.views.get(&editor.tab_model_id).is_some_and(|view| view.editing.contains(&cell.handle));
        context.insert("notebookCellMarkdownEditMode".into(), json!(cell.kind == MARKUP && editing));
        let (boundary, line_boundary) = match editor_focused {
            true => cursor_boundaries(cell_editor),
            false => ("none", "none"),
        };
        context.insert("notebookEditorCursorAtBoundary".into(), json!(boundary));
        context.insert("notebookEditorCursorAtLineBoundary".into(), json!(line_boundary));
    }

    /// Moves the focused cell by `delta` (clamped, `isize::MIN`/`MAX` for the first and last) and reveals it; with
    /// `editor` the new cell's editor takes the keyboard, its cursor on the line next to the cell left
    /// (`focusEditorLine`), unless it is a markup preview. Cells of collapsed sections are not in the list.
    fn move_notebook_focus(&mut self, delta: isize, editor: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(notebook_editor) = self.active_notebook_editor() else { return };
        let (tab, focus) = (notebook_editor.tab_model_id, notebook_editor.focus);
        let Some(model) = self.host.notebooks.model(&notebook_editor.notebook) else { return };
        if model.cells.is_empty() {
            return;
        }
        let visible: Vec<usize> = self.notebook_rows(tab, model).iter().filter(|row| !row.hidden).map(|row| row.index).collect();
        let Some(last) = visible.len().checked_sub(1) else { return };
        let position = visible.iter().rposition(|index| *index <= focus).unwrap_or(0);
        let target = visible[(position as isize).saturating_add(delta).clamp(0, last as isize) as usize];
        if editor && target == focus {
            return;
        }
        let cell = &model.cells[target];
        let (handle, kind, uri) = (cell.handle, cell.kind, cell.uri.clone());
        let preview = kind == MARKUP && !self.notebook_views.views.get(&tab).is_some_and(|view| view.editing.contains(&handle));
        self.focus_notebook_cell(tab, handle, editor && !preview, window, cx);
        self.reveal_notebook_cells(tab, target, target + 1, CellReveal::InView, cx);
        if editor && !preview && let Some((scheme, path)) = uri_parts(&uri).ok().map(|(scheme, path)| (scheme.to_owned(), path)) {
            let line = if delta > 0 { 0 } else { self.editors.model(&scheme, &path).map_or(0, |model| model.document().lines.len() - 1) };
            if let Some(model) = self.editors.model_mut(&scheme, &path) {
                let position = crate::editor::Position { line, column: 0 };
                model.set_selections(vec![crate::editor::Selection { anchor: position, head: position }]);
            }
        }
    }

    /// Brings the webview up to date with the model: previews of markup cells not being edited, the outputs of code
    /// cells by their preferred renderer, and where each sits (`view-scroll`).
    fn sync_back_layer(&mut self, tab: u32, notebook: &str) {
        let Some(model) = self.host.notebooks.model(notebook) else { return };
        let rows = self.notebook_rows(tab, model);
        let mut previews = Vec::new();
        let mut outputs = Vec::new();
        for row in &rows {
            let cell = &model.cells[row.index];
            if row.kind == MARKUP {
                previews.push((cell.handle, self.cell_text(&cell.uri), cell.mime.clone().unwrap_or("text/markdown".into()), cell.metadata.clone(), row));
                continue;
            }
            // A collapsed section's outputs leave the webview and come back when it expands.
            if row.hidden {
                continue;
            }
            let mut offset = row.layout.output_top;
            for (output, height) in cell.outputs.iter().zip(self.output_heights(tab, cell)) {
                let picked = self.output_presentations(&model.view_type, output).into_iter().find_map(|(mime, renderer)| renderer.map(|renderer| (mime, renderer.id)));
                outputs.push((cell.handle, cell.internal_metadata["executionId"].clone(), output.clone(), picked, row.layout.top, offset));
                offset += height.unwrap_or(0.);
            }
        }
        let Some(back_layer) = self.notebook_views.views.get_mut(&tab).and_then(|view| view.webview.as_mut()).filter(|layer| layer.initialized) else { return };
        let shown: HashSet<u64> = previews.iter().filter(|preview| !preview.4.editing && !preview.4.hidden).map(|preview| preview.0).collect();
        let gone: Vec<u64> = back_layer.previews.keys().filter(|handle| !previews.iter().any(|preview| preview.0 == **handle)).copied().collect();
        if !gone.is_empty() {
            back_layer.post(json!({ "type": "deleteMarkupCell", "ids": gone.iter().map(|handle| cell_id(*handle)).collect::<Vec<_>>() }));
            for handle in gone {
                back_layer.previews.remove(&handle);
                back_layer.hidden_previews.remove(&handle);
            }
        }
        for (handle, content, mime, metadata, row) in &previews {
            let id = cell_id(*handle);
            if row.editing || row.hidden {
                if back_layer.previews.contains_key(handle) && back_layer.hidden_previews.insert(*handle) {
                    back_layer.post(json!({ "type": "hideMarkupCells", "ids": [id] }));
                }
                continue;
            }
            match back_layer.previews.get(handle) {
                None => back_layer.post(json!({ "type": "createMarkupCell", "cell": { "mime": mime, "cellId": id, "cellHandle": handle, "content": content, "offset": row.layout.top, "visible": true, "metadata": metadata } })),
                Some(sent) if sent != content || back_layer.hidden_previews.contains(handle) => {
                    back_layer.post(json!({ "type": "showMarkupCell", "id": id, "handle": handle, "content": content, "top": row.layout.top, "metadata": metadata }));
                }
                Some(_) => {}
            }
            back_layer.hidden_previews.remove(handle);
            back_layer.previews.insert(*handle, content.clone());
        }
        let wanted: HashSet<&str> = outputs.iter().map(|output| output.2.id.as_str()).collect();
        let cleared: Vec<String> = back_layer.outputs.keys().filter(|id| !wanted.contains(id.as_str())).cloned().collect();
        for id in cleared {
            let (handle, _, _) = back_layer.outputs.remove(&id).expect("listed above");
            back_layer.output_heights.remove(&id);
            back_layer.post(json!({ "type": "clearOutput", "cellId": cell_id(handle), "outputId": id, "rendererId": null }));
        }
        for (handle, execution_id, output, picked, cell_top, offset) in &outputs {
            let Some((mime, renderer_id)) = picked else { continue };
            // notebookEditorWidget.ts `createOutput`: `notebookRendererMessaging.prepare`.
            drop(self.host.activate_by_event(&format!("onRenderer:{renderer_id}")));
            let item = output.items.iter().find(|item| item.mime == *mime).expect("picked from the output's items");
            match back_layer.outputs.get(&output.id) {
                Some((_, sent, bytes)) if sent == mime && *bytes == item.data => continue,
                // cellOutput.ts `_pickActiveMimeTypeRenderer`: another presentation removes the inset first.
                Some((_, sent, _)) if sent != mime => back_layer.post(json!({ "type": "clearOutput", "cellId": cell_id(*handle), "outputId": output.id, "rendererId": null })),
                _ => {}
            }
            let mut metadata = output.metadata.clone();
            // `createMetadata`: an image's alt text is its text/plain item.
            if mime.starts_with("image") && let Some(text) = output.items.iter().find(|item| item.mime == "text/plain") {
                metadata["vscode_altText"] = json!(String::from_utf8_lossy(&text.data));
            }
            let message = json!({
                "type": "html",
                "executionId": execution_id,
                "cellId": cell_id(*handle),
                "cellTop": cell_top,
                "outputOffset": offset,
                "left": 0,
                "requiredPreloads": [],
                "createOnIdle": false,
                "outputId": output.id,
                "rendererId": renderer_id,
                "content": {
                    "type": 1,
                    "outputId": output.id,
                    "metadata": metadata,
                    "output": { "mime": mime, "valueBytes": { "$$vscode_array_buffer_reference$$": true, "index": 0, "view": { "type": 2, "byteLength": item.data.len(), "byteOffset": 0 } } },
                    "allOutputs": output.items.iter().map(|item| json!({ "mime": item.mime })).collect::<Vec<_>>(),
                },
                "initiallyHidden": false,
            });
            back_layer.post_with_buffer(message, &item.data);
            back_layer.outputs.insert(output.id.clone(), (*handle, mime.clone(), item.data.clone()));
        }
        let widgets: Vec<Value> = outputs
            .iter()
            .filter(|output| output.3.is_some())
            .map(|(handle, _, output, _, cell_top, offset)| json!({ "cellId": cell_id(*handle), "outputId": output.id, "cellTop": cell_top, "outputOffset": offset, "forceDisplay": true }))
            .collect();
        let markup: Vec<Value> = previews.iter().filter(|preview| shown.contains(&preview.0)).map(|preview| json!({ "id": cell_id(preview.0), "top": preview.4.layout.top })).collect();
        let scroll = json!({ "type": "view-scroll", "widgets": widgets, "markupCells": markup });
        if scroll != back_layer.last_scroll {
            back_layer.post(scroll.clone());
            back_layer.last_scroll = scroll;
        }
    }

    /// The cells of the notebook editor of `tab` with their layout.
    pub(super) fn notebook_rows(&self, tab: u32, model: &NotebookModel) -> Vec<CellRow> {
        let view = self.notebook_views.views.get(&tab);
        let back_layer = view.and_then(|view| view.webview.as_ref());
        let mut inputs = Vec::new();
        // notebookViewModelImpl.ts: a REPL editor's list leaves out its input cell, the last.
        let repl = self.host.notebooks.interactives.iter().any(|interactive| interactive.repl && interactive.notebook == model.uri_string());
        let cells = &model.cells[..model.cells.len() - usize::from(repl)];
        // foldingModel.ts: the header cells with a section; those collapsed hide the rest of their section.
        let ranges = if repl { Vec::new() } else { self.notebook_folding_ranges(model) };
        let collapsed = |handle: u64| view.is_some_and(|view| view.collapsed.contains(&handle));
        let mut hidden = HashSet::new();
        for (start, end) in &ranges {
            if collapsed(model.cells[*start].handle) && !hidden.contains(start) {
                hidden.extend(start + 1..=*end);
            }
        }
        for (index, cell) in cells.iter().enumerate() {
            let editing = view.is_some_and(|view| view.editing.contains(&cell.handle));
            let input = match cell.kind == CODE || editing {
                _ if hidden.contains(&index) => CellInput::Hidden,
                true => {
                    let outputs_height = self.output_heights(tab, cell).into_iter().flatten().sum();
                    CellInput::Editor { lines: self.cell_line_count(&cell.uri), outputs_height, markup: cell.kind != CODE }
                }
                false => CellInput::Preview { height: back_layer.and_then(|layer| layer.preview_heights.get(&cell.handle)).copied(), collapsed: collapsed(cell.handle) && ranges.iter().any(|(start, _)| *start == index) },
            };
            inputs.push(input);
        }
        let (layouts, _) = layout::layout(&inputs, self.notebook_line_height());
        let mut rows = Vec::new();
        for (index, (cell, layout)) in cells.iter().zip(layouts).enumerate() {
            let fold = ranges.iter().find(|(start, _)| *start == index).map(|(_, end)| (collapsed(cell.handle), end - index));
            let editing = view.is_some_and(|view| view.editing.contains(&cell.handle));
            rows.push(CellRow { handle: cell.handle, index, kind: cell.kind, editing, layout, hidden: hidden.contains(&index), fold });
        }
        rows
    }

    /// notebookEditorWidget.ts `getEditorViewState` saved into the memento (`_saveEditorViewState`): the focused cell,
    /// whether its editor had the keyboard, the scroll, the markup cells in edit mode and each cell editor's state.
    pub(super) fn save_notebook_view_state(&mut self, tab: u32) {
        let Some(editor) = self.host.notebooks.editor(tab) else { return };
        let Some(model) = self.host.notebooks.model(&editor.notebook) else { return };
        let view = self.notebook_views.views.get(&tab);
        let viewport = self.editors.viewport;
        let mut editor_view_states = serde_json::Map::new();
        for cell in &model.cells {
            if let Some(cell_editor) = self.cell_model(&cell.uri) {
                editor_view_states.insert(cell.handle.to_string(), cell_editor.view_state(viewport));
            }
        }
        let editing: serde_json::Map<String, Value> = view.map(|view| view.editing.iter().map(|handle| (handle.to_string(), json!(true))).collect()).unwrap_or_default();
        let state = json!({
            "editingCells": editing,
            "collapsedInputCells": {},
            "collapsedOutputCells": {},
            "cellLineNumberStates": {},
            "editorViewStates": editor_view_states,
            "scrollPosition": { "left": 0, "top": view.map_or(0., |view| view.scroll_top) },
            "focus": editor.focus,
            "editorFocused": view.is_some_and(|view| view.editor_focused),
        });
        let resource = model.uri_string();
        self.notebook_views.view_states.save(&resource, state);
    }

    /// `_loadNotebookEditorViewState` and `restoreListViewState` for a new notebook editor.
    pub(super) fn restore_notebook_view_state(&mut self, tab: u32) {
        let Some(editor) = self.host.notebooks.editor(tab) else { return };
        let Some(model) = self.host.notebooks.model(&editor.notebook) else { return };
        let Some(state) = self.notebook_views.view_states.load(&model.uri_string()) else { return };
        let cells: Vec<(u64, Value)> = model.cells.iter().map(|cell| (cell.handle, cell.uri.clone())).collect();
        let focus = state["focus"].as_u64().map(|focus| focus as usize).filter(|focus| *focus < cells.len()).unwrap_or(0);
        if let Some(editor) = self.host.notebooks.editors.iter_mut().find(|editor| editor.tab_model_id == tab) {
            editor.focus = focus;
            editor.selections = vec![(focus, focus + 1)];
        }
        let view = self.notebook_views.views.entry(tab).or_default();
        view.scroll_top = state["scrollPosition"]["top"].as_f64().unwrap_or(0.) as f32;
        view.editing = cells.iter().filter(|(handle, _)| state["editingCells"][handle.to_string()] == true).map(|(handle, _)| *handle).collect();
        view.restore_editor_focus = state["editorFocused"] == true;
        let viewport = self.editors.viewport;
        for (handle, uri) in cells {
            let Some(cell_state) = state["editorViewStates"].get(handle.to_string()) else { continue };
            let Ok((scheme, path)) = uri_parts(&uri) else { continue };
            if let Some(cell_editor) = self.editors.model_mut(scheme, &path) {
                cell_editor.restore_view_state(cell_state, viewport);
            }
        }
    }

    /// Whether the notebook editor of `tab` came back with the keyboard in its focused cell's editor.
    pub(super) fn take_notebook_restore_editor_focus(&mut self, tab: u32) -> bool {
        self.notebook_views.views.get_mut(&tab).is_some_and(|view| std::mem::take(&mut view.restore_editor_focus))
    }

    /// The memento from workspace storage, and back into it on shutdown.
    pub(super) fn load_notebook_view_states(&mut self) {
        let stored = self.workspace_storage.lock().unwrap().get(VIEW_STATE_STORAGE_KEY).map(str::to_owned);
        self.notebook_views.view_states = stored.map(|raw| crate::editor::view_state::ViewStates::from_memento_key(&raw, VIEW_STATE_MEMENTO_KEY)).unwrap_or_default();
    }

    pub(super) fn store_notebook_view_states(&mut self) {
        let tabs: Vec<u32> = self.host.notebooks.editors.iter().map(|editor| editor.tab_model_id).collect();
        for tab in tabs {
            self.save_notebook_view_state(tab);
        }
        let memento = self.notebook_views.view_states.to_memento_key(VIEW_STATE_MEMENTO_KEY);
        self.workspace_storage.lock().unwrap().set(VIEW_STATE_STORAGE_KEY, memento);
    }

    /// Scrolls the list of the notebook editor of `tab` to cells `start..end` (notebookEditorWidget.ts
    /// `revealCellRangeInView`, `revealInCenter`, `revealInCenterIfOutsideViewport`, `revealInViewAtTop`).
    pub(super) fn reveal_notebook_cells(&mut self, tab: u32, start: usize, end: usize, reveal: CellReveal, cx: &mut Context<Self>) {
        let Some(model) = self.host.notebooks.editor(tab).and_then(|editor| self.host.notebooks.model(&editor.notebook)) else { return };
        let rows = self.notebook_rows(tab, model);
        let (Some(first), Some(last)) = (rows.get(start), rows.get(end.max(start + 1) - 1)) else { return };
        let (top, bottom) = (first.layout.top, last.layout.top + last.layout.height);
        let view = self.notebook_views.views.entry(tab).or_default();
        let height = f32::from(view.bounds.get().size.height);
        let center = |top: f32, bottom: f32| (top + bottom) / 2. - height / 2.;
        let scroll_top = match reveal {
            CellReveal::InView if top < view.scroll_top || bottom - top > height => top,
            CellReveal::InView if bottom > view.scroll_top + height => bottom - height,
            CellReveal::InView => view.scroll_top,
            CellReveal::Center => center(top, first.layout.top + first.layout.height),
            CellReveal::CenterIfOutside if top < view.scroll_top || top > view.scroll_top + height => center(top, first.layout.top + first.layout.height),
            CellReveal::CenterIfOutside => view.scroll_top,
            CellReveal::Top => top,
        };
        self.set_list_scroll(ListId::Notebook(tab), point(0., scroll_top), cx);
    }

    /// notebookEditorWidget.ts `layout`: the list's content and its `paddingBottom`, the list height less 50 with
    /// `editor.scrollBeyondLastLine` outside the interactive and REPL history lists.
    fn notebook_scroll_height(&self, tab: u32) -> f32 {
        let Some(model) = self.host.notebooks.editor(tab).and_then(|editor| self.host.notebooks.model(&editor.notebook)) else { return 0. };
        let content_height = self.notebook_rows(tab, model).last().map_or(layout::LIST_PADDING_TOP, |row| row.layout.top + row.layout.height);
        let history = self.host.notebooks.interactives.iter().any(|interactive| interactive.notebook == model.uri_string());
        if history || self.setting("editor.scrollBeyondLastLine") == json!(false) {
            return content_height;
        }
        let height = self.notebook_views.views.get(&tab).map_or(0., |view| f32::from(view.bounds.get().size.height));
        content_height + (height - 50.).max(0.)
    }

    pub(super) fn notebook_list_scroller(&self, tab: u32) -> ListScroller {
        let view = self.notebook_views.views.get(&tab);
        let bounds = view.map(|view| view.bounds.get()).unwrap_or_default();
        let top = view.map_or(0., |view| view.scroll_top);
        ListScroller::Notebook { bounds, top, max: self.notebook_scroll_height(tab) - f32::from(bounds.size.height) }
    }

    pub(super) fn set_notebook_scroll_top(&mut self, tab: u32, top: f32) {
        self.notebook_views.views.entry(tab).or_default().scroll_top = top;
    }

    /// mainThreadNotebookEditors.ts `onDidChangeVisibleRanges`: each shown notebook editor's cells overlapping its
    /// viewport (all of them before its first layout), sent when they change.
    pub(super) fn send_notebook_visible_ranges(&mut self) {
        let mut changed = Vec::new();
        for editor in &self.host.notebooks.editors {
            let Some(model) = self.host.notebooks.model(&editor.notebook) else { continue };
            if !self.host.notebooks.sent_editors.contains(&editor.id) {
                continue;
            }
            let rows = self.notebook_rows(editor.tab_model_id, model);
            let view = self.notebook_views.views.get(&editor.tab_model_id);
            let (top, height) = view.map_or((0., 0.), |view| (view.scroll_top, f32::from(view.bounds.get().size.height)));
            let visible: Vec<usize> = rows.iter().filter(|row| height <= 0. || (row.layout.top < top + height && row.layout.top + row.layout.height > top)).map(|row| row.index).collect();
            let ranges = match (visible.first(), visible.last()) {
                (Some(first), Some(last)) => vec![(*first, last + 1)],
                _ => Vec::new(),
            };
            if editor.visible_ranges.as_ref() != Some(&ranges) {
                changed.push((editor.id.clone(), ranges));
            }
        }
        for (id, ranges) in changed {
            let wire: Vec<Value> = ranges.iter().map(|(start, end)| json!({ "start": start, "end": end })).collect();
            self.host.send("ExtHostNotebookEditors", "$acceptEditorPropertiesChanged", vec![json!(id), json!({ "visibleRanges": { "ranges": wire } })]);
            if let Some(editor) = self.host.notebooks.editors.iter_mut().find(|editor| editor.id == id) {
                editor.visible_ranges = Some(ranges);
            }
        }
    }

    /// The outline entries of a notebook's cells (`NotebookCellOutlineDataSource.recomputeState`).
    pub(super) fn notebook_outline(&self, model: &NotebookModel) -> Vec<outline::Entry> {
        let key = model.uri_string();
        let texts: Vec<String> = model.cells.iter().map(|cell| self.cell_text(&cell.uri)).collect();
        let cells: Vec<outline::CellSource> = model
            .cells
            .iter()
            .zip(&texts)
            .map(|(cell, text)| outline::CellSource { markup: cell.kind == MARKUP, text, executing: cell.kind == CODE && self.host.notebooks.kernels.state(&key, cell.handle).is_some() })
            .collect();
        outline::entries(&cells)
    }

    /// The active notebook editor's resource and its breadcrumb outline elements (icon, label), with
    /// `breadcrumbs.symbolPath`.
    pub(super) fn notebook_breadcrumbs(&self, foreground: gpui::Rgba, line_height: f32) -> Option<(std::path::PathBuf, Vec<(Option<Div>, String)>)> {
        let editor = self.active_notebook_editor()?;
        let model = self.host.notebooks.model(&editor.notebook)?;
        let (_, path) = uri_parts(&model.uri).ok()?;
        let symbol_path = self.setting("breadcrumbs.symbolPath");
        if symbol_path == "off" {
            return Some((path, Vec::new()));
        }
        let entries = self.notebook_outline(model);
        let active = outline::active(&entries, editor.focus);
        let mut chain = outline::breadcrumbs(&entries, active, self.setting("notebook.breadcrumbs.showCodeCells") != false);
        if symbol_path == "last" {
            chain.drain(..chain.len().saturating_sub(1));
        }
        let show_icons = self.setting("breadcrumbs.icons") != false;
        let items = chain.into_iter().map(|entry| (show_icons.then(|| self.outline_entry_icon(model, entry, foreground, line_height)).flatten(), entry.label.clone())).collect();
        Some((path, items))
    }

    /// `NotebookOutlineRenderer.renderElement`: a code cell that is not running shows its language's file icon when
    /// the file icon theme has file icons, every other entry its codicon.
    pub(super) fn outline_entry_icon(&self, model: &NotebookModel, entry: &outline::Entry, foreground: gpui::Rgba, line_height: f32) -> Option<Div> {
        let language = &model.cells[entry.cell].language;
        if !entry.markup && entry.icon == "code" && let Some(icon) = self.file_icons.as_ref().and_then(|icons| icons.language_icon(language, self.theme.kind, 13., line_height, foreground)) {
            return Some(icon);
        }
        Some(self.icon(entry.icon, 16., 16., foreground).flex_none())
    }

    fn notebook_line_height(&self) -> f32 {
        LINE_HEIGHT * self.editor_zoom.multiplier()
    }

    fn cell_line_count(&self, cell_uri: &Value) -> usize {
        self.cell_model(cell_uri).map_or(1, |editor| editor.document().lines.len())
    }

    pub(super) fn cell_model(&self, cell_uri: &Value) -> Option<&OpenEditor> {
        let (scheme, path) = uri_parts(cell_uri).ok()?;
        self.editors.model(scheme, &path)
    }

    /// The notebook toolbar and the cell list of the active notebook editor.
    pub(super) fn notebook_editor_body(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let Some((tab, notebook)) = self.active_notebook_tab() else { return div() };
        let Some(model) = self.host.notebooks.model(&notebook) else { return div().flex_1() };
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(self.notebook_toolbar(model, window, cx))
            .child(self.notebook_list(tab, model, window, cx))
            .children(self.interactive_input_cell(model, window, cx))
    }

    /// notebookEditorToolbar.ts: `+ Code`, `+ Markdown`, Run All, and the kernel picker on the right.
    fn notebook_toolbar(&self, model: &NotebookModel, window: &Window, cx: &mut Context<Self>) -> Div {
        let foreground = self.color("foreground");
        let icon_color = self.color("icon.foreground");
        let button = |id: &'static str, icon: &str, label: &str, command: &'static str, args: Vec<Value>, cx: &mut Context<Self>| {
            div()
                .id(id)
                .h(px(22.))
                .px(px(4.))
                .flex()
                .items_center()
                .gap(px(4.))
                .rounded(px(5.))
                .cursor_pointer()
                .hover(|button| button.bg(self.color("toolbar.hoverBackground")))
                .child(self.icon(icon, 16., 16., icon_color))
                .child(label.to_owned())
                .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                    drop(workbench.execute_command(command, args.clone(), window, cx));
                }))
        };
        // `updateKernelStatusAction`: the bound kernel, else the only one.
        let kernel = self.host.notebooks.kernels.resolve(&model.view_type, &model.uri_string()).map(|kernel| kernel.label.clone());
        let kernel_label = kernel.unwrap_or("Select Kernel".into());
        let toolbar = div().h(px(TOOLBAR_HEIGHT)).flex_none().px(px(8.)).flex().items_center().gap(px(4.)).text_size(px(13.)).text_color(foreground);
        // The interactive window's and REPL editor's toolbar (`MenuId.InteractiveToolbar`): its `navigation` items on the
        // left with the toolbar's context (notebookEditorToolbar.ts), the kernel picker on the right.
        if self.host.notebooks.interactives.iter().any(|interactive| interactive.notebook == model.uri_string()) {
            let context = self.key_context(window);
            let argument = json!({ "ui": true, "notebookEditor": { "notebookUri": model.uri }, "source": "notebookToolbar" });
            // executeActions.ts: Interrupt (group `navigation/execute`, enabled while an interruptible kernel runs a
            // cell) and, in the interactive window while a cell runs, Go to Running Cell (`navigation`, order 10).
            let execution = self.active_notebook_execution();
            let running = execution.as_ref().is_some_and(|(_, _, _, running)| !running.is_empty());
            let interruptible = self.host.notebooks.kernels.resolve(&model.view_type, &model.uri_string()).is_some_and(|kernel| kernel.supports_interrupt);
            let interrupt = match running && interruptible {
                true => button("notebook-interrupt", "primitive-square", "Interrupt", "notebook.interruptExecution", Vec::new(), cx),
                false => div().id("notebook-interrupt").h(px(22.)).px(px(4.)).flex().items_center().gap(px(4.)).opacity(0.4).child(self.icon("primitive-square", 16., 16., icon_color)).child("Interrupt"),
            };
            let mut items: Vec<(String, f32, gpui::AnyElement)> = vec![("navigation/execute".into(), 0., interrupt.into_any_element())];
            if running && context.get("activeEditor") == Some(&json!("workbench.editor.interactive")) {
                items.push(("navigation".into(), 10., self.command_button("notebook.revealRunningCell", "Go to Running Cell".into(), Vec::new(), icon_color, cx).into_any_element()));
            }
            for item in self.menus.iter().filter(|item| item.menu == crate::menus::MenuId::InteractiveToolbar && item.group.starts_with("navigation") && item.when.as_ref().is_none_or(|when| when.evaluate(&context))) {
                let crate::menus::Entry::Command { id, .. } = &item.entry else { continue };
                items.push((item.group.clone(), item.order as f32, self.command_button(id, item.title.clone(), vec![argument.clone()], icon_color, cx).into_any_element()));
            }
            items.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
            let toolbar = toolbar.children(items.into_iter().map(|(_, _, element)| element));
            return toolbar.child(div().flex_1()).child(button("notebook-kernel", "server-environment", &kernel_label, "_notebook.selectKernel", Vec::new(), cx));
        }
        toolbar
            .child(button("notebook-insert-code", "add", "Code", "notebook.cell.insertCodeCellBelowAndFocusContainer", Vec::new(), cx))
            .child(button("notebook-insert-markdown", "add", "Markdown", "notebook.cell.insertMarkdownCellBelow", Vec::new(), cx))
            // notebookToolbar.css: 8px item margins around the separator, as measured in the reference.
            .child(div().ml(px(4.)).mr(px(2.)).w(px(1.)).h(px(16.)).bg(self.color("menu.separatorBackground")))
            .child(button("notebook-run-all", "run-all", "Run All", "notebook.execute", Vec::new(), cx))
            .children(self.notebook_toolbar_more(window, cx).map(|more| more.ml(px(4.))))
            .child(div().flex_1())
            .child(button("notebook-kernel", "server-environment", &kernel_label, "_notebook.selectKernel", Vec::new(), cx))
    }

    /// notebookCellList.ts: the cells at their tops less the scroll, the webview placed over the list.
    fn notebook_list(&self, tab: u32, model: &NotebookModel, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let rows = self.notebook_rows(tab, model);
        let content_height = rows.last().map_or(layout::LIST_PADDING_TOP, |row| row.layout.top + row.layout.height);
        let view = self.notebook_views.views.get(&tab);
        let scroll_top = view.map_or(0., |view| view.scroll_top);
        let focused_index = self.host.notebooks.editor(tab).map(|editor| editor.focus);
        let bounds_cell = view.map(|view| view.bounds.clone()).unwrap_or_default();
        let list_width = f32::from(bounds_cell.get().size.width);
        // The back layer draws over GPUI: it leaves the find widget uncovered.
        let find_hole = self.notebook_find_widget_bounds(tab, list_width);
        let webview = view.and_then(|view| view.webview.as_ref()).filter(|layer| layer.initialized).map(|layer| (layer.webview.view(), layer.clip.clone()));
        let covered = self.quick_inputs.current().is_some() || self.keybindings_editor.open || self.context_menu.is_some();
        // The webview takes presses over outputs and previews only (`ClipView`).
        let mut hit_rects = Vec::new();
        for row in rows.iter().filter(|row| !row.hidden) {
            let top = row.layout.top - scroll_top;
            match row.kind == CODE || row.editing {
                true => hit_rects.push(Bounds::new(point(px(layout::CODE_CELL_LEFT_MARGIN + layout::CELL_RUN_GUTTER), px(top + row.layout.output_top)), size(px(10000.), px(row.layout.height - row.layout.output_top - layout::BOTTOM_TOOLBAR_GAP - layout::CELL_BOTTOM_MARGIN)))),
                false => hit_rects.push(Bounds::new(point(px(layout::MARKDOWN_CELL_GUTTER), px(top)), size(px(10000.), px(row.layout.output_top)))),
            }
        }
        let mut list = div()
            .id(("notebook-list", tab as usize))
            .track_focus(&self.notebook_views.list_focus)
            .flex_1()
            .min_h_0()
            .relative()
            .overflow_hidden()
            .child(
                canvas(
                    move |bounds, window, _| {
                        bounds_cell.set(bounds);
                        let Some((view, clip)) = &webview else { return };
                        if covered {
                            clip.setHidden(true);
                            return;
                        }
                        let frame = Bounds::new(point(px(0.), px(-scroll_top)), size(bounds.size.width, px(content_height.max(f32::from(bounds.size.height) + scroll_top))));
                        // The cells end 16px left of the list's right edge: the 10px ruler and scrollbar there stay uncovered.
                        let clip_bounds = Bounds::new(bounds.origin, size(bounds.size.width - px(overview_ruler::RULER_WIDTH), bounds.size.height));
                        clip.place(view, window, clip_bounds, frame, hit_rects.clone(), find_hole);
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            );
        for row in rows.iter().filter(|row| !row.hidden) {
            let focused = focused_index == Some(row.index);
            list = list.child(self.notebook_cell(tab, model, row, focused, scroll_top, window, cx));
        }
        let list = list
            .children(self.cell_drop_indicator(tab, &rows, scroll_top))
            .child(self.notebook_overview_ruler(tab, &rows, window))
            .child(self.list_scroll_layer(ListId::Notebook(tab), cx))
            .children(self.notebook_find_widget(tab, list_width, window, cx));
        div().flex_1().min_h_0().flex().flex_col().child(list)
    }

    /// One cell at its place in the list.
    #[allow(clippy::too_many_arguments)]
    fn notebook_cell(&self, tab: u32, model: &NotebookModel, row: &CellRow, focused: bool, scroll_top: f32, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let cell = &model.cells[row.index];
        let top = row.layout.top - scroll_top;
        let handle = cell.handle;
        let mut container = div()
            .absolute()
            .left_0()
            .right_0()
            .top(px(top))
            .h(px(row.layout.height))
            .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, _: &MouseDownEvent, window, cx| workbench.focus_notebook_cell(tab, handle, true, window, cx)))
            .on_drag_move(cx.listener(move |workbench, event: &gpui::DragMoveEvent<dnd::CellDrag>, _, cx| workbench.cell_drag_move(tab, handle, event, cx)))
            // The drop runs in the capture phase: a press's release in the cell's content stops before the bubble phase.
            .capture_any_mouse_up(cx.listener(move |workbench, event: &gpui::MouseUpEvent, _, cx| {
                if cx.has_active_drag() {
                    workbench.drop_cell(tab, handle, event.modifiers.alt, cx);
                }
            }));
        let first_line = self.cell_model(&cell.uri).map(|editor| editor.document().lines[0].clone()).unwrap_or_default();
        container = container.children(self.cell_drag_handle(tab, row, first_line));
        // cellFocusIndicator.ts: the focused cell's left bars, one over its code and one over its outputs,
        // `notebook.focusedCellBorder` while the focus is in the list (`.monaco-list:focus-within`), else
        // `notebook.inactiveFocusedCellBorder`.
        let input = self.editors.active().filter(|active| self.is_interactive_input(active)).is_some();
        let list_focused = self.notebook_views.list_focus.is_focused(window) || (self.editor_focus.is_focused(window) && !input);
        // notebookEditorWidget.ts: the bars also show, inactive, on a hovered row; 3px wide, the output bar 3px
        // (`focusIndicatorGap`) below the code bar.
        let hovered = self.notebook_views.views.get(&tab).is_some_and(|view| view.hovered == Some(row.handle));
        if focused || hovered {
            let color = if focused && list_focused { self.color("notebook.focusedCellBorder") } else { self.color("notebook.inactiveFocusedCellBorder") };
            let bar = |top: f32, height: f32| div().absolute().left(px(layout::FOCUS_INDICATOR_LEFT_MARGIN)).top(px(top)).w(px(3.)).h(px(height)).rounded(px(1.5)).bg(color);
            let outputs_height = row.layout.height - row.layout.output_top - layout::BOTTOM_TOOLBAR_GAP - layout::CELL_BOTTOM_MARGIN;
            match row.kind == CODE || row.editing {
                true => container = container.child(bar(row.layout.editor_top, row.layout.editor_height + layout::STATUS_BAR_HEIGHT)),
                false => container = container.child(bar(row.layout.editor_top, row.layout.output_top)),
            }
            if row.kind == CODE && outputs_height > layout::FOCUS_INDICATOR_GAP {
                container = container.child(bar(row.layout.output_top + layout::FOCUS_INDICATOR_GAP, outputs_height - layout::FOCUS_INDICATOR_GAP));
            }
        }
        // notebookCellTitleToolbar.css: with `notebook.cellToolbarVisibility` `click` (the default) the focused row's
        // toolbar; with `hover` the hovered row's and the focused one's while the list has the focus; and the row
        // whose `...` dropdown is open.
        let dropdown = self.context_menu.is_some() && self.notebook_views.views.get(&tab).is_some_and(|view| view.toolbar_menu == Some(row.handle));
        let shows_toolbar = match self.setting("notebook.cellToolbarVisibility") == "hover" {
            true => hovered || (focused && list_focused) || dropdown,
            false => focused || dropdown,
        };
        container = container.child(self.insert_toolbar(tab, row, cx));
        container = container.children(self.fold_indicator(tab, row, focused, hovered, cx)).children(self.folded_hint(tab, model, row, cx));
        if row.kind != CODE && !row.editing {
            return container.children(shows_toolbar.then(|| self.cell_toolbar(tab, model, row, window, cx)).flatten());
        }
        let left = layout::CODE_CELL_LEFT_MARGIN + layout::CELL_RUN_GUTTER;
        let editor_box = div()
            .absolute()
            .left(px(left))
            .right(px(layout::CELL_RIGHT_MARGIN))
            .top(px(row.layout.editor_top))
            .h(px(row.layout.editor_height + layout::STATUS_BAR_HEIGHT))
            .rounded(px(4.))
            .border_1()
            // notebook.css `.cell-editor-focus .cell-editor-part:before`: the focused cell's editor with the keyboard.
            .border_color(match focused && self.editor_focus.is_focused(window) && !input {
                true => self.color("notebook.focusedEditorBorder"),
                false => self.theme.color("notebook.cellBorderColor").unwrap_or(self.color("editorWidget.border")),
            })
            .bg(self.theme.color("notebook.cellEditorBackground").unwrap_or(self.color("editor.background")))
            .overflow_hidden()
            .child(self.cell_editor(tab, cell, row.layout.editor_height, window, cx))
            .child(self.cell_status_bar(model, row.index, focused, window, cx));
        container = container.child(editor_box);
        if row.kind == CODE {
            container = container.child(self.cell_run_gutter(tab, model, row, cx)).children(self.output_toolbars(tab, model, row, cx));
        }
        container.children(shows_toolbar.then(|| self.cell_toolbar(tab, model, row, window, cx)).flatten())
    }

    /// The cell's code editor, between the editor paddings.
    fn cell_editor(&self, tab: u32, cell: &notebook::Cell, height: f32, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let Some(editor) = self.cell_model(&cell.uri) else { return div() };
        let handle = cell.handle;
        let inactive = !self.editors.active().is_some_and(|active| std::ptr::eq(active, editor));
        let text_height = height - layout::EDITOR_TOP_PADDING - layout::EDITOR_BOTTOM_PADDING;
        // The box's 1px border takes layout space here and none upstream (an outline), so the text starts 1px higher and
        // 1px further left.
        div().absolute().left(px(-1.)).right_0().top(px(layout::EDITOR_TOP_PADDING - 1.)).h(px(text_height)).flex().overflow_hidden()
            .debug_selector(move || format!("notebook-cell-editor-{handle}"))
            // The lines of an inactive cell editor act on the active editor: the cell takes the focus first, in the capture phase,
            // so the line's own press places the cursor in this cell.
            .when(inactive, |area| {
                area.capture_any_mouse_down(cx.listener(move |workbench, event: &MouseDownEvent, window, cx| {
                    if event.button == MouseButton::Left {
                        workbench.focus_notebook_cell(tab, handle, true, window, cx);
                    }
                }))
            })
            .child(self.embedded_code_editor(editor, window, cx))
    }

    /// cellExecution.ts and the run toolbar: the run button over the gutter of a focused cell, the execution order
    /// (`[n]`) or the running state at the bottom of the gutter.
    fn cell_run_gutter(&self, tab: u32, model: &NotebookModel, row: &CellRow, cx: &mut Context<Self>) -> Div {
        let cell = &model.cells[row.index];
        let state = self.host.notebooks.kernels.state(&model.uri_string(), cell.handle);
        // cellExecution.ts: the order shows with a kernel that implements it, or without a kernel.
        let kernel = self.host.notebooks.kernels.resolve(&model.view_type, &model.uri_string());
        let order = cell.internal_metadata["executionOrder"].as_u64().filter(|_| kernel.is_none_or(|kernel| kernel.supports_execution_order));
        let label = match state {
            Some(ExecutionState::Pending | ExecutionState::Unconfirmed) => "[ ]".to_owned(),
            Some(ExecutionState::Executing) => "[*]".to_owned(),
            None => order.map_or(String::new(), |order| format!("[{order}]")),
        };
        let index = row.index;
        let uri = model.uri.clone();
        let run = self
            .icon(if state.is_some() { "stop" } else { "play" }, 22., 16., self.color("icon.foreground"))
            .id(("notebook-run-cell", row.handle as usize))
            .absolute()
            .top(px(row.layout.editor_top + 4.))
            .left(px(layout::CODE_CELL_LEFT_MARGIN + 6.))
            .rounded(px(5.))
            .cursor_pointer()
            .hover(|button| button.bg(self.color("toolbar.hoverBackground")))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                drop(workbench.execute_command("notebook.cell.execute", vec![json!({ "start": index, "end": index + 1 }), uri.clone()], window, cx));
            }))
            .child(self.hover_target(("notebook-run-cell-hover", row.handle as usize), HoverContent::Text("Execute Cell".into()), HoverOptions::mouse()));
        // The interactive window's and REPL editor's `cellExecuteToolbar` (`MenuId.InteractiveCellExecute`) is empty.
        let composite = self.host.notebooks.interactives.iter().any(|interactive| interactive.notebook == model.uri_string());
        let shows_run = !composite && (self.notebook_views.views.get(&tab).is_some_and(|view| view.hovered == Some(row.handle)) || self.host.notebooks.editor(tab).is_some_and(|editor| editor.focus == row.index));
        div()
            .child(
                div()
                    .absolute()
                    .left(px(layout::CODE_CELL_LEFT_MARGIN))
                    .w(px(layout::CELL_RUN_GUTTER))
                    .top(px(row.layout.editor_top + row.layout.editor_height))
                    .h(px(layout::STATUS_BAR_HEIGHT))
                    .flex()
                    .items_center()
                    .justify_center()
                    .font_family(super::editor_view::EDITOR_FONT)
                    .text_size(px(12.))
                    .text_color(self.color("descriptionForeground"))
                    .child(label),
            )
            .when(shows_run, |gutter| gutter.child(run))
    }

    /// cellToolbars.ts `BetweenCellToolbar`: `+ Code` and `+ Markdown` in the gap below the cell, shown on hover;
    /// each inserts below this cell.
    fn insert_toolbar(&self, tab: u32, row: &CellRow, cx: &mut Context<Self>) -> Div {
        let group = gpui::SharedString::from(format!("notebook-insert-{}", row.handle));
        let handle = row.handle;
        let button = |id: &'static str, label: &str, command: &'static str, cx: &mut Context<Self>| {
            div()
                .id((id, handle as usize))
                .h(px(layout::BOTTOM_TOOLBAR_HEIGHT + 2.))
                .px(px(6.))
                .flex()
                .items_center()
                .gap(px(2.))
                .rounded(px(3.))
                .border_1()
                .border_color(self.color("notebook.cellToolbarSeparator"))
                .bg(self.color("editor.background"))
                .cursor_pointer()
                .hover(|button| button.bg(self.color("toolbar.hoverBackground")))
                .child(self.icon("add", 14., 14., self.color("icon.foreground")))
                .child(label.to_owned())
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                    workbench.focus_notebook_cell(tab, handle, false, window, cx);
                    drop(workbench.execute_command(command, Vec::new(), window, cx));
                }))
        };
        div()
            .group(group.clone())
            .absolute()
            .left_0()
            .right_0()
            .top(px(layout::bottom_toolbar_offset(&row.layout) - layout::BOTTOM_TOOLBAR_HEIGHT / 2.))
            .h(px(layout::BOTTOM_TOOLBAR_HEIGHT * 2.))
            .flex()
            .items_center()
            .justify_center()
            .gap(px(8.))
            .text_size(px(12.))
            .text_color(self.color("foreground"))
            .child(
                div()
                    .invisible()
                    .group_hover(group, |style| style.visible())
                    .flex()
                    .gap(px(8.))
                    .child(button("notebook-insert-code-below", "Code", "notebook.cell.insertCodeCellBelow", cx))
                    .child(button("notebook-insert-markdown-below", "Markdown", "notebook.cell.insertMarkdownCellBelow", cx)),
            )
    }

    /// Posts `message` to the webview of each notebook editor `accepts` takes (by editor id and notebook); whether one
    /// had a webview to take it (backLayerWebView.ts `receiveMessageHandler`, `postKernelMessage`).
    pub(super) fn post_to_notebook_webviews(&mut self, message: Value, accepts: impl Fn(&notebook::NotebookEditor) -> bool) -> bool {
        let tabs: Vec<u32> = self.host.notebooks.editors.iter().filter(|editor| accepts(editor)).map(|editor| editor.tab_model_id).collect();
        let mut sent = false;
        for tab in tabs {
            if let Some(back_layer) = self.notebook_views.views.get_mut(&tab).and_then(|view| view.webview.as_mut()) {
                back_layer.post(message.clone());
                sent = true;
            }
        }
        sent
    }

    /// The active notebook editor shows markup cell `handle` in edit mode.
    pub(super) fn edit_markup_cell(&mut self, handle: u64) {
        if let Some((tab, _)) = self.active_notebook_tab() {
            self.notebook_views.views.entry(tab).or_default().editing.insert(handle);
        }
    }

    fn delete_notebook_cell(&mut self, tab: u32, handle: u64, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(notebook) = self.host.notebooks.editor(tab).map(|editor| editor.notebook.clone()) else { return };
        let Some(model) = self.host.notebooks.model(&notebook) else { return };
        let Some(index) = model.index_of_handle(handle) else { return };
        let data = self.cell_data(&model.cells[index]);
        let delete = json!({ "editType": 1, "index": index, "count": 1, "cells": [] });
        if self.apply_notebook_edits(&notebook, std::slice::from_ref(&delete), cx).is_ok() {
            self.push_cells_undo(&notebook, vec![json!({ "editType": 1, "index": index, "count": 0, "cells": [data] })], vec![delete]);
        }
        self.editors_changed(cx);
    }

    /// A cell as `ICellDto2` data, its text included.
    pub(super) fn cell_data(&self, cell: &notebook::Cell) -> Value {
        json!({
            "cellKind": cell.kind,
            "language": cell.language,
            "mime": cell.mime,
            "source": self.cell_text(&cell.uri),
            "outputs": cell.outputs.iter().map(notebook::Output::dto).collect::<Vec<_>>(),
            "metadata": cell.metadata,
            "internalMetadata": cell.internal_metadata,
        })
    }

    /// A cell edit of notebook `key` goes on its undo stack and clears what was undone.
    pub(super) fn push_cells_undo(&mut self, key: &str, undo: Vec<Value>, redo: Vec<Value>) {
        let Some(model) = self.host.notebooks.model_mut(key) else { return };
        model.undo.push(notebook::CellsUndo { stamp: crate::editor::next_undo_stamp(), undo, redo });
        model.redo.clear();
    }

    /// Undo or redo in the active notebook editor (notebookEditor.ts `UndoCommand` over the notebook's undo
    /// group): the latest of its cell edits and its cells' text edits, or for redo the one undone last.
    /// None outside a notebook editor.
    pub(super) fn notebook_undo(&mut self, redo: bool, cx: &mut Context<Self>) -> Option<crate::commands::Outcome> {
        let (tab, notebook) = self.active_notebook_tab()?;
        let model = self.host.notebooks.model(&notebook)?;
        let pick = |stamps: &mut Vec<(u64, Option<u64>)>| match redo {
            true => stamps.iter().min_by_key(|(stamp, _)| *stamp).copied(),
            false => stamps.iter().max_by_key(|(stamp, _)| *stamp).copied(),
        };
        // (stamp, Some(cell handle)) for a cell's text edit, (stamp, None) for a cell edit.
        let mut stamps = Vec::new();
        let structural = if redo { model.redo.last() } else { model.undo.last() };
        stamps.extend(structural.map(|entry| (entry.stamp, None)));
        for cell in &model.cells {
            let (undo, redone) = self.cell_model(&cell.uri).map(OpenEditor::undo_stamps).unwrap_or_default();
            stamps.extend(if redo { redone } else { undo }.map(|stamp| (stamp, Some(cell.handle))));
        }
        let Some((_, target)) = pick(&mut stamps) else { return Some(crate::commands::Outcome::done()) };
        match target {
            Some(handle) => {
                let index = model.index_of_handle(handle).expect("listed from the model");
                if let Some(editor) = self.host.notebooks.editors.iter_mut().find(|editor| editor.tab_model_id == tab) {
                    editor.focus = index;
                    editor.selections = vec![(index, index + 1)];
                }
                self.editors_changed(cx);
                Some(self.run_on_active_editor(if redo { OpenEditor::redo } else { OpenEditor::undo }, cx))
            }
            None => {
                let model = self.host.notebooks.model_mut(&notebook).expect("found above");
                let entry = if redo { model.redo.pop() } else { model.undo.pop() }.expect("picked from the stack");
                let edits = if redo { &entry.redo } else { &entry.undo };
                // Inverse edits were computed one after another, so they apply one after another; a redo is the
                // original batch.
                let applied = match redo {
                    true => self.apply_notebook_edits(&notebook, edits, cx).map(|_| ()),
                    false => edits.iter().try_for_each(|edit| self.apply_notebook_edits(&notebook, std::slice::from_ref(edit), cx).map(|_| ())),
                };
                let model = self.host.notebooks.model_mut(&notebook).expect("found above");
                match redo {
                    true => model.undo.push(entry),
                    false => model.redo.push(entry),
                }
                self.editors_changed(cx);
                Some(crate::commands::Outcome::Ready(applied.map(|_| Value::Null)))
            }
        }
    }
}

impl BackLayer {
    /// backLayerWebView.ts `_sendMessageToWebview`.
    fn post(&mut self, message: Value) {
        self.webview.send("message", json!({ "message": message.to_string(), "buffers": [] }));
    }

    /// A message whose `$$vscode_array_buffer_reference$$` index 0 is `bytes`.
    fn post_with_buffer(&mut self, message: Value, bytes: &[u8]) {
        use base64::Engine;
        let buffer = base64::engine::general_purpose::STANDARD.encode(bytes);
        self.webview.send("message", json!({ "message": message.to_string(), "buffers": [buffer] }));
    }
}

/// baseCellViewModel.ts `cursorAtBoundary` and `cursorAtLineBoundary` of an empty primary selection.
fn cursor_boundaries(editor: &OpenEditor) -> (&'static str, &'static str) {
    let selection = editor.selection();
    if !selection.is_empty() {
        return ("none", "none");
    }
    let lines = &editor.document().lines;
    let head = selection.head;
    let boundary = match (head.line == 0, head.line + 1 == lines.len()) {
        (true, true) => "both",
        (true, false) => "top",
        (false, true) => "bottom",
        (false, false) => "none",
    };
    let length = lines[head.line].chars().count();
    let line_boundary = match head.column {
        _ if length == 0 => "both",
        0 => "start",
        column if column == length => "end",
        _ => "none",
    };
    (boundary, line_boundary)
}

fn cell_id(handle: u64) -> String {
    format!("cell-{handle}")
}

fn parse_cell_id(id: &str) -> Option<u64> {
    id.strip_prefix("cell-")?.parse().ok()
}

/// common/webview.ts `asWebviewUri` of a file.
fn webview_uri(path: &std::path::Path) -> String {
    let path = path.to_string_lossy();
    format!("https://file+.vscode-resource.vscode-cdn.net{}", path.replace('%', "%25").replace(' ', "%20").replace('#', "%23"))
}

/// `encodeURIComponent`.
fn query_component(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')' => encoded.push(byte as char),
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

mod cell_actions;
mod dnd;
mod find;
mod folding;
mod outputs;
mod overview_ruler;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn encodes_as_encode_uri_component() {
        assert_eq!(query_component("{\"a\":\"b c/é\"}"), "%7B%22a%22%3A%22b%20c%2F%C3%A9%22%7D");
    }

    #[test]
    fn cell_ids_round_trip() {
        assert_eq!(parse_cell_id(&cell_id(42)), Some(42));
        assert_eq!(parse_cell_id("output-1"), None);
    }
}
