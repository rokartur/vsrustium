//! The workbench as the reference draws it in the modern floating layout
//! (src/vs/workbench/browser/media/floatingPanels.css), empty or with a folder in the Explorer.
//! Sizes are the reference's CSS pixels.

mod actions;
mod editor_commands;
mod editor_view;
mod keyboard;
pub mod quick_access;
mod quick_input_view;
mod text_input;

use std::cell::Cell;
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use gpui::{ClickEvent, Context, Div, FocusHandle, FontFeatures, FontWeight, Rgba, Task, UniformListScrollHandle, Window, div, prelude::*, px, rgb, svg, transparent_black};
use serde_json::Value;

use crate::commands::Commands;
use crate::editor::{EditorGroup, TextDocument};
use crate::explorer::FileTree;
use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::{Theme, ThemeKind};

pub struct Workbench {
    pub theme: Theme,
    /// The repo root: codicon SVGs and the letterpress are read from upstream's own files under it.
    pub app_root: PathBuf,
    /// The opened folder; `None` is the empty window.
    pub folder: Option<FileTree>,
    pub languages: Languages,
    pub editors: EditorGroup,
    pub(crate) host: HostSync,
    pub commands: Commands,
    /// Extension tests run: dialogs answer their default without showing (abstractFileDialogService.ts skipDialogs).
    pub skip_dialogs: bool,
    pub keybindings: keyboard::Keybindings,
    /// Context keys set with the `setContext` command.
    context_keys: HashMap<String, Value>,
    /// The status bar message and the task that hides it.
    status_message: Option<(String, Task<()>)>,
    editor_scroll: UniformListScrollHandle,
    /// Window x of the text area, recorded at paint, to turn a click into a column.
    text_area_left: Rc<Cell<f32>>,
    pub editor_focus: FocusHandle,
    /// The IME's marked text: a UTF-16 range of the cursor's line.
    marked_text: Option<Range<usize>>,
    /// A left button press in the text is held, so moves extend the selection.
    selecting_text: bool,
    pub quick_inputs: quick_input_view::QuickInputs,
    pub quick_access: quick_access::QuickAccess,
}

impl Render for Workbench {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        let window_border = self.theme.color("window.activeBorder");
        // The reference lays the grid out at the full window size and shifts it inside the 1px window border.
        let inset = if window_border.is_some() { 1. } else { 0. };
        let grid = div()
            .absolute()
            .left(px(inset))
            .top(px(inset))
            .w(viewport.width)
            .h(viewport.height)
            .flex()
            .flex_col()
            .child(self.title_bar(f32::from(viewport.width)))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_row()
                    .gap(px(4.))
                    .px(px(4.))
                    .pb(px(4.))
                    // The side bar meets the activity bar flush; the bar draws the seam (floatingPanels.css).
                    .child(div().flex().child(self.activity_bar()).when(self.folder.is_some(), |row| row.child(self.side_bar(cx))))
                    .child(if self.editors.active().is_some() { self.text_editor_part(window, cx) } else { self.editor_part() }),
            )
            .child(self.status_bar());
        let quick_input = self.quick_input_widget(window, cx);
        div()
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(self.color("titleBar.activeBackground"))
            .text_color(self.color("foreground"))
            .text_size(px(13.))
            .child(grid)
            .children(quick_input)
            // shortcut: 16px is the macOS 26+ (Tahoe) corner radius (style.css .macos-tahoe); older macOS uses 10px, branch on the OS version if the port supports it.
            .when_some(window_border, |root, border| root.child(div().absolute().inset_0().border_1().rounded(px(16.)).border_color(border)))
    }
}

