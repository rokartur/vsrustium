//! The empty workbench as the reference draws it with no folder open and the modern floating layout
//! (src/vs/workbench/browser/media/floatingPanels.css). Sizes are the reference's CSS pixels.

use std::path::PathBuf;

use gpui::{Context, Div, Rgba, Window, div, prelude::*, px, rgb, svg, transparent_black};

use crate::theme::{Theme, ThemeKind};

pub struct Workbench {
    pub theme: Theme,
    /// The repo root: codicon SVGs and the letterpress are read from upstream's own files under it.
    pub app_root: PathBuf,
}

impl Render for Workbench {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
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
            .child(div().flex_1().flex().flex_row().gap(px(4.)).px(px(4.)).pb(px(4.)).child(self.activity_bar()).child(self.editor_part()))
            .child(self.status_bar());
        div()
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(self.color("titleBar.activeBackground"))
            .text_color(self.color("foreground"))
            .text_size(px(13.))
            .child(grid)
            // shortcut: 16px is the macOS 26+ (Tahoe) corner radius (style.css .macos-tahoe); older macOS uses 10px, branch on the OS version if the port supports it.
            .when_some(window_border, |root, border| root.child(div().absolute().inset_0().border_1().rounded(px(16.)).border_color(border)))
    }
}

impl Workbench {
    /// No color resolves to transparent, as an unset CSS variable does.
    fn color(&self, key: &str) -> Rgba {
        self.theme.color(key).unwrap_or(transparent_black().into())
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
            .child("Search");
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
        let layout_actions = ["layout", "layout-sidebar-left-off", "layout-panel-off", "layout-sidebar-right-off"];
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
        let views = ["files", "search-large", "source-control", "debug-alt", "extensions"];
        let inactive = self.color("activityBar.inactiveForeground");
        div()
            .w(px(44.))
            .flex()
            .flex_col()
            .items_center()
            .justify_between()
            .py(px(3.))
            .border_1()
            .rounded(px(8.))
            .bg(self.color("activityBar.background"))
            .border_color(self.color("surface.border"))
            .child(div().flex().flex_col().gap(px(8.)).children(views.map(|name| self.icon(name, 36., 24., inactive))))
            .child(self.icon("settings-gear", 36., 24., inactive))
    }

    fn editor_part(&self) -> Div {
        // letterpress-hcDark.svg paints its own fill; the others are black at the opacity the file sets (editorgroupview.css).
        let (file, color) = match self.theme.kind {
            ThemeKind::Dark => ("letterpress-dark.svg", rgb(0x000000)),
            ThemeKind::Light => ("letterpress-light.svg", rgb(0x000000)),
            ThemeKind::HighContrastDark => ("letterpress-hcDark.svg", rgb(0x3c3c3c)),
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
        let foreground = self.color("statusBar.noFolderForeground");
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
            .bg(self.color("statusBar.noFolderBackground"))
            .text_color(foreground)
            .text_size(px(12.))
            .child(div().h(px(24.)).flex().child(self.icon("remote", 28., 16., foreground).h_full()).child(problems))
            .child(self.icon("bell", 28., 16., foreground).h(px(24.)))
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