impl Workbench {
    pub fn new(theme: Theme, app_root: PathBuf, folder: Option<FileTree>, languages: Languages, host: HostSync, cx: &mut Context<Self>) -> Self {
        let mut workbench = Workbench {
            theme,
            app_root,
            folder,
            languages,
            editors: EditorGroup::default(),
            host,
            commands: Commands::default(),
            skip_dialogs: false,
            keybindings: keyboard::Keybindings::default(),
            context_keys: HashMap::new(),
            status_message: None,
            editor_scroll: UniformListScrollHandle::new(),
            text_area_left: Rc::default(),
            editor_focus: cx.focus_handle(),
            marked_text: None,
            selecting_text: false,
            quick_inputs: quick_input_view::QuickInputs::new(cx),
            quick_access: quick_access::QuickAccess::default(),
        };
        crate::main_thread_commands::register_commands_documentation(&mut workbench);
        workbench.register_workbench_actions();
        workbench.register_editor_commands();
        workbench.register_context_commands();
        workbench.register_quick_input_commands();
        workbench.register_quick_access_commands();
        // Keybindings resolve before the focused element sees the key, and with nothing focused too.
        let entity = cx.entity().downgrade();
        cx.intercept_keystrokes(move |event, window, cx| {
            let _ = entity.update(cx, |workbench, cx| workbench.dispatch_key(&event.keystroke, window, cx));
        })
        .detach();
        workbench
    }

    /// The opened folder, which quick pick items show their resource paths relative to.
    pub fn workspace_path(&self) -> Option<&Path> {
        self.folder.as_ref().map(|folder| folder.root.as_path())
    }

    /// A single click opens a preview editor and keeps the focus in the Explorer, a double click opens a pinned
    /// one and focuses it (`workbench.editor.enablePreview`).
    pub fn open_file(&mut self, path: PathBuf, pinned: bool, window: &mut Window, cx: &mut Context<Self>) {
        match self.editors.find(&path) {
            Some(index) => {
                self.editors.activate(index);
                if pinned {
                    self.editors.pin(index);
                }
            }
            None => match TextDocument::read(&path, &self.languages) {
                Ok(document) => self.editors.open(document, !pinned),
                // shortcut: a read error is logged, not shown as the reference's editor error placeholder.
                Err(error) => eprintln!("opening {}: {error}", path.display()),
            },
        }
        self.marked_text = None;
        match pinned && self.editors.active().is_some() {
            true => window.focus(&self.editor_focus, cx),
            false => window.blur(cx),
        }
        self.editors_changed(cx);
    }

    /// Keeps the Explorer selection, the scroll position and the extension host in step with the group.
    fn editors_changed(&mut self, cx: &mut Context<Self>) {
        if let (Some(tree), Some(editor)) = (self.folder.as_mut(), self.editors.active()) {
            tree.reveal(&editor.document.path);
        }
        self.editor_scroll = UniformListScrollHandle::new();
        self.host.send_changes(&self.editors);
        cx.notify();
    }

    /// No color resolves to transparent, as an unset CSS variable does.
    fn color(&self, key: &str) -> Rgba {
        self.theme.color(key).unwrap_or(transparent_black().into())
    }

    /// A foreground key with no color inherits, as CSS `color` does; every part here inherits `foreground`.
    fn foreground_or(&self, key: &str) -> Rgba {
        self.theme.color(key).unwrap_or(self.color("foreground"))
    }

    fn title_bar(&self, viewport_width: f32) -> Div {
        let foreground = self.color("titleBar.activeForeground");
        // Disabled codicon actions take disabledForeground (actionbar.css .disabled .action-label:not(.icon)).
        let disabled = self.color("disabledForeground");
        let command_center = div()
            .flex()
            .items_center()
            // .command-center-center is 38vw wide plus its border (titlebarpart.css).
            .w(px(viewport_width * 0.38 + 2.))
            .h(px(24.))
            .pl(px(8.))
            .border_1()
            .rounded(px(6.))
            .bg(self.color("commandCenter.background"))
            .border_color(self.color("commandCenter.border"))
            .text_color(self.color("commandCenter.foreground"))
            .text_size(px(12.))
            .child(self.folder.as_ref().map_or("Search".into(), |folder| folder.root.file_name().unwrap_or_default().to_string_lossy().into_owned()));
        let center = div()
            .absolute()
            .top(px(6.5))
            .w_full()
            .h(px(24.))
            .flex()
            .justify_center()
            .items_center()
            .child(self.icon("arrow-left", 22., 16., disabled))
            .child(self.icon("arrow-right", 22., 16., disabled).ml(px(4.)))
            .child(command_center.ml(px(10.)))
            .child(div().w(px(6.)));
        let side_bar_toggle = if self.folder.is_some() { "layout-sidebar-left" } else { "layout-sidebar-left-off" };
        let layout_actions = ["layout", side_bar_toggle, "layout-panel-off", "layout-sidebar-right-off"];
        let right = div()
            .absolute()
            .top(px(7.5))
            .right(px(12.))
            .flex()
            .gap(px(4.))
            .children(layout_actions.map(|name| self.icon(name, 22., 16., foreground)));
        div().relative().h(px(35.)).flex_none().child(center).child(right)
    }

    fn activity_bar(&self) -> Div {
        let views = ["search-large", "source-control", "debug-alt", "extensions"];
        let inactive = self.color("activityBar.inactiveForeground");
        let explorer = if self.folder.is_some() { self.active_view_icon("files") } else { self.icon("files", 36., 24., inactive) };
        div()
            .w(px(44.))
            .flex()
            .flex_col()
            .items_center()
            .justify_between()
            .py(px(3.))
            .border_1()
            .when(self.folder.is_some(), |bar| bar.rounded_l(px(8.)))
            .when(self.folder.is_none(), |bar| bar.rounded(px(8.)))
            .bg(self.color("activityBar.background"))
            .border_color(self.color("surface.border"))
            .child(div().flex().flex_col().gap(px(8.)).child(explorer).children(views.map(|name| self.icon(name, 36., 24., inactive))))
            .child(self.icon("settings-gear", 36., 24., inactive))
    }

    /// The checked view: a 32px rounded indicator behind the icon (activityaction.css); High Contrast
    /// hides it and outlines the item in contrastActiveBorder instead (menuActivityBar.css `.hc-black, .hc-light`).
    fn active_view_icon(&self, name: &str) -> Div {
        let indicator = div().absolute().top(px(2.)).left(px(2.)).size(px(32.)).rounded(px(4.));
        let indicator = if self.theme.kind.is_high_contrast() {
            indicator.when_some(self.theme.color("contrastActiveBorder"), |indicator, outline| indicator.border_1().border_color(outline))
        } else {
            indicator.when_some(self.theme.color("modernActivityBarItem.activeBackground"), |indicator, background| indicator.bg(background))
        };
        div().relative().child(indicator).child(self.icon(name, 36., 24., self.color("modernActivityBarItem.activeForeground")))
    }

    /// The Explorer: title, the folder section with its tree, and the collapsed Outline and Timeline sections.
    fn side_bar(&self, cx: &mut Context<Self>) -> Div {
        let tree = self.folder.as_ref().expect("side bar is drawn only with a folder");
        let foreground = self.foreground_or("sideBar.foreground");
        let title = div()
            .h(px(32.))
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .px(px(4.))
            .when_some(self.theme.color("sideBarTitle.border"), |title, border| title.border_b_1().border_color(border))
            // The h2 keeps its 32px line under the HC border-bottom, so it is laid out from the top, not centered.
            .child(div().self_start().pl(px(8.)).line_height(px(32.)).text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(self.foreground_or("sideBarTitle.foreground")).child("Explorer"))
            .child(self.icon("ellipsis", 22., 16., foreground));
        // explorerviewlet.css: in high contrast an explorer item has a 20px line from the row top, 1px above center.
        let label_line = if self.theme.kind.is_high_contrast() { 20. } else { 22. };
        // The tree is not focused once the editor has focus, so the selection takes the inactive colors.
        let selection = self.color("list.inactiveSelectionBackground");
        // listWidget.ts: a selected row gets a 1px dotted listSelectionOutline (activeContrastBorder) inside it.
        let selection_outline = self.theme.color("contrastActiveBorder");
        let selected = tree.selected.clone();
        let folder_name = tree.root.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let rows = tree.rows().into_iter().enumerate().map(|(index, row)| {
            let twistie = match (row.is_dir, row.expanded) {
                (true, true) => Some("chevron-down"),
                (true, false) => Some("chevron-right"),
                (false, _) => None,
            };
            let path = row.path;
            let is_dir = row.is_dir;
            div()
                .id(index)
                .h(px(22.))
                .mx(px(4.))
                .flex()
                .items_center()
                .rounded(px(4.))
                // monaco-tl-twistie: 3px row inset, 8px base indent plus 8px per level, a 16px icon, 6px before the label.
                .pl(px(11. + 8. * (row.depth - 1) as f32))
                .child(match twistie {
                    Some(name) => self.icon(name, 16., 16., foreground),
                    None => div().size(px(16.)).flex_none(),
                })
                .child(div().self_start().ml(px(3.)).line_height(px(label_line)).child(row.name))
                .when(selected.as_ref() == Some(&path), |row| {
                    // shortcut: dashed, GPUI has no dotted border.
                    // An overlay, since a border on the row would push its content 1px like a CSS border, not an outline.
                    let outline = selection_outline.map(|color| div().absolute().inset_0().rounded(px(4.)).border_1().border_dashed().border_color(color));
                    row.relative().bg(selection).children(outline)
                })
                .on_click(cx.listener(move |workbench, event: &ClickEvent, window, cx| {
                    if is_dir {
                        workbench.folder.as_mut().expect("rows exist only with a folder").toggle(&path);
                        cx.notify();
                    } else {
                        workbench.open_file(path.clone(), event.click_count() == 2, window, cx);
                    }
                }))
        });
        div()
            .w(px(256.))
            .flex()
            .flex_col()
            // The seam with the activity bar is the bar's border; the side bar keeps a transparent 1px edge.
            .pl(px(1.))
            .border_t_1()
            .border_r_1()
            .border_b_1()
            .rounded_r(px(8.))
            .bg(self.color("sideBar.background"))
            .border_color(self.color("surface.border"))
            .text_color(foreground)
            .child(title)
            .child(div().flex_1().overflow_hidden().flex().flex_col().child(self.section_header(&folder_name, true, false)).children(rows))
            .child(self.section_header("Outline", false, true))
            .child(self.section_header("Timeline", false, true))
            // The pane view is laid out 5px shorter than the side bar content.
            .child(div().h(px(5.)).flex_none())
    }

    /// A pane header (paneview.ts): 28px, sideBarSectionHeader colors, a top border on every section but the first.
    fn section_header(&self, title: &str, expanded: bool, separated: bool) -> Div {
        let foreground = self.foreground_or("sideBarSectionHeader.foreground");
        let border = self.theme.color("sideBarSectionHeader.border").filter(|_| separated);
        div()
            .relative()
            .h(px(28.))
            .flex_none()
            .mx(px(4.))
            .pl(px(4.))
            .flex()
            .items_center()
            .rounded(px(4.))
            .bg(self.color("sideBarSectionHeader.background"))
            .text_color(foreground)
            .when_some(border, |header, border| header.child(div().absolute().top_0().left_0().right_0().h(px(1.)).bg(border)))
            .child(self.icon(if expanded { "chevron-down" } else { "chevron-right" }, 16., 16., foreground).mx(px(2.)))
            .child(div().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).child(title.to_string()))
    }

    fn editor_part(&self) -> Div {
        // The high contrast letterpress files paint their own fill; the others are black at the opacity the file sets (editorgroupview.css).
        let (file, color) = match self.theme.kind {
            ThemeKind::Dark => ("letterpress-dark.svg", rgb(0x000000)),
            ThemeKind::Light => ("letterpress-light.svg", rgb(0x000000)),
            ThemeKind::HighContrastDark => ("letterpress-hcDark.svg", rgb(0x3c3c3c)),
            ThemeKind::HighContrastLight => ("letterpress-hcLight.svg", rgb(0xd9d9d9)),
        };
        let path = self.app_root.join("src/vs/workbench/browser/parts/editor/media").join(file);
        // .watermark-container is 280px tall and centered; the 256px letterpress sits at its top.
        let watermark = div().h(px(280.)).child(svg().external_path(path.to_string_lossy().into_owned()).size(px(256.)).text_color(color));
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .border_1()
            .rounded(px(8.))
            .bg(self.color("editor.background"))
            .border_color(self.color("surface.border"))
            .child(watermark)
    }

    fn status_bar(&self) -> Div {
        let (foreground, background) = match self.folder {
            Some(_) => (self.color("statusBar.foreground"), self.color("statusBar.background")),
            None => (self.color("statusBar.noFolderForeground"), self.color("statusBar.noFolderBackground")),
        };
        let problems = div()
            .h_full()
            .ml(px(3.))
            .px(px(5.))
            .flex()
            .items_center()
            .child(self.icon("error", 16., 16., foreground))
            .child(" 0 ")
            .child(self.icon("warning", 16., 16., foreground))
            .child(" 0");
        div()
            .h(px(28.))
            .flex_none()
            .flex()
            .justify_between()
            .items_center()
            .px(px(12.))
            .bg(background)
            .text_color(foreground)
            .text_size(px(12.))
            // statusbarpart.css: font-variant-numeric: tabular-nums.
            .font_features(FontFeatures(Arc::new(vec![("tnum".into(), 1)])))
            .child(div().h(px(24.)).flex().child(self.icon("remote", 28., 16., foreground).h_full()).child(problems).children(self.status_bar_message()))
            .child(div().h(px(24.)).flex().children(self.editor_status_items(foreground)).child(self.icon("bell", 28., 16., foreground).h_full()))
    }

    /// The `status.message` entry: multi-chord mode, else the latest status message (notificationsStatus.ts).
    fn status_bar_message(&self) -> Option<Div> {
        let message = self.chord_status().or_else(|| self.status_message.as_ref().map(|(message, _)| message.clone()))?;
        Some(div().h_full().ml(px(3.)).px(px(5.)).flex().items_center().child(message))
    }

    /// A codicon centered in a square box. Drawn from the icon's SVG: Wu's text system skips fonts
    /// without an 'm' glyph, so codicon.ttf cannot load (gpui_macos text_system.rs load_family).
    fn icon(&self, name: &str, box_size: f32, icon_size: f32, color: Rgba) -> Div {
        let path = self.app_root.join("node_modules/@vscode/codicons/src/icons").join(format!("{name}.svg"));
        div()
            .size(px(box_size))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .child(svg().external_path(path.to_string_lossy().into_owned()).size(px(icon_size)).text_color(color))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use exthost::scan::scan_extension;
    use gpui::{Modifiers, TestAppContext, point, px};

    use super::Workbench;
    use crate::explorer::FileTree;
    use crate::host_sync::HostSync;
    use crate::languages::Languages;
    use crate::theme::load_theme;

    #[gpui::test]
    fn clicking_a_folder_row_toggles_it(cx: &mut TestAppContext) {
        let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
        let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let theme = load_theme(&[themes], "Dark Modern").unwrap();
        let folder = app_root.join("port/fixtures/workspace");
        let docs = folder.join("docs");
        let languages = Languages::new(&[]);
        let host = HostSync::new(std::sync::mpsc::channel().0);
        let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), Some(FileTree::new(folder)), languages, host, cx));
        cx.run_until_parked();

        // The first tree row (docs) spans y 96..118 under the side bar's "workspace" header (reference DOM dump).
        let docs_row = point(px(120.), px(107.));
        let docs_expanded = |cx: &mut gpui::VisualTestContext| {
            workbench.read_with(cx, |workbench, _| workbench.folder.as_ref().unwrap().rows().iter().any(|row| row.path == docs && row.expanded))
        };
        cx.simulate_click(docs_row, Modifiers::none());
        assert!(docs_expanded(cx));
        cx.simulate_click(docs_row, Modifiers::none());
        assert!(!docs_expanded(cx));
    }

    #[gpui::test]
    fn keys_and_typed_text_edit_the_focused_editor_and_reach_the_host(cx: &mut TestAppContext) {
        let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
        let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let theme = load_theme(&[themes], "Dark Modern").unwrap();
        let dir = std::env::temp_dir().join(format!("code-gpui-keys-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, "ab\ncd").unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let (workbench, cx) = cx.add_window_view(|window, cx| {
            let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), HostSync::new(sender), cx);
            workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), dir.join("keybindings.json"), cx);
            workbench.open_file(file.clone(), true, window, cx);
            workbench
        });
        cx.run_until_parked();
        receiver.try_iter().count();
        let text = |cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().document.lines.join("\n"));

        cx.simulate_keystrokes("down end");
        cx.simulate_input("!");
        assert_eq!(text(cx), "ab\ncd!");
        cx.simulate_keystrokes("shift-home cmd-c");
        assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()).as_deref(), Some("cd!"));
        cx.simulate_keystrokes("backspace");
        assert_eq!(text(cx), "ab\n");
        cx.simulate_keystrokes("cmd-z cmd-z enter tab");
        assert_eq!(text(cx), "ab\ncd\n    ");
        cx.simulate_keystrokes("cmd-s");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "ab\ncd\n    ");
        assert!(workbench.read_with(cx, |workbench, _| !workbench.editors.active().unwrap().is_dirty()));
        let methods: Vec<_> = receiver.try_iter().map(|message| message.method).collect();
        assert_eq!(methods.iter().filter(|method| **method == "$acceptModelChanged").count(), 6);
        assert_eq!(methods.last(), Some(&"$acceptModelSaved"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[gpui::test]
    fn user_keybindings_remove_defaults_and_bind_chords(cx: &mut TestAppContext) {
        let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
        let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let theme = load_theme(&[themes], "Dark Modern").unwrap();
        let dir = std::env::temp_dir().join(format!("code-gpui-keybindings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, "abc").unwrap();
        let user_file = dir.join("keybindings.json");
        let rules = r#"// keybindings.json is JSONC
            [{ "key": "end", "command": "-cursorEnd" }, { "key": "cmd+k cmd+e", "command": "cursorEnd", "when": "editorTextFocus && mine" }]"#;
        std::fs::write(&user_file, rules).unwrap();
        let (workbench, cx) = cx.add_window_view(|window, cx| {
            let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), HostSync::new(std::sync::mpsc::channel().0), cx);
            workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), user_file.clone(), cx);
            workbench.open_file(file.clone(), true, window, cx);
            workbench
        });
        cx.run_until_parked();
        let column = |cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().selection.head.column);
        let status = |cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.chord_status());

        cx.simulate_keystrokes("end");
        assert_eq!(column(cx), 0, "the user removed the default End binding");
        cx.simulate_keystrokes("cmd-k cmd-e");
        assert_eq!(column(cx), 0, "the chord's when clause needs the `mine` context key");
        workbench.update_in(cx, |workbench, window, cx| workbench.execute_command("setContext", vec!["mine".into(), true.into()], window, cx));
        cx.simulate_keystrokes("cmd-k");
        assert_eq!(status(cx).as_deref(), Some("(⌘K) was pressed. Waiting for second key of chord..."));
        cx.simulate_keystrokes("cmd-e");
        assert_eq!(column(cx), 3);
        assert_eq!(status(cx), None, "leaving chord mode drops its message");
        cx.simulate_keystrokes("cmd-k x");
        let message = workbench.read_with(cx, |workbench, _| workbench.status_message.as_ref().map(|(message, _)| message.clone()));
        assert_eq!(message.as_deref(), Some("The key combination (⌘K, X) is not a command."));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
