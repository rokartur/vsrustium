//! The editor part with open files: the connected tab strip (connectedEditorTabs.css), the breadcrumbs,
//! and the code editor's view parts (margin, current line, word highlights, indent guides, text, cursor,
//! decorationsOverviewRuler.ts). Sizes are the reference's CSS pixels with editor.fontSize 12 and lineHeight 18.

use std::cell::Cell;
use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use std::time::{Duration, Instant};

use gpui::{
    Axis, Bounds, Context, DispatchPhase, Div, DragMoveEvent, ElementInputHandler, ExternalPaths, FontFeatures, Hitbox, HitboxBehavior, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Point,
    PathBuilder, Rgba, ScrollDelta, ScrollWheelEvent, Stateful, StyledText, Task, Window, canvas, div, fill, font, point, prelude::*, px, rgb, size,
};

pub(super) mod diff_editor_view;
pub(super) mod diff_gutter;
pub(super) mod drag_and_drop;
pub(super) mod injected_text_view;
pub(super) mod merge_editor_view;
pub(super) mod multi_diff_editor_view;
mod comment_glyph;
pub(super) mod quick_diff;
pub(super) mod quick_diff_widget;
pub(super) mod sticky_scroll_view;
pub(super) mod text_decorations;
pub(super) mod unchanged_regions_view;

use files::encoding::SUPPORTED_ENCODINGS;
use serde_json::json;

use super::Workbench;
use super::minimap_view::minimap_info;
use super::search_editor_view::SEARCH_EDITOR_SCHEME;
use super::webview_panels::WEBVIEW_PANEL_SCHEME;
use crate::brackets::BracketRange;
use crate::editor::minimap::MinimapLayoutInfo;
use crate::editor::{
    HORIZONTAL_SCROLLBAR_HEIGHT, Indentation, LINE_HEIGHT, OpenEditor, Position, Reveal, Scrollbar, Selection, VERTICAL_SCROLLBAR_WIDTH, VerticalReveal, Viewport, column_at_visible, commands,
    expand_tabs, line_overflow, show_more_width, visible_column, word_at,
};
use crate::editor::folding::FoldDecoration;
use crate::editor::whitespace::whitespace_marks;
use crate::multi_diff_editor::MULTI_DIFF_EDITOR_SCHEME;
use crate::theme::ThemeKind;

pub(super) const EDITOR_FONT: &str = "Menlo";
/// `editor.fontSize` at zoom level 0.
pub(super) const EDITOR_FONT_SIZE: f32 = 12.;
/// lineDecorationsWidth 10 plus 16 for the folding controls (editorOptions.ts layout).
const DECORATIONS_WIDTH: f32 = 26.;
/// The workbench text editor sets lineNumbersMinChars to 3 (textEditor.ts).
const LINE_NUMBERS_MIN_CHARS: usize = 3;
const OVERVIEW_RULER_WIDTH: f32 = 14.;
const TAB_STRIP_HEIGHT: f32 = 32.;
/// The sticky tabs' own bar above the strip (`pinnedTabsOnSeparateRow`), measured in the reference.
const UPPER_TAB_BAR_HEIGHT: f32 = 30.;

#[derive(Clone, Copy, PartialEq)]
enum TabRow {
    /// The strip connected to the editor; its first tab meets the part's rounded corner.
    Main { first: bool },
    /// The sticky tabs' bar above it (`pinnedTabsOnSeparateRow`).
    Upper,
}
const BREADCRUMBS_HEIGHT: f32 = 22.;
/// scrollableElement.ts `HIDE_TIMEOUT`; scrollbars.css fades in over 100ms and out over 800ms.
const SCROLLBAR_HIDE_TIMEOUT: Duration = Duration::from_millis(500);
const SCROLLBAR_FADE_IN: Duration = Duration::from_millis(100);
const SCROLLBAR_FADE_OUT: Duration = Duration::from_millis(800);
const SCROLL_SHADOW_HEIGHT: f32 = 6.;
const FOLD_CONTROLS_FADE: Duration = Duration::from_millis(500);

impl Workbench {
    /// `secondary`: the drawn secondary side when the active editor is a side by side editor.
    pub(super) fn text_editor_part(&self, secondary: Option<gpui::AnyElement>, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let editor = self.editors.active().expect("text editor part is drawn only with an editor open");
        if !self.editor_groups.drawing_inactive {
            self.schedule_unusual_line_terminators_check(editor, window, cx);
        }
        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .relative()
            .flex()
            .flex_col()
            .child(self.tab_strip(window, cx))
            .map(|part| match editor.document().scheme.as_str() {
                // The playground's focused snippet is the active editor; the page draws it with the others.
                _ if self.playground_active() => part.child(self.playground_body(window, cx)),
                // A webview panel's resource has no file system provider, a custom editor's has.
                WEBVIEW_PANEL_SCHEME if self.webview_resource(&editor.document().path).is_none() => part,
                _ if self.active_notebook_resource().is_some_and(|(scheme, _)| scheme == "untitled") => part,
                super::extension_editor::EXTENSION_EDITOR_SCHEME => part.child(self.extension_editor_body(&editor.document().path, cx)),
                super::welcome_page::WALKTHROUGH_SCHEME => part.child(self.walkthrough_editor_body(&editor.document().path, window, cx)),
                // searchEditor.ts draws its query widget where a text editor has its breadcrumbs.
                SEARCH_EDITOR_SCHEME => part.child(self.search_editor_query(window, cx)),
                crate::output::OUTPUT_SCHEME => part.child(self.output_title_actions(window, cx)),
                MULTI_DIFF_EDITOR_SCHEME => part.child(self.multi_diff_editor_body(window, cx)),
                // breadcrumbsControl.ts `update`: no breadcrumbs for a resource without a file system provider.
                "untitled" => part,
                // mergeEditor.ts: the merge editor has no breadcrumbs.
                _ if self.editors.active_merge().is_some() => part,
                _ if self.setting("breadcrumbs.enabled") == json!(false) => part,
                _ => part.child(self.breadcrumbs(editor, cx).opacity(self.unfocused_editor_opacity(window))),
            })
            .map(|part| match self.editors.active_diff() {
                Some(diff) => part.child(self.diff_editor_body(diff, editor, window, cx)),
                None if self.playground_active() => part,
                None if matches!(editor.document().scheme.as_str(), super::extension_editor::EXTENSION_EDITOR_SCHEME | super::welcome_page::WALKTHROUGH_SCHEME) => part,
                None if self.shows_webview(editor) => part.child(self.webview_editor_body(editor)),
                None if self.editors.active_tab().is_some_and(|tab| tab.document().scheme == crate::notebook::NOTEBOOK_EDITOR_SCHEME) => part.child(self.notebook_editor_body(window, cx)),
                None if editor.document().scheme == MULTI_DIFF_EDITOR_SCHEME => part,
                None if editor.document().open_error.is_some() => part.child(self.editor_placeholder(cx)),
                None => match (self.editors.active_merge(), secondary) {
                    (Some(merge), _) => part.child(self.merge_editor_body(merge, editor, window, cx)),
                    (None, Some(secondary)) => part.child(self.side_by_side_body(secondary, editor, window, cx)),
                    (None, None) => part.children(self.unicode_banner(editor, cx)).child(self.text_area(editor, window, cx).opacity(self.unfocused_editor_opacity(window))),
                },
            })
    }

    /// The active tab reaches into the part's top border, which Chrome still draws over it.
    pub(super) fn part_top_border(&self) -> Div {
        div().absolute().inset(px(-1.)).border_1().rounded(px(8.)).border_color(self.color("surface.border"))
    }

    /// One strip behind every tab; the active tab is an editor-colored fill whose bottom shoulders curve
    /// into the strip, so the active tab and the editor below read as one surface.
    fn tab_strip(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let active = self.editors.active_index().expect("tab strip is drawn only with an editor open");
        let icon_color = self.color("icon.foreground");
        // multieditortabscontrol.css: .editor-actions is cursor default, its action items pointer (actionbar.css).
        let actions = div()
            .ml_auto()
            .cursor_default()
            // Centered above the strip's bottom border, as the tabs are.
            .h(px(TAB_STRIP_HEIGHT - 1.))
            .px(px(4.))
            .flex()
            .items_center()
            .gap(px(4.))
            // searchEditor.contribution.ts: Rerun Search sits in the editor title of a search editor.
            .when(self.active_search_editor().is_some(), |actions| {
                actions.child(self.icon("refresh", 22., 16., icon_color).id("rerun-search").cursor_pointer().on_click(cx.listener(|workbench, _, window, cx| {
                    workbench.execute_command("rerunSearchEditorSearch", Vec::new(), window, cx);
                })))
            })
            // A group that is not active shows only the overflow menu.
            .when(!self.editor_groups.drawing_inactive, |actions| {
                actions.children(self.editor_title_actions(window, cx)).child(self.icon("split-horizontal", 22., 16., icon_color).id("split-editor").cursor_pointer().on_click(cx.listener(|workbench, _, window, cx| {
                    workbench.execute_command("workbench.action.splitEditor", Vec::new(), window, cx);
                })))
            })
            .child(self.icon("ellipsis", 22., 16., icon_color).cursor_pointer().when(self.editors.active_merge().is_some(), |more| {
                more.on_mouse_down(gpui::MouseButton::Left, cx.listener(|workbench, event: &gpui::MouseDownEvent, window, cx| workbench.show_merge_more_actions(event, window, cx)))
            }));
        // The line under the strip (tabs-border-bottom) runs below the shoulders; the active fill covers it.
        let bottom_line = div().absolute().bottom_0().left_0().right_0().h(px(1.)).bg(self.connected_tab_border());
        // multiRowEditorTabsControl.ts: with `workbench.editor.pinnedTabsOnSeparateRow` the sticky tabs get a bar of
        // their own above the others while the group has both (`two-tab-bars`); only the lower bar has the actions.
        let count = self.editors.editors.len();
        let sticky_count = self.editors.sticky_count();
        let two_bars = sticky_count > 0 && sticky_count < count && self.setting("workbench.editor.pinnedTabsOnSeparateRow") == json!(true);
        let lower_tabs = if two_bars { sticky_count..count } else { 0..count };
        // editortabscontrol.css: the whole title, tabs and their close buttons included, is cursor pointer.
        let lower = div()
            .relative()
            .h(px(TAB_STRIP_HEIGHT))
            .flex_none()
            .cursor_pointer()
            .flex()
            .bg(self.tab_strip_background())
            .child(bottom_line)
            .children(lower_tabs.clone().map(|index| self.tab(index, index == active, TabRow::Main { first: index == lower_tabs.start }, cx)))
            .child(actions);
        if !two_bars {
            return lower;
        }
        // connectedEditorTabs.css `.connected-tab-upper-bar`: 2px above and before its tabs, none of them connected.
        let upper = div()
            .h(px(UPPER_TAB_BAR_HEIGHT))
            .flex_none()
            .cursor_pointer()
            .pt(px(2.))
            .pl(px(2.))
            .flex()
            .bg(self.tab_strip_background())
            .children((0..sticky_count).map(|index| self.tab(index, index == active, TabRow::Upper, cx)));
        div().flex_none().flex().flex_col().child(upper).child(lower)
    }

    /// The `navigation` group of `MenuId.EditorTitle` by order: the diff editor's built-in actions and the
    /// extensions' `editor/title` items, run with the active editor's resource.
    fn editor_title_actions(&self, window: &Window, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let mut actions = self.diff_title_actions(cx);
        let mut context = self.key_context(window);
        // A notebook's cell editors keep `editorLangId` in their own scoped context, which the title does not see.
        if self.active_notebook_editor().is_some() {
            context.remove("editorLangId");
        }
        actions.extend(self.multi_diff_title_actions(&context, cx));
        actions.extend(self.merge_title_actions(cx));
        let resource = self.editors.active().map(|editor| editor.document().uri()).into_iter().collect::<Vec<_>>();
        let foreground = self.color("icon.foreground");
        // preferences.contribution.ts `_workbench.openUserSettingsEditor` (order 1) on a user settings.json outside a
        // diff editor, and editor.contribution.ts Toggle Layout of a side by side editor, left of Split Editor.
        let settings_file = self.editors.active_diff().is_none() && self.editors.active().is_some_and(|editor| self.is_user_settings_file(&editor.document().path));
        let side_by_side = self.editors.side_group().is_some();
        for (order, icon, title, command, shown) in [(1., "go-to-file", "Open Settings (UI)", "workbench.action.openSettings2", settings_file), (f64::MAX, "editor-layout", "Toggle Layout", "workbench.action.toggleSplitEditorInGroupLayout", side_by_side)] {
            if shown {
                let button = self.icon(icon, 22., 16., foreground).id(title).cursor_pointer().on_click(cx.listener(move |workbench, _, window, cx| drop(workbench.execute_command(command, Vec::new(), window, cx))));
                actions.push((order, button.into_any_element()));
            }
        }
        for item in self.menus.iter().filter(|item| item.menu == crate::menus::MenuId::EditorTitle && item.group == "navigation" && item.when.as_ref().is_none_or(|when| when.evaluate(&context))) {
            match &item.entry {
                crate::menus::Entry::Command { id, .. } => actions.push((item.order, self.command_button(id, item.title.clone(), resource.clone(), foreground, cx).into_any_element())),
                // layoutActions.ts: the only workbench submenu here, with its `Codicon.gear` icon.
                crate::menus::Entry::Submenu { submenu: submenu @ crate::menus::MenuId::NotebookEditorLayoutConfigure, .. } => {
                    actions.push((item.order, self.submenu_button(*submenu, "gear", item.title.clone(), foreground, cx).into_any_element()));
                }
                crate::menus::Entry::Submenu { .. } => {}
            }
        }
        actions.sort_by(|a, b| a.0.total_cmp(&b.0));
        actions.into_iter().map(|(_, element)| element).collect()
    }

    /// A title icon whose dropdown shows `submenu` below it (WorkbenchToolBar's `SubmenuItemAction` dropdown).
    fn submenu_button(&self, submenu: crate::menus::MenuId, icon: &str, title: String, foreground: Rgba, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let bounds = Rc::new(Cell::new(Bounds::default()));
        let measured = bounds.clone();
        self.icon(icon, 22., 16., foreground)
            .id(gpui::SharedString::from(format!("submenu-button-{submenu:?}")))
            .rounded(px(5.))
            .cursor_pointer()
            .hover(|icon| icon.bg(self.color("toolbar.hoverBackground")))
            .on_click(cx.listener(move |workbench, _: &gpui::ClickEvent, window, cx| {
                cx.stop_propagation();
                let context = workbench.key_context(window);
                let actions = crate::menus::menu_actions(&workbench.menus, submenu, &context);
                workbench.show_context_menu_actions(actions, &context, bounds.get(), Vec::new(), window, cx);
            }))
            .child(canvas(move |element_bounds, _, _| measured.set(element_bounds), |_, _, _, _| {}).absolute().size_full())
            .child(self.hover_target(gpui::SharedString::from(format!("submenu-button-hover-{submenu:?}")), super::hover::HoverContent::Text(title), super::hover::HoverOptions::mouse()))
    }

    /// `--modern-ui-connected-tab-strip-background` (connectedEditorTabs.ts): the connected tabs background,
    /// else the tabs background, else the editor's.
    pub(super) fn tab_strip_background(&self) -> Rgba {
        ["editorGroupHeader.connectedTabsBackground", "editorGroupHeader.tabsBackground", "editor.background"]
            .into_iter()
            .find_map(|key| self.theme.color(key))
            .expect("editor.background has a registry default")
    }

    /// `--modern-ui-connected-tab-border`: the header's tabs border; in high contrast the focus border of the active group.
    fn connected_tab_border(&self) -> Rgba {
        match self.theme.kind {
            ThemeKind::HighContrastDark | ThemeKind::HighContrastLight => self.color("focusBorder"),
            _ => ["editorGroupHeader.tabsBorder", "tab.border", "editor.background"]
                .into_iter()
                .find_map(|key| self.theme.color(key))
                .expect("editor.background has a registry default"),
        }
    }

    /// editorTabsControl.ts `getHoverTitle`: the long title, with a preview note on a preview editor's tab.
    fn tab_hover(&self, index: usize) -> impl IntoElement {
        self.editor_title_hover(("tab-hover", index), &self.editors.editors[index])
    }

    /// A tab's or an Open Editors row's hover: the editor's `getTitle(Verbosity.LONG)`.
    pub(super) fn editor_title_hover(&self, id: impl Into<gpui::ElementId>, editor: &OpenEditor) -> impl IntoElement {
        // `getTitle(Verbosity.LONG)`: a search editor's and a webview's is its name.
        let title = match (self.editors.diffs.get(&editor.model_id), editor.document().scheme.as_str()) {
            (Some(diff), _) => diff.title(|side| crate::quick_input::path_label(crate::editor::split_query(&side.scheme, &side.path).0, None)),
            (None, SEARCH_EDITOR_SCHEME | WEBVIEW_PANEL_SCHEME | super::extension_editor::EXTENSION_EDITOR_SCHEME | super::welcome_page::WALKTHROUGH_SCHEME | MULTI_DIFF_EDITOR_SCHEME) => self.editor_name(editor),
            // labels.ts: an untitled label's title is its name and its path.
            (None, "untitled") if !editor.text_model().associated_file => match self.untitled_description(editor) {
                Some(path) => format!("{} \u{2022} {path}", self.editor_name(editor)),
                None => self.editor_name(editor),
            },
            // `MergeEditorInput` keeps `getTitle` of its result.
            (None, _) => match self.editors.merges.get(&editor.model_id) {
                Some(merge) => crate::quick_input::path_label(&merge.result.path, None),
                None => crate::quick_input::path_label(&editor.document().path, None),
            },
        };
        let content = if editor.preview { super::hover::HoverContent::Preview(title) } else { super::hover::HoverContent::Text(title) };
        self.hover_target(id, content, super::hover::HoverOptions::mouse())
    }

    /// A diff editor's tab is its modified side's (`DiffEditorInput.isDirty`); an auto saved one shows no dirty mark.
    pub(super) fn tab_shows_dirty(&self, index: usize) -> bool {
        self.editor_shows_dirty(&self.editors.editors[index])
    }

    pub(super) fn editor_shows_dirty(&self, tab: &OpenEditor) -> bool {
        // A notebook tab is its notebook's working copy, not its focused cell's text model.
        if tab.document().scheme == crate::notebook::NOTEBOOK_EDITOR_SCHEME {
            return self.host.notebooks.model(&tab.document().path.to_string_lossy()).is_some_and(|model| model.dirty);
        }
        let model = self.editors.editor_model(tab);
        model.is_dirty() && !self.editor_is_saving(model)
    }

    /// A tab sized to its label (tab.sizing fit): 8px before the label, 30.5px after it for the close action.
    fn tab(&self, index: usize, active: bool, row: TabRow, cx: &mut Context<Self>) -> Div {
        let editor = &self.editors.editors[index];
        let upper = row == TabRow::Upper;
        // connectedEditorTabs.css: the upper bar's tabs are 28px high, their label and icon 2px from the top.
        let content_top = if upper { 2. } else { 3.5 };
        // multiEditorTabsControl.ts `getStickyTabWidth`: `workbench.editor.pinnedTabSizing` compact (28px with the
        // modern UI tabs, icon only, no actions) and shrink (80px, label cut) size the sticky tabs.
        let sticky_sizing = match self.setting("workbench.editor.pinnedTabSizing").as_str() {
            Some("compact") if editor.sticky => Some(28.),
            Some("shrink") if editor.sticky => Some(80.),
            _ => None,
        };
        let compact = sticky_sizing == Some(28.);
        let readonly = self.readonly(&editor.document().scheme, &editor.document().path).is_some();
        let action_foreground = if active { self.foreground_or("modernEditorTab.activeForeground") } else { self.foreground_or("tab.inactiveForeground") };
        // tabs.css colors `.tab-label:not(.monaco-decoration-itemColor)`: a decorated label (read-only, no color of its
        // own) inherits the tab's color from multieditortabscontrol.css, `tab.activeForeground` when active.
        let foreground = if active && readonly { self.foreground_or("tab.activeForeground") } else { action_foreground };
        // textFileService.ts decorations: a deleted file's label is struck through in `list.errorForeground`.
        let orphaned = editor.document().scheme == "file" && self.text_files.orphaned.contains(&editor.document().path);
        // multiEditorTabsControl.ts: the label takes the file's decorations per `workbench.editor.decorations`.
        let decoration = (editor.document().scheme == "file").then(|| self.file_decoration(&editor.document().path, false)).flatten();
        let decoration_color = self.decoration_color(decoration.as_ref(), self.setting("workbench.editor.decorations.colors") != json!(false));
        let badge = self.decoration_badge(decoration.as_ref(), foreground).filter(|_| self.setting("workbench.editor.decorations.badges") != json!(false));
        let label = div()
            .mt(px(content_top))
            .h(px(24.))
            .line_height(px(24.))
            .whitespace_nowrap()
            .flex()
            .items_baseline()
            .when(editor.preview, |label| label.italic())
            .when_some(decoration_color, |label, color| label.text_color(color))
            .when(orphaned, |label| label.line_through().text_color(self.foreground_or("list.errorForeground")))
            // A flex item's text never shrinks, so the cut name gets its own box for the ellipsis.
            .map(|label| match sticky_sizing {
                Some(_) => label.min_w_0().pr(px(5.)).child(div().min_w_0().overflow_hidden().text_ellipsis().child(self.editor_name(editor))),
                None => label.child(self.editor_name(editor)),
            })
            .children(self.untitled_description(editor).map(|description| {
                // iconlabel.css `.label-description`: 0.9em at 0.7 opacity (0.95 in light themes), 0.5em after the name.
                let opacity = if self.theme.kind == ThemeKind::Light { 0.95 } else { 0.7 };
                div().ml(px(6.5)).text_size(px(11.7)).opacity(opacity).child(description)
            }));
        let tab_scheme = self.editors.editors[index].document().scheme.clone();
        let file_icon = match matches!(tab_scheme.as_str(), crate::notebook::NOTEBOOK_EDITOR_SCHEME | super::welcome_page::WALKTHROUGH_SCHEME) {
            // A notebook tab's icon is its notebook file's, not the focused cell's; the playground's is its page's.
            true => self.document_icon(&self.editors.editors[index], 24., foreground),
            false => self.document_icon(self.editors.tab_model(index), 24., foreground),
        };
        // textFileService.ts decorations: a read-only file's label ends in the lockSmall badge at 0.75 opacity. The
        // iconlabel.css `::after` font-size 90% (11.7px) wins over the decoration rule's 16px, which loads first.
        let lock = readonly.then(|| self.icon("lock-small", 11.7, 11.7, foreground).mt(px(9.5)).ml(px(5.)).opacity(0.75));
        // A dirty tab shows a filled circle where the close action goes, also when inactive, and the action itself
        // while hovered (multieditortabscontrol.css `.dirty .action-label:not(:hover)`); a sticky tab always shows its
        // unpin action (`pinned`, `pinned-dirty` while dirty).
        let dirty = self.tab_shows_dirty(index);
        let sticky = editor.sticky;
        let action_icon = if sticky { "pinned" } else { "close-small" };
        let rest_icon = match (sticky, dirty) {
            (true, true) => "pinned-dirty",
            (false, true) => "circle-filled",
            _ => action_icon,
        };
        let action_group = gpui::SharedString::from(format!("tab-action-{index}"));
        let tab_group = gpui::SharedString::from(format!("tab-{index}"));
        let close = div()
            .id(("close", index))
            .group(action_group.clone())
            .relative()
            .rounded(px(4.))
            .hover(|action| action.bg(self.color("toolbar.hoverBackground")))
            .child(self.icon(rest_icon, 20., 16., action_foreground).group_hover(action_group.clone(), |icon| icon.invisible()))
            .child(self.icon(action_icon, 20., 16., action_foreground).absolute().top_0().left_0().invisible().group_hover(action_group, |icon| icon.visible()))
            .when(sticky, |close| close.child(self.hover_target(("tab-unpin-hover", index), super::hover::HoverContent::Text("Unpin Editor".into()), super::hover::HoverOptions::mouse())))
            .on_click(cx.listener(move |workbench, _, window, cx| {
                cx.stop_propagation();
                match sticky {
                    true => drop(workbench.execute_command("workbench.action.unpinEditor", vec![json!({ "editorIndex": index })], window, cx)),
                    false => drop(workbench.close_editors(vec![index], window, cx)),
                }
            }));
        let tab = div()
            .id(("tab", index))
            .relative()
            .h_full()
            .flex_none()
            .flex()
            .text_color(foreground)
            .when_some(sticky_sizing, |tab, width| tab.w(px(width)))
            .map(|tab| match (file_icon, compact) {
                (Some(icon), true) => tab.pl(px(5.)).child(icon.mt(px(content_top))),
                (None, true) => tab.px(px(4.)).child(label.flex_1().justify_center()),
                (Some(icon), false) => tab.pl(px(6.)).pr(px(30.5)).child(icon.mt(px(content_top))).child(label.ml(px(6.))),
                (None, false) => tab.pl(px(8.)).pr(px(30.5)).child(label),
            })
            .children(lock)
            // In a tab the badge sits right before the action's padding, without the list rows' 16px end margin.
            .children(badge.map(|badge| badge.mt(px(3.5)).mr(px(0.)).line_height(px(24.))))
            .map(|tab| self.tab_drag_and_drop(tab, self.editors.id, index, cx))
            .on_click(cx.listener(move |workbench, event: &gpui::ClickEvent, _, cx| {
                workbench.editors.activate(index);
                if event.click_count() == 2 {
                    workbench.editors.pin(index);
                }
                workbench.editors_changed(cx);
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| workbench.show_tab_context_menu(index, event.position, window, cx)))
            // multiEditorTabsControl.ts: a middle click closes the tab unless `preventPinnedEditorClose` keeps it.
            .on_mouse_up(MouseButton::Middle, cx.listener(move |workbench, _: &gpui::MouseUpEvent, window, cx| {
                if !workbench.prevents_mouse_close(&workbench.editors.editors[index]) {
                    drop(workbench.close_editors(vec![index], window, cx));
                }
            }))
            .child(self.tab_hover(index))
            // connectedEditorTabs.css: an inactive tab's close action is transparent until the tab is hovered.
            .group(tab_group.clone())
            .when(!compact, |tab| {
                let close_top = if upper { 4. } else { 5.5 };
                tab.child(div().absolute().top(px(close_top)).right(px(6.5)).when(!(active || dirty || sticky), |close| close.invisible().group_hover(tab_group, |close| close.visible())).child(close))
            });
        // multieditortabscontrol.css: tabs abut; only the last one keeps 8px after it, before the editor actions.
        if !active {
            return div().h_full().flex_none().child(tab);
        }
        // connectedEditorTabs.css: an active tab in the upper bar is not connected to the editor, only a 24px fill inset 2px.
        if upper {
            let fill = div().absolute().inset(px(2.)).rounded(px(4.)).bg(self.color("editor.background"));
            return div().h_full().flex_none().relative().child(fill).child(tab);
        }
        // The fill starts in the part's top border row; its caps and shoulders are cornerRadius-medium plus the stroke.
        let surface = self.color("editor.background");
        let border = self.connected_tab_border();
        let radius = px(7.);
        let shoulder = |piece: Div| piece.absolute().bottom_0().size(radius).bg(surface);
        let strip_corner = || div().size_full().bg(self.tab_strip_background()).border_b_1().border_color(border);
        div()
            .h_full()
            .flex_none()
            .relative()
            .child(div().absolute().top(px(-1.)).left_0().right_0().h(px(TAB_STRIP_HEIGHT + 1.)).bg(surface).border_t_1().border_r_1().rounded_t(radius).border_color(border))
            .when_some(self.theme.color("tab.activeBorder"), |tab, color| tab.child(div().absolute().bottom_0().left_0().right_0().h(px(1.)).bg(color)))
            // The first tab meets the part's own rounded corner, so it has no left shoulder.
            .when(row == TabRow::Main { first: false }, |tab| tab.child(shoulder(div().left(-radius)).child(strip_corner().border_r_1().rounded_br(radius))))
            .child(shoulder(div().left_full()).child(strip_corner().border_l_1().rounded_bl(radius)))
            .child(tab)
    }

    pub(super) fn document_icon(&self, editor: &OpenEditor, line_height: f32, foreground: Rgba) -> Option<Div> {
        let document = &editor.document();
        // `SearchEditorInput.getIcon`.
        if document.scheme == SEARCH_EDITOR_SCHEME {
            return Some(self.icon("search", 16., 16., foreground).h(px(line_height)));
        }
        // `MultiDiffEditorInput.getIcon`.
        if document.scheme == MULTI_DIFF_EDITOR_SCHEME {
            return Some(self.icon("diff-multiple", 16., 16., foreground).h(px(line_height)));
        }
        // `ReplEditorInput.getIcon`.
        if document.scheme == crate::notebook::NOTEBOOK_EDITOR_SCHEME && self.host.notebooks.interactives.iter().any(|interactive| interactive.repl && interactive.notebook == document.path.to_string_lossy()) {
            return Some(self.icon("debug-line-by-line", 16., 16., foreground).h(px(line_height)));
        }
        // walkThroughPart.css: the playground's `vs_code_editor_walkthrough.md` file icon is code-icon.svg.
        if document.scheme == super::welcome_page::WALKTHROUGH_SCHEME && self.is_playground(editor) {
            let icon = self.app_root.join("src/vs/workbench/browser/media/code-icon.svg");
            return Some(div().flex().items_center().h(px(line_height)).child(gpui::img(icon).size(px(16.))));
        }
        // `ExtensionsInput.getIcon`.
        if document.scheme == super::extension_editor::EXTENSION_EDITOR_SCHEME {
            return Some(self.icon("extensions", 16., 16., foreground).h(px(line_height)));
        }
        if let Some(icon) = self.webview_icon(&document.path) {
            return Some(div().flex().items_center().h(px(line_height)).child(gpui::img(icon.to_path_buf()).size(px(16.))));
        }
        // A custom editor or media preview tab is labeled by its resource (CustomEditorInput), with the file's icon.
        if let Some(resource) = self.webview_resource(&document.path) {
            return self.file_icons.as_ref()?.file_icon(&resource, self.languages.language_of(&resource, ""), self.theme.kind, 13., line_height, foreground);
        }
        self.file_icons.as_ref()?.file_icon(&document.path, &document.language_id, self.theme.kind, 13., line_height, foreground)
    }

    /// The file's path below its workspace folder, one item per segment (breadcrumbs.filePath relative), after the
    /// folder's name in a workspace (`BreadcrumbsModel._initFilePathInfo`).
    pub(super) fn breadcrumbs(&self, editor: &OpenEditor, cx: &mut Context<Self>) -> Div {
        let foreground = self.color("breadcrumb.foreground");
        // A notebook editor's breadcrumbs are its notebook's resource and the notebook outline (notebookOutline.ts).
        let is_notebook = matches!(editor.document().scheme.as_str(), crate::notebook::CELL_SCHEME | crate::notebook::NOTEBOOK_EDITOR_SCHEME);
        let (notebook_path, notebook_items) = self.notebook_breadcrumbs(foreground, BREADCRUMBS_HEIGHT).filter(|_| is_notebook).unzip();
        let document = editor.document();
        // A custom editor's breadcrumbs are its resource's (`EditorResourceAccessor.getOriginalUri`).
        let custom_resource = self.webview_resource(&document.path);
        let path = notebook_path.as_ref().or(custom_resource.as_ref()).unwrap_or(&document.path);
        let workspace = self.workspace();
        let folder = workspace.and_then(|workspace| workspace.folder_of(path));
        let home = std::env::home_dir().filter(|home| folder.is_none() && path.starts_with(home));
        // breadcrumbsModel.ts `_initFilePathInfo`: the path below the workspace folder, else below the user home as
        // `~` (`getUriHome`), else below the root, which has no item.
        let (relative, root): (PathBuf, Option<String>) = match (folder, home) {
            (Some(folder), _) => (path.iter().skip(folder.path.iter().count()).collect(), None),
            (None, Some(home)) => (path.iter().skip(home.iter().count()).collect(), Some("~".into())),
            (None, None) => (path.iter().skip(1).collect(), None),
        };
        let mut segments: Vec<String> = root.into_iter().chain(relative.iter().map(|segment| segment.to_string_lossy().into_owned())).collect();
        if let Some(folder) = folder.filter(|_| workspace.is_some_and(|workspace| workspace.file.is_some())) {
            segments.insert(0, folder.name.clone());
        }
        let file = segments.len() - 1;
        let symbol_items: Vec<(Option<Div>, String)> = match notebook_items {
            Some(items) => items,
            None => self
                .breadcrumb_symbols(editor)
                .into_iter()
                .map(|(kind, name)| {
                    let codicon = kind.map(crate::symbol_icons::codicon);
                    let color = codicon.and_then(crate::symbol_icons::color_key).and_then(|key| self.theme.color(key)).unwrap_or(foreground);
                    let icon = codicon.filter(|_| self.setting("breadcrumbs.icons") != false).map(|codicon| self.icon(codicon, 16., 16., color));
                    (icon, name)
                })
                .collect(),
        };
        let last = file + symbol_items.len();
        let file_items = segments.into_iter().enumerate().map(|(index, segment)| {
            // The 16px before each item is the item's own ::before; folders have no icon in Seti, the file does.
            let file_icon = match (index == file, &notebook_path) {
                (false, _) => None,
                (true, Some(path)) => self.file_icons.as_ref().and_then(|icons| icons.file_icon(path, self.languages.language_of(path, ""), self.theme.kind, 13., BREADCRUMBS_HEIGHT, foreground)),
                (true, None) => self.document_icon(editor, BREADCRUMBS_HEIGHT, foreground),
            };
            (file_icon, segment)
        });
        // breadcrumbsWidget.css: an item is its icon, its label and the 16px separator codicon, without gaps; the
        // first one starts 16px in, measured against the reference with ui-pair.
        let items = file_items.chain(symbol_items).enumerate().map(|(index, (icon, segment))| {
            div()
                .flex()
                .items_center()
                .when(index == 0, |item| item.pl(px(16.)))
                .whitespace_nowrap()
                .when_some(icon, |item, icon| item.child(icon.mr(px(6.))))
                .child(segment)
                .when(index < last, |item| item.child(self.icon("chevron-right", 16., 16., foreground)))
        });
        // editorGroupView.ts: the title's bottom border, drawn over the breadcrumbs' last row.
        let title_border = ["editorGroupHeader.border", "contrastBorder"].into_iter().find_map(|key| self.theme.color(key));
        let widget = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_hidden()
            // The breadcrumbs sit in the title (editortabscontrol.css) and each item is pointer (breadcrumbsWidget.css).
            .cursor_pointer()
            .flex()
            // breadcrumbsWidget.ts `revealLast`: a path wider than the widget shows its end; the row is at least as
            // wide as the widget, so a short one starts at the left.
            .justify_end()
            .child(div().flex_none().min_w_full().flex().children(items));
        div()
            .relative()
            .h(px(BREADCRUMBS_HEIGHT))
            .flex_none()
            .overflow_hidden()
            .flex()
            .bg(self.color("breadcrumb.background"))
            .text_color(foreground)
            .child(widget)
            .children(self.editor_type_picker(cx))
            .when_some(title_border, |crumbs, color| crumbs.child(div().absolute().bottom_0().left_0().right_0().h(px(1.)).bg(color)))
    }

    pub(super) fn editor_metrics(&self, editor: &OpenEditor, window: &mut Window) -> EditorMetrics {
        let mut metrics = EditorMetrics::measure(window, editor.document().lines.len(), self.line_numbers(editor), self.editor_zoom.multiplier());
        // diffEditorEditors.ts `_adjustOptionsForSubEditor`: no folding and no minimap in either side.
        if self.editors.active_diff().is_some() {
            metrics.decorations_width = diff_editor_view::diff_decorations_width(metrics.char_width);
        } else if self.is_interactive_input(editor) {
            // simpleEditorOptions.ts: lineDecorationsWidth 0 and no folding.
            metrics.decorations_width = 0.;
        } else if editor.document().scheme == crate::notebook::CELL_SCHEME {
            // cellEditorOptions.ts: lineDecorationsWidth 0, only the folding controls.
            metrics.decorations_width = DECORATIONS_WIDTH - 10.;
        } else if self.editor_has_comment_threads(editor) {
            metrics.decorations_width += comment_glyph::RESERVED_WIDTH;
        }
        // walkThroughPart.ts `getEditorOptions`: lineNumbersMinChars 1.
        if editor.document().scheme == super::walkthrough_part::SNIPPET_SCHEME && metrics.line_numbers != LineNumbers::Off {
            let digits = editor.document().lines.len().to_string().len();
            metrics.line_numbers_width = (metrics.line_numbers_width / LINE_NUMBERS_MIN_CHARS.max(digits) as f32 * digits as f32).round();
        }
        metrics
    }

    /// contextmenu.ts `_doShowContextMenu` without a mouse event: below the cursor (`getScrolledVisiblePosition`).
    pub(super) fn cursor_anchor(&self, window: &mut Window) -> Option<gpui::Point<Pixels>> {
        let editor = self.editors.active()?;
        let metrics = self.editor_metrics(editor, window);
        let head = editor.selection().head;
        let viewport = self.editors.viewport;
        let view_line = editor.head_view_line(0);
        let left = metrics.content_left() + editor.segment_visible_column(editor.segment(view_line), head.column) * metrics.char_width - editor.scroll.left;
        let top = editor.line_top(view_line, viewport) - editor.scroll.top + metrics.line_height;
        let origin = self.text_area_bounds.get().origin;
        Some(point(px(origin.x + left), px(origin.y + top)))
    }

    /// `editor.lineNumbers` of `editor`.
    pub(super) fn line_numbers(&self, editor: &OpenEditor) -> LineNumbers {
        let overrides = config::configuration::Overrides { resource: Some(&editor.document().path), override_identifier: Some(&editor.document().language_id) };
        let setting = self.settings.lock().unwrap().get_value(Some("editor.lineNumbers"), &overrides);
        match setting.as_ref().and_then(|value| value.as_str()) {
            // outputView.ts `OutputEditor.getConfigurationOverrides`: output editors hide line numbers.
            _ if editor.document().scheme == crate::output::OUTPUT_SCHEME => LineNumbers::Off,
            _ if self.zen_mode_hides_line_numbers() => LineNumbers::Off,
            // simpleEditorOptions.ts: an interactive window's or REPL editor's input has none.
            _ if self.is_interactive_input(editor) => LineNumbers::Off,
            // cellEditorOptions.ts: a cell editor's line numbers follow its own `lineNumbers`, else `notebook.lineNumbers`.
            _ if editor.document().scheme == crate::notebook::CELL_SCHEME => match self.cell_line_numbers(&editor.document().uri_string()) {
                true => LineNumbers::On,
                false => LineNumbers::Off,
            },
            Some("off") => LineNumbers::Off,
            // gotoLineQuickAccess.ts: relative numbers switch to "on" while the picker previews in this editor.
            Some("relative") if self.quick_access.goto_line.as_ref().is_some_and(|state| state.previews(editor.model_id)) => LineNumbers::On,
            Some("relative") => LineNumbers::Relative,
            Some("interval") => LineNumbers::Interval,
            _ => LineNumbers::On,
        }
    }

    /// One of several live code editors stacked in one scrolling view (`EditorGroup::stacked_editors`, a notebook's
    /// cells): the group's code editor for the model it edits, the lines of the others; sized by its caller.
    pub(super) fn embedded_code_editor(&self, editor: &OpenEditor, window: &mut Window, cx: &mut Context<Self>) -> Div {
        if self.editors.active().is_some_and(|active| std::ptr::eq(active, editor)) {
            return div().flex_1().flex().child(self.text_area(editor, window, cx));
        }
        let metrics = self.editor_metrics(editor, window);
        let rows = self.editor_lines(editor, 0..editor.view_line_count(), metrics, window, cx);
        div().flex_1().flex().flex_col().font_family(EDITOR_FONT).text_size(px(metrics.font_size)).line_height(px(metrics.line_height)).children(rows)
    }

    pub(super) fn text_area(&self, editor: &OpenEditor, window: &mut Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let metrics = self.editor_metrics(editor, window);
        let in_diff = self.editors.active_diff().is_some();

        let area_left = self.text_area_left.clone();
        let area_bounds = self.text_area_bounds.clone();
        let focus = self.editor_focus.clone();
        let workbench = cx.entity();
        let laid_out = cx.entity();
        let view_line_count = editor.view_line_count();
        let sticky_scroll_lines = self.sticky_scroll_lines();
        let top_whitespace = self.editors.viewport.top_whitespace;
        let stop_rendering_line_after = self.stop_rendering_line_after;
        let minimap_settings = self.minimap_settings();
        let in_cell = editor.document().scheme == crate::notebook::CELL_SCHEME || self.is_interactive_input(editor);
        // cellEditorOptions.ts and simpleEditorOptions.ts: no minimap and no overview ruler lanes in a cell or input editor.
        // walkThroughPart.ts `getEditorOptions`: no minimap in the playground's editors either.
        let in_snippet = editor.document().scheme == super::walkthrough_part::SNIPPET_SCHEME;
        let minimap_enabled = !in_diff && !in_cell && !in_snippet && minimap_settings.enabled;
        let viewport = self.editors.viewport;
        let lines = editor.visible_lines(viewport);
        let zones = editor.zones_between(lines.clone());
        let zones_above: f32 = zones.iter().filter(|zone| zone.0 == lines.start).map(|zone| zone.1).sum();
        let lines_top = editor.line_top(lines.start, viewport) - zones_above - editor.scroll.top;
        let first_line = lines.start;
        let mut rows = Vec::new();
        for (index, row) in self.editor_lines(editor, lines, metrics, window, cx).into_iter().enumerate() {
            rows.extend(zones.iter().filter(|zone| zone.0 == first_line + index).map(|zone| self.view_zone_body(editor, zone.2, zone.1, metrics.content_left(), cx)));
            rows.push(row);
        }
        let minimap_width = self.minimap.info.as_ref().map_or(0., |(info, _)| info.width as f32);
        let margin_shift = self.minimap.info.as_ref().map_or(0., |(info, _)| info.margin_shift() as f32);
        let shifted = || div().absolute().top_0().bottom_0().left(px(margin_shift)).right_0();
        let now = Instant::now();
        if self.scrollbars.in_transition(now) || self.fold_controls.in_transition(now) || self.minimap.in_transition(now) {
            window.request_animation_frame();
        }
        let editor_origin = self.text_area_bounds.get().origin;
        let middle_scroll_dot = self.middle_scroll_dot(point(px(editor_origin.x), px(editor_origin.y)));
        let middle_scrolling = middle_scroll_dot.is_some();
        let scrolled = cx.entity();
        let group = self.editors.id;
        let inactive = self.editor_groups.drawing_inactive;
        self.editor_accessibility(div().id("text-area"), editor, window)
            .flex_1()
            .relative()
            .overflow_hidden()
            .font_family(EDITOR_FONT)
            .text_size(px(metrics.font_size))
            .line_height(px(metrics.line_height))
            .when(!inactive, |area| area.track_focus(&self.editor_focus))
            .on_key_down(cx.listener(Self::editor_key_down))
            .on_key_up(cx.listener(Self::inspect_tokens_key_up))
            .on_scroll_wheel(cx.listener(Self::editor_wheel))
            .on_drag_move(cx.listener(|workbench, event: &DragMoveEvent<ExternalPaths>, _, cx| {
                if !event.bounds.contains(&event.event.position) {
                    workbench.drag_files_over_text(None, false, cx);
                }
            }))
            .on_modifiers_changed(cx.listener(Self::sticky_modifiers_changed))
            .on_modifiers_changed(cx.listener(|workbench, event: &gpui::ModifiersChangedEvent, _, cx| {
                workbench.text_drag_modifiers_changed(event.modifiers, cx);
                workbench.content_hover_modifiers_changed(event.modifiers, cx);
                workbench.links_modifiers_changed(event.modifiers, cx);
                workbench.definition_link_modifiers_changed(event.modifiers, cx);
                workbench.inlay_hints_modifiers_changed(event.modifiers, cx);
            }))
            .on_hover(cx.listener(|workbench, hovered: &bool, window, cx| {
                workbench.scrollbars.mouse_over = *hovered;
                match hovered {
                    true => workbench.reveal_scrollbars(cx),
                    false => {
                        workbench.hide_scrollbars(cx);
                        workbench.content_hover_mouse_left(window.mouse_position(), cx);
                        workbench.definition_link_mouse_left();
                        workbench.commenting_hover_moved(None, cx);
                    }
                }
            }))
            // The line's row sets `row_target` first: moves bubble from the row up to the text area.
            .on_mouse_move(cx.listener(|workbench, event: &MouseMoveEvent, _, cx| {
                let row_target = workbench.content_hover.row_target.take();
                workbench.links_mouse_moved(row_target, event.modifiers, cx);
                let target = match row_target {
                    Some((line, visible)) => super::content_hover::MouseTarget::Text { line, visible },
                    None => super::content_hover::MouseTarget::Other,
                };
                workbench.content_hover_mouse_moved(target, event.position, event.modifiers, event.pressed_button.is_some(), cx);
            }))
            .capture_any_mouse_down(cx.listener(|workbench, event: &MouseDownEvent, _, cx| {
                workbench.hide_content_hover(cx);
                if workbench.middle_scroll_mouse_down(event.button == MouseButton::Middle, event.position, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_any_mouse_up(cx.listener(|workbench, _, _, cx| workbench.middle_scroll_mouse_up(cx)))
            .capture_key_down(cx.listener(|workbench, event: &gpui::KeyDownEvent, _, cx| {
                workbench.definition_link_key_down(event.keystroke.modifiers, cx);
                workbench.end_middle_scroll(cx);
                workbench.cancel_text_drag(cx);
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|workbench, _, _, cx| {
                workbench.selecting_text = false;
                workbench.line_select_start = None;
                workbench.cancel_text_drag(cx);
            }))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|workbench, _, _, cx| {
                workbench.selecting_text = false;
                workbench.line_select_start = None;
                workbench.cancel_text_drag(cx);
            }))
            .child(
                canvas(
                    move |bounds, window, cx| {
                        let (width, height) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                        let pixel_ratio = window.scale_factor();
                        let minimap = minimap_enabled.then(|| minimap_info(width, height, metrics.content_left(), metrics.char_width, metrics.line_height, view_line_count, pixel_ratio, minimap_settings.layout));
                        // The margin and content right of a left minimap: pointer positions count from there.
                        let shift = px(minimap.as_ref().map_or(0., |minimap| minimap.margin_shift() as f32));
                        area_left.set(f32::from(bounds.left() + shift));
                        area_bounds.set(Bounds::from_corners(point(bounds.left() + shift, bounds.top()), bounds.bottom_right()).map(f32::from));
                        // editorOptions.ts: contentWidth = remainingWidth - minimapWidth.
                        let minimap_width = minimap.as_ref().map_or(0., |minimap| minimap.width as f32);
                        let viewport = Viewport { width: width - metrics.content_left() - minimap_width, height, char_width: metrics.char_width, line_height: metrics.line_height, sticky_scroll_lines, top_whitespace, stop_rendering_line_after };
                        let minimap = minimap.map(|minimap| (minimap, pixel_ratio as f64));
                        laid_out.update(cx, |workbench, cx| workbench.group_laid_out(group, |workbench| workbench.editor_laid_out(viewport, minimap, cx)));
                    },
                    move |bounds, _, window, cx| {
                        if !inactive {
                            window.handle_input(&focus, ElementInputHandler::new(bounds, workbench), cx);
                        }
                        if middle_scrolling && scrolled.update(cx, |workbench, cx| workbench.middle_scroll_frame(cx)) {
                            // observeWindowMousePos: the pointer anywhere in the window steers the session.
                            window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                                if phase == DispatchPhase::Bubble {
                                    scrolled.update(cx, |workbench, _| workbench.middle_scroll_mouse_moved(event.position));
                                }
                            });
                            window.request_animation_frame();
                        }
                    },
                )
                .absolute()
                .inset_0(),
            )
            .child(
                shifted()
                    // .view-lines is monaco-mouse-cursor-text; the line numbers, glyph margin and the overview ruler over the
                    // vertical scrollbar keep the default cursor.
                    .child(div().debug_selector(|| "editor-view-lines".into()).absolute().top_0().bottom_0().left(px(metrics.content_left())).right(px(OVERVIEW_RULER_WIDTH)).cursor(self.text_cursor_style()).on_mouse_down(
                        MouseButton::Right,
                        // Below the last line (MouseTargetType.CONTENT_EMPTY): the position is the end of the last line.
                        cx.listener(|workbench, event: &MouseDownEvent, window, cx| {
                            let last = workbench.editors.active().expect("the text area shows an editor").document().lines.len() - 1;
                            workbench.show_editor_context_menu(last, f32::MAX, event.position, window, cx);
                        }),
                    ).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|workbench, event: &MouseDownEvent, window, cx| {
                            window.focus(&workbench.editor_focus, cx);
                            let last = workbench.editors.active().expect("the text area shows an editor").document().lines.len() - 1;
                            workbench.selecting_text = true;
                            workbench.click_text(last, f32::MAX, event.modifiers, event.modifiers.shift, event.click_count, cx);
                        }),
                    ))
                    // editorScrollbar.ts clips the lines at contentWidth: the column right of the minimap stays empty.
                    .child(div().absolute().left_0().right(px(minimap_width - margin_shift)).top_0().bottom_0().overflow_hidden().child(div().absolute().left_0().right_0().top(px(lines_top)).children(rows)))
                    .children(self.marker_navigation_widget(editor, metrics.content_left(), metrics.char_width, cx))
                    .children(self.quick_diff_widget(editor, metrics, window, cx))
                    .children(self.references_peek_widget(editor, metrics.content_left(), metrics.char_width, window, cx))
                    .children(self.hierarchy_peek_widget(editor, metrics.content_left(), metrics.char_width, window, cx))
                    .child(
                        div().id("margin").absolute().top_0().bottom_0().left_0().w(px(metrics.content_left())).on_hover(cx.listener(|workbench, hovered: &bool, _, cx| {
                            let animate = !workbench.is_motion_reduced();
                            workbench.fold_controls.set_shown(*hovered, Instant::now(), animate);
                            cx.notify();
                        })),
                    )
                    .when(!in_cell, |area| area.child(self.overview_ruler(editor).absolute().top_0().right_0().h_full()))
                    .child(self.scrollbars_canvas(editor, metrics, cx).absolute().inset_0())
                    // contextmenu.ts: a right press on the scrollbar (MouseTargetType.SCROLLBAR) shows the minimap menu.
                    .child(div().absolute().top_0().bottom_0().right_0().w(px(VERTICAL_SCROLLBAR_WIDTH)).on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|workbench, event: &MouseDownEvent, window, cx| workbench.show_scrollbar_context_menu(event.position, window, cx)),
                    )),
            )
            .when(editor.scroll.top > 0., |area| area.child(div().absolute().top_0().left_0().right(px(VERTICAL_SCROLLBAR_WIDTH)).h(px(SCROLL_SHADOW_HEIGHT)).cursor_default().child(self.scroll_shadow().size_full())))
            .children(self.minimap(editor, cx))
            .child(
                shifted()
                    .children(middle_scroll_dot)
                    .children(self.empty_editor_hint(editor, metrics.content_left(), cx))
                    .children(self.sticky_scroll_widget(editor, metrics, window, cx))
                    // Messages, hovers and the overlay widgets are the focused editor's.
                    .when(!inactive, |shifted| {
                        shifted
                            .children(self.editor_message_widget(editor, metrics.content_left(), metrics.char_width, window, cx))
                            .children(self.content_hover_widget(editor, metrics.content_left(), metrics.char_width, window, cx))
                            .children(self.standalone_color_picker_widget(editor, metrics.content_left(), metrics.char_width, cx))
                            .children(self.ghost_text_lines(editor, metrics.content_left()))
                            .children(self.inspect_tokens_widget(editor, metrics.content_left(), metrics.char_width, window))
                    }),
            )
            // Overlay widgets: placed from the editor's right edge whichever side the minimap is on.
            .when(!inactive, |area| {
                area.children(self.find_widget(self.editors.viewport.width + metrics.content_left() + minimap_width, metrics.content_left(), minimap_width, window, cx))
                    .children(self.parameter_hints_widget(editor, metrics.content_left(), metrics.char_width, window, cx))
                    .children(self.lightbulb_widget(editor, metrics.content_left(), metrics.char_width, cx))
                    .children(self.paste_widget(editor, metrics.content_left(), metrics.char_width, cx))
                    .children(self.inline_progress_widget(editor, metrics.content_left(), metrics.char_width, cx))
                    .children(self.code_lens_widgets(editor, metrics.content_left(), metrics.char_width, cx))
                    .children(self.comment_thread_widgets(editor, metrics.content_left(), metrics.char_width, window, cx))
                    .children(self.rename_widget(editor, metrics.content_left(), metrics.char_width, window, cx))
                    .children(self.suggest_widget(editor, metrics.content_left(), metrics.char_width, window, cx))
                    .children(self.find_options_widget(minimap_width, cx))
                    .children(self.floating_menu(editor, viewport.height, minimap_width, window, cx))
                    .children(self.whitespace_hint_widget(minimap_width, cx))
                    .children(self.define_keybinding_overlay(editor, self.editors.viewport.width + metrics.content_left() + minimap_width, viewport.height, window, cx))
                    .children(self.breakpoint_widget_element(editor, metrics.content_left(), window, cx))
                    .children(self.exception_widget_element(editor, metrics.content_left(), cx))
            })
            .children(self.editor_inset_elements(editor, metrics.content_left()))
    }

    /// wordHighlighter.ts: `onDidBlurEditorWidget` stops the highlights once no code editor has the focus,
    /// `onDidFocusEditorText` runs the query from the selection again. The find widget, sticky lines and the
    /// unicode banner are inside the widget.
    pub fn watch_editor_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focus = self.editor_focus.clone();
        let blurred_at = Rc::new(Cell::new(None));
        let deactivated = Rc::new(Cell::new(false));
        let blur = cx.on_focus_out(&focus, window, {
            let (blurred_at, deactivated) = (blurred_at.clone(), deactivated.clone());
            move |workbench, _, window, cx| {
                // GPUI reports a window deactivation as focus out; the DOM keeps the focus there, so the
                // reactivation is no focus event either.
                if workbench.editor_focus.contains_focused(window, cx) {
                    return deactivated.set(true);
                }
                workbench.editor_message_text_blurred(cx);
                if !(workbench.unicode_banner_focus.is_focused(window) && workbench.unicode_banner_shown()) {
                    workbench.editor_widget_blurred(&blurred_at, cx);
                }
            }
        });
        // The banner holding the focus goes when its action disables the highlights: the focus leaves the editor.
        let banner_blur = cx.on_blur(&self.unicode_banner_focus, window, {
            let blurred_at = blurred_at.clone();
            move |workbench, window, cx| {
                if window.is_window_active() && !workbench.editor_focus.contains_focused(window, cx) {
                    workbench.editor_widget_blurred(&blurred_at, cx);
                }
            }
        });
        let text_blur = cx.on_blur(&focus, window, |workbench, window, cx| {
            if window.is_window_active() {
                workbench.multicursor_text_focus_changed(false, window, cx);
            }
        });
        let text_focus = cx.on_focus(&focus, window, move |workbench, window, cx| {
            let Some(editor) = workbench.editors.active_mut() else { return };
            // A picked command runs before GPUI reports the restored focus; upstream's delayed query, started on
            // focus, is cancelled by that command's edit (`onDidChangeModelContent` stops it).
            let returned = blurred_at.take().is_none_or(|(model, version)| model != editor.model_id || version == editor.text_model().version_id);
            let reactivated = deactivated.take();
            if returned && !reactivated {
                editor.refresh_highlights();
            }
            if !reactivated {
                workbench.multicursor_text_focus_changed(true, window, cx);
            }
            cx.notify();
        });
        let window_deactivated = cx.observe_window_activation(window, |workbench, window, cx| {
            if !window.is_window_active() {
                workbench.window_lost_focus_auto_save(cx);
            }
        });
        self.editor_focus_listeners = vec![blur, banner_blur, text_blur, text_focus, window_deactivated];
    }

    fn editor_widget_blurred(&mut self, blurred_at: &Cell<Option<(u32, u32)>>, cx: &mut Context<Self>) {
        self.text_drag = None;
        if let Some(editor) = self.editors.active_mut() {
            editor.highlights.clear();
            blurred_at.set(Some((editor.model_id, editor.text_model().version_id)));
        }
        self.editor_blurred_auto_save(cx);
        self.suggest_editor_blurred(cx);
        self.cancel_parameter_hints(cx);
        self.code_actions_editor_blurred(cx);
        self.rename_editor_blurred();
        cx.notify();
    }

    /// The text area got its size: the group's viewport follows it, which can clamp the scroll position and
    /// change the visible lines; the lines drawn widen the scrollable content (`ViewLines._ensureMaxLineWidth`).
    pub(super) fn editor_laid_out(&mut self, viewport: Viewport, minimap: Option<(MinimapLayoutInfo, f64)>, cx: &mut Context<Self>) {
        let resized = self.editors.viewport != viewport;
        let font_changed = (self.editors.viewport.char_width, self.editors.viewport.line_height) != (viewport.char_width, viewport.line_height);
        let minimap_changed = self.minimap.info != minimap;
        self.editors.viewport = viewport;
        self.minimap.info = minimap;
        let reveal = self.editors.reveal_after_layout.take();
        let rewrapped = self.configure_word_wrap();
        let editor = self.editors.active_mut().expect("laid out only with an editor open");
        if let Some(line) = reveal {
            editor.reveal_range(line, line, VerticalReveal::CenterIfOutsideViewport, viewport);
        }
        let scroll_width = editor.scroll_width(viewport);
        let visible_lines = editor.visible_lines(viewport);
        // viewLines.ts `_updateLineWidths`: with every line drawn, their widest is the content width.
        if font_changed || visible_lines == (0..editor.view_line_count()) {
            editor.forget_line_widths();
        }
        for view_line in visible_lines {
            let document = editor.document();
            let text = &document.lines[editor.model_line(view_line)];
            let tab_size = editor.document().indentation.tab_size;
            let width = match line_overflow(text, self.stop_rendering_line_after) {
                _ if editor.wraps() => editor.wrapped_line_width(view_line).unwrap_or_default() * viewport.char_width,
                None => editor.line_layout(editor.model_line(view_line)).width() * viewport.char_width,
                Some((limit, _, label)) => visible_column(text, limit, tab_size) as f32 * viewport.char_width + show_more_width(&label, viewport.char_width),
            };
            drop(document);
            editor.saw_line_width(width);
        }
        let moved = editor.set_scroll(editor.scroll.top, editor.scroll.left, viewport);
        let widened = editor.scroll_width(viewport) != scroll_width;
        if resized || moved {
            self.host.send_visible_ranges(&self.editors);
        }
        if resized || moved || widened || minimap_changed || rewrapped {
            cx.notify();
        }
    }

    /// The wheel scrolls the editor in `StandardWheelEvent` units: 40 browser pixels or one line per notch. With
    /// `editor.mouseWheelZoom`, Cmd or Ctrl without Shift and Alt zooms instead (mouseHandler.ts).
    pub(super) fn editor_wheel(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let (x, y) = match event.delta {
            ScrollDelta::Pixels(delta) => (f32::from(delta.x) / 40., f32::from(delta.y) / 40.),
            ScrollDelta::Lines(delta) => (delta.x, delta.y),
        };
        if self.setting("editor.mouseWheelZoom").as_bool().unwrap_or(false) {
            let modifiers = event.modifiers;
            let has_zoom_modifiers = (modifiers.platform || modifiers.control) && !modifiers.shift && !modifiers.alt;
            // ScrollDelta::Lines is AppKit's imprecise delta of a mouse wheel, `isPhysicalMouseWheel` upstream.
            let physical_wheel = matches!(event.delta, ScrollDelta::Lines(_));
            let level = self.editor_zoom.level();
            if self.editor_zoom.wheel(y, physical_wheel, has_zoom_modifiers, Instant::now()) {
                cx.stop_propagation();
                if self.editor_zoom.level() != level {
                    cx.notify();
                }
                return;
            }
        }
        let fast = event.modifiers.alt;
        let scrolled = match event.delta {
            // scrollableElement.ts: only a physical mouse wheel scrolls smoothly.
            ScrollDelta::Lines(_) => self.scroll_smoothly(cx, |editor, viewport| {
                editor.scroll_by_wheel(x, y, fast, viewport);
            }),
            ScrollDelta::Pixels(_) => {
                let viewport = self.editors.viewport;
                self.editors.active_mut().expect("the text area scrolls only with an editor open").scroll_by_wheel(x, y, fast, viewport)
            }
        };
        if scrolled {
            cx.stop_propagation();
            self.editor_scrolled(cx);
        }
    }

    /// scrollableElement.ts `_onDidScroll`: the scrollbars show, and the host hears of new visible lines.
    pub(super) fn editor_scrolled(&mut self, cx: &mut Context<Self>) {
        self.host.send_visible_ranges(&self.editors);
        self.reveal_scrollbars(cx);
        self.minimap_scrolled(cx);
        self.code_lens_scrolled(cx);
        self.inlay_hints_scrolled(cx);
        cx.notify();
    }

    /// `_reveal`: the scrollbars fade in, and fade out 500ms later unless the pointer is over the editor.
    fn reveal_scrollbars(&mut self, cx: &mut Context<Self>) {
        self.scrollbars.set_shown(true, Instant::now());
        self.scrollbars.hide_timer = None;
        if !self.scrollbars.mouse_over && self.scrollbars.drag.is_none() {
            self.scrollbars.hide_timer = Some(cx.spawn(async move |workbench, cx| {
                cx.background_executor().timer(SCROLLBAR_HIDE_TIMEOUT).await;
                workbench.update(cx, |workbench, cx| workbench.hide_scrollbars(cx)).ok();
            }));
        }
        cx.notify();
    }

    /// `_hide`: kept while the pointer is over the editor or drags a slider.
    fn hide_scrollbars(&mut self, cx: &mut Context<Self>) {
        if !self.scrollbars.mouse_over && self.scrollbars.drag.is_none() {
            self.scrollbars.set_shown(false, Instant::now());
            cx.notify();
        }
    }

    /// The sliders over the overview ruler and along the bottom (scrollbars.css, abstractScrollbar.ts): a press
    /// on a track centers the slider on the pointer, then the slider follows the pointer until release.
    fn scrollbars_canvas(&self, editor: &OpenEditor, metrics: EditorMetrics, cx: &mut Context<Self>) -> gpui::Canvas<Vec<(Hitbox, Bounds<Pixels>, Axis)>> {
        let viewport = self.editors.viewport;
        let bars = [(Axis::Vertical, editor.vertical_scrollbar(viewport)), (Axis::Horizontal, editor.horizontal_scrollbar(viewport))];
        let opacity = self.scrollbars.opacity(Instant::now());
        let shown = self.scrollbars.shown;
        let drag = self.scrollbars.drag;
        let faded = |key: &str| {
            let color = self.color(key);
            Rgba { a: color.a * opacity, ..color }
        };
        let colors = [faded("scrollbarSlider.background"), faded("scrollbarSlider.hoverBackground"), faded("scrollbarSlider.activeBackground")];
        let workbench = cx.entity();
        canvas(
            move |bounds, window, _| {
                let mut tracks = Vec::new();
                for (axis, bar) in bars {
                    let track = match axis {
                        Axis::Vertical => Bounds::new(point(bounds.right() - px(VERTICAL_SCROLLBAR_WIDTH), bounds.top()), size(px(VERTICAL_SCROLLBAR_WIDTH), px(bar.track))),
                        Axis::Horizontal => Bounds::new(point(bounds.left() + px(metrics.content_left()), bounds.bottom() - px(HORIZONTAL_SCROLLBAR_HEIGHT)), size(px(bar.track), px(HORIZONTAL_SCROLLBAR_HEIGHT))),
                    };
                    // An invisible scrollbar takes no pointer events (.invisible { pointer-events: none }).
                    if bar.needed && shown {
                        tracks.push((window.insert_hitbox(track, HitboxBehavior::BlockMouse), track, axis));
                    }
                }
                tracks
            },
            move |_, tracks, window, _| {
                let mut sliders = Vec::new();
                for (axis, bar) in bars {
                    let Some((_, track, _)) = tracks.iter().find(|(_, _, track_axis)| *track_axis == axis) else { continue };
                    let slider = slider_bounds(*track, axis, bar);
                    let color = match drag {
                        Some(drag) if drag.axis == axis => colors[2],
                        _ if slider.contains(&window.mouse_position()) => colors[1],
                        _ => colors[0],
                    };
                    window.paint_quad(fill(slider, color));
                    sliders.push(slider);
                }
                let hovered_slider = move |window: &Window| sliders.iter().any(|slider| slider.contains(&window.mouse_position()));
                let painted_hover = hovered_slider(window);
                let workbench_moved = workbench.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble {
                        return;
                    }
                    if let Some(drag) = drag {
                        let along = if drag.axis == Axis::Vertical { event.position.y } else { event.position.x };
                        workbench_moved.update(cx, |workbench, cx| workbench.drag_slider(drag, f32::from(along), cx));
                    } else if hovered_slider(window) != painted_hover {
                        window.refresh();
                    }
                });
                let workbench_released = workbench.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble && event.button == MouseButton::Left && drag.is_some() {
                        workbench_released.update(cx, |workbench, cx| {
                            workbench.scrollbars.drag = None;
                            workbench.hide_scrollbars(cx);
                        });
                    }
                });
                let workbench = workbench.clone();
                window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble {
                        return;
                    }
                    let Some((_, track, axis)) = tracks.iter().find(|(hitbox, _, _)| hitbox.is_hovered(window)) else { return };
                    let (track, axis) = (*track, *axis);
                    // The vertical scrollbar's column shows the menu itself, shown or not.
                    if event.button == MouseButton::Right && axis == Axis::Horizontal {
                        cx.stop_propagation();
                        workbench.update(cx, |workbench, cx| workbench.show_scrollbar_context_menu(event.position, window, cx));
                        return;
                    }
                    if event.button != MouseButton::Left {
                        return;
                    }
                    let along = |position: Point<Pixels>| f32::from(if axis == Axis::Vertical { position.y - track.top() } else { position.x - track.left() });
                    let offset = along(event.position);
                    cx.stop_propagation();
                    workbench.update(cx, |workbench, cx| workbench.press_scrollbar(axis, offset, f32::from(if axis == Axis::Vertical { event.position.y } else { event.position.x }), cx));
                });
            },
        )
    }

    /// A press on a track: outside the slider it jumps to center the slider there, then a drag starts.
    fn press_scrollbar(&mut self, axis: Axis, offset: f32, pointer: f32, cx: &mut Context<Self>) {
        let viewport = self.editors.viewport;
        let editor = self.editors.active_mut().expect("scrollbars show only with an editor open");
        let bar = scrollbar(editor, axis, viewport);
        let on_slider = (bar.slider_position..bar.slider_position + bar.slider_size).contains(&offset);
        if !on_slider {
            set_axis_scroll(editor, axis, bar.position_at(offset), viewport);
        }
        let state = scrollbar(editor, axis, viewport);
        self.scrollbars.drag = Some(SliderDrag { axis, pointer, state });
        self.editor_scrolled(cx);
    }

    fn drag_slider(&mut self, drag: SliderDrag, pointer: f32, cx: &mut Context<Self>) {
        let viewport = self.editors.viewport;
        let editor = self.editors.active_mut().expect("scrollbars show only with an editor open");
        if set_axis_scroll(editor, drag.axis, drag.state.position_dragged(pointer - drag.pointer), viewport) {
            self.editor_scrolled(cx);
        }
    }

    /// scrollDecoration.css: `box-shadow: var(--vscode-scrollbar-shadow) 0 6px 6px -6px inset` over a 6px strip,
    /// which leaves the blurred top edge only: alpha falls off as a Gaussian with sigma 3px, half strength at the edge.
    pub(super) fn scroll_shadow(&self) -> gpui::Canvas<()> {
        let shadow = self.theme.color("scrollbar.shadow");
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let Some(shadow) = shadow else { return };
                let scale = window.scale_factor();
                let rows = (SCROLL_SHADOW_HEIGHT * scale) as usize;
                for row in 0..rows {
                    let y = (row as f32 + 0.5) / scale;
                    let alpha = shadow.a * 0.5 * erfc(y / (3. * std::f32::consts::SQRT_2));
                    let origin = bounds.origin + point(px(0.), px(row as f32 / scale));
                    window.paint_quad(fill(Bounds::new(origin, size(bounds.size.width, px(1. / scale))), Rgba { a: alpha, ..shadow }));
                }
            },
        )
    }

    pub(super) fn editor_lines(&self, editor: &OpenEditor, range: Range<usize>, metrics: EditorMetrics, window: &mut Window, cx: &mut Context<Self>) -> Vec<Div> {
        // Of a notebook's cells or the playground's snippets only the focused one has the code editor's focus.
        let in_cell = editor.document().scheme == crate::notebook::CELL_SCHEME;
        let stacked = in_cell || editor.document().scheme == super::walkthrough_part::SNIPPET_SCHEME;
        let focused = self.editor_focus.is_focused(window) && (!stacked || self.editors.active().is_some_and(|active| std::ptr::eq(active, editor)));
        let scroll_left = editor.scroll.left;
        let selections = &editor.selections;
        let tab_size = editor.document().indentation.tab_size;
        let primary_line = editor.selection().head.line;
        // currentLineHighlight.ts: every cursor's line, drawn only while all selections are empty.
        let all_empty = selections.iter().all(|selection| selection.is_empty());
        let bracket_matches = self.bracket_matches(editor);
        // Past the cap there are no marks, the banner shows instead.
        let unicode = self.unicode_highlights(editor).filter(|(_, highlights)| !highlights.has_more);
        let bracket_box = (self.theme.color("editorBracketMatch.background"), self.theme.color("editorBracketMatch.border"));
        // indentGuides.ts: the guide around the primary cursor with `editor.guides.highlightActiveIndentation`.
        let active_guide = (self.language_setting(editor, "editor.guides.highlightActiveIndentation") != json!(false))
            .then(|| crate::editor::active_indent_guide(&editor.text_model().guides, editor.selections[0].head.line));
        let colors = LineColors {
            number: self.color("editorLineNumber.foreground"),
            active_number: self.color("editorLineNumber.activeForeground"),
            text: self.color("editor.foreground"),
            // currentLineHighlight.ts theming participant: the border only without a visible background, or when the theme sets it.
            line_background: self.theme.color(if focused { "editor.lineHighlightBackground" } else { "editor.inactiveLineHighlightBackground" }),
            line_border: match self.theme.color("editor.lineHighlightBackground").is_none_or(|color| color.a == 0.) || self.theme.defines("editor.lineHighlightBorder") {
                true => self.theme.color("editor.lineHighlightBorder"),
                false => None,
            },
            selection_highlight: self.theme.color("editor.selectionHighlightBackground"),
            selection_highlight_border: self.theme.color("editor.selectionHighlightBorder"),
            guide: self.color("editorIndentGuide.background1"),
            active_guide: self.color("editorIndentGuide.activeBackground1"),
            cursor: self.color("editorCursor.foreground"),
            primary_cursor: self.color("editorMultiCursor.primary.foreground"),
            secondary_cursor: self.color("editorMultiCursor.secondary.foreground"),
            selection: self.color(if focused { "editor.selectionBackground" } else { "editor.inactiveSelectionBackground" }),
        };
        // currentLineHighlight.css: high contrast draws the exact border 1px wide.
        let line_border_width = if self.theme.kind.is_high_contrast() { 1. } else { 2. };
        // editorOptions.ts: overtype input mode draws `overtypeCursorStyle`, else `cursorStyle`; only block and line are drawn.
        let block_cursor = self.setting(if self.overtype { "editor.overtypeCursorStyle" } else { "editor.cursorStyle" }) == json!("block");
        let cursor_background = self.theme.color("editorCursor.background");
        // outputView.ts `OutputEditor.getConfigurationOverrides`: renderLineHighlight none.
        // cellEditorOptions.ts: renderLineHighlightOnlyWhenFocus.
        // simpleEditorOptions.ts: an interactive input's is none.
        let line_highlight = editor.document().scheme != crate::output::OUTPUT_SCHEME && (!in_cell || focused) && !self.is_interactive_input(editor);
        let char_width = metrics.char_width;
        let fold_controls = self.fold_controls.opacity(Instant::now());
        let settings_file = self.edits_settings_file(editor);
        let edit_setting_line = settings_file.then(|| self.edit_setting_line(editor)).flatten();
        let fold_control_color = self.color("editorGutter.foldingControlForeground");
        let fold_background = self.color("editor.foldBackground");
        let fold_placeholder = self.color("editor.foldPlaceholderForeground");
        let rounded_selection = self.setting("editor.roundedSelection") != json!(false);
        let render_whitespace = self.setting("editor.renderWhitespace");
        let render_whitespace = render_whitespace.as_str().unwrap_or_default();
        let render_control_characters = self.setting("editor.renderControlCharacters") != serde_json::Value::Bool(false);
        let whitespace_color = self.color("editorWhitespace.foreground");
        let line_height = metrics.line_height;
        // selections.css: high contrast themes square the corners.
        let selection_radius = if self.theme.kind.is_high_contrast() { 0. } else { 3. };
        let editor_background = self.color("editor.background");
        let view_line_count = editor.view_line_count();
        // viewLines.ts linesVisibleRangesForRange: a selected line break is one typicalHalfwidthCharacterWidth wide.
        // With word wrap each view line gets the part of the selection on its segment, in the view line's x.
        let selection_range = |selection: Selection, view_line: usize| {
            let segment = editor.segment(view_line);
            let text = &editor.document().lines[segment.line];
            let (start, end, line_break) = selected_columns(selection, segment.line, text)?;
            let line_break = line_break && segment.last;
            let (start, end) = (start.max(segment.start), end.min(segment.end));
            if start > end || (start == end && !line_break && !segment.first) {
                return None;
            }
            let layout = editor.line_layout(segment.line);
            let (left, right) = layout.range(start, end);
            let segment_left = segment.indent as f32 - layout.x(segment.start);
            Some(((segment_left + left) * char_width, (segment_left + right) * char_width + if line_break { char_width } else { 0. }))
        };
        let mut fold_starts = HashMap::new();
        for (range, decoration) in editor.folding.regions.ranges().iter().zip(editor.folding.decorations()) {
            if decoration != FoldDecoration::Hidden {
                fold_starts.insert(range.start - 1, decoration == FoldDecoration::Collapsed);
            }
        }
        range
            .map(|view_line| {
                let segment = editor.segment(view_line);
                let index = segment.line;
                let text = &editor.document().lines[index];
                let layout = editor.line_layout(index);
                // The x of the segment's first char in the model line's x, which the line's decorations use.
                let shift = (layout.x(segment.start) - segment.indent as f32) * char_width;
                let collapsed = fold_starts.get(&index).copied().filter(|_| segment.first);
                // lineNumbers.ts marks only the primary cursor's line active.
                let number = div()
                    .w(px(metrics.line_numbers_width))
                    .flex_none()
                    .text_right()
                    .text_color(if index == primary_line { colors.active_number } else { colors.number })
                    // lineNumbers.css `.relative-current-line-number`: the cursor's line number aligns left.
                    .when(metrics.line_numbers == LineNumbers::Relative && index == primary_line, |number| number.text_left())
                    .children(line_number_label(metrics.line_numbers, index, primary_line, editor.document().lines.len()).filter(|_| segment.first));
                let mut current_line = div().absolute().inset_0();
                if line_highlight && all_empty && (0..selections.len()).any(|cursor_index| editor.head_view_line(cursor_index) == view_line) {
                    if let Some(background) = colors.line_background {
                        current_line = current_line.bg(background);
                    }
                    if let Some(border) = colors.line_border {
                        current_line = current_line.border(px(line_border_width)).border_color(border);
                    }
                }
                // The text scrolls left under the fixed margin and current line border.
                let mut content = div().absolute().top_0().bottom_0().right_0().left(px(-scroll_left - shift));
                // selections.ts: inner corner pieces of every selection go before the rest (issue #77777).
                let mut inner_corners = Vec::new();
                let mut pieces = Vec::new();
                for selection in selections {
                    let in_line_x = |(left, right): (f32, f32)| (left + shift, right + shift);
                    let Some((left, right)) = selection_range(*selection, view_line).map(in_line_x) else { continue };
                    let above = view_line.checked_sub(1).and_then(|above| selection_range(*selection, above)).map(in_line_x);
                    let below = if view_line + 1 < view_line_count { selection_range(*selection, view_line + 1).map(in_line_x) } else { None };
                    // `hasMultipleSelections` insets the first line by 1px at the top, the last (not also first) at the bottom.
                    let multiple = selections.len() > 1;
                    let top = if multiple && above.is_none() { 1. } else { 0. };
                    let bottom = if multiple && above.is_some() && below.is_none() { 1. } else { 0. };
                    let piece = |left: f32, width: f32| div().absolute().top(px(top)).bottom(px(bottom)).left(px(left)).w(px(width));
                    let mut rest = piece(left, right - left).bg(colors.selection);
                    if rounded_selection {
                        let corners = selection_corners(above, (left, right), below, char_width / 4.);
                        let radius = px(selection_radius);
                        if corners.start_top == Corner::Intern || corners.start_bottom == Corner::Intern {
                            let mut background = piece(left - ROUNDED_PIECE_WIDTH, ROUNDED_PIECE_WIDTH).bg(editor_background);
                            if corners.start_top == Corner::Intern {
                                background = background.rounded_tr(radius);
                            }
                            if corners.start_bottom == Corner::Intern {
                                background = background.rounded_br(radius);
                            }
                            inner_corners.push(piece(left - ROUNDED_PIECE_WIDTH, ROUNDED_PIECE_WIDTH).bg(colors.selection));
                            inner_corners.push(background);
                        }
                        if corners.end_top == Corner::Intern || corners.end_bottom == Corner::Intern {
                            let mut background = piece(right, ROUNDED_PIECE_WIDTH).bg(editor_background);
                            if corners.end_top == Corner::Intern {
                                background = background.rounded_tl(radius);
                            }
                            if corners.end_bottom == Corner::Intern {
                                background = background.rounded_bl(radius);
                            }
                            inner_corners.push(piece(right, ROUNDED_PIECE_WIDTH).bg(colors.selection));
                            inner_corners.push(background);
                        }
                        if corners.start_top == Corner::Extern {
                            rest = rest.rounded_tl(radius);
                        }
                        if corners.start_bottom == Corner::Extern {
                            rest = rest.rounded_bl(radius);
                        }
                        if corners.end_top == Corner::Extern {
                            rest = rest.rounded_tr(radius);
                        }
                        if corners.end_bottom == Corner::Extern {
                            rest = rest.rounded_br(radius);
                        }
                    }
                    pieces.push(rest);
                }
                content = content.children(inner_corners).children(pieces);
                for level in 0..editor.text_model().guides[index] {
                    let x = (level * editor.document().indentation.indent_size()) as f32 * char_width;
                    let active = active_guide.as_ref().is_some_and(|(lines, indent)| lines.contains(&index) && level + 1 == *indent);
                    content = content.child(div().absolute().top_0().left(px(x)).w(px(1.)).h_full().bg(if active { colors.active_guide } else { colors.guide }));
                }
                // foldingDecorations.ts COLLAPSED_HIGHLIGHTED_VISUAL_DECORATION: .folded-background over the whole line.
                if collapsed == Some(true) {
                    content = content.child(div().absolute().inset_0().left(px(scroll_left)).bg(fold_background));
                }
                let (find_whole_lines, find_matches, find_colors) = self.find_line_decorations(editor, index, char_width, scroll_left);
                content = content.children(find_whole_lines);
                content = content.children(self.diff_line_decorations(editor, index, char_width, scroll_left));
                content = content.children(self.text_decoration_boxes(editor, index, scroll_left, |column| layout.x(column) * char_width));
                content = content.children(self.search_editor_line_decorations(editor, index, char_width));
                // anchorSelect.css: `.selection-anchor` is a 2px #007ACC bar in every theme, under the word highlights.
                if let Some(anchor) = editor.selection_anchor
                    && anchor.line == index
                {
                    let left = layout.x(anchor.column) * char_width;
                    content = content.child(div().absolute().top_0().h_full().left(px(left)).w(px(2.)).bg(rgb(0x007ACC)));
                }
                // highlightDecorations.css: `.focused .selectionHighlight`, drawn under the word highlights.
                let selection_highlights = self.find.selection_highlights.as_ref().filter(|_| focused).map_or(&[][..], |(_, ranges)| ranges);
                let first = selection_highlights.partition_point(|range| range.end.line < index);
                let on_line = selection_highlights[first..].iter().take_while(|range| range.start.line <= index);
                for (start, end, _) in on_line.filter_map(|range| selected_columns(Selection { anchor: range.start, head: range.end }, index, text)) {
                    let (start, end) = layout.range(start, end);
                    let (start, end) = (start * char_width, end * char_width);
                    let mut highlight = div().absolute().top_0().h_full().left(px(start)).w(px(end - start));
                    if let Some(background) = colors.selection_highlight {
                        highlight = highlight.bg(background);
                    }
                    // shortcut: GPUI has no dotted border, so the high contrast selection highlight is dashed.
                    if let Some(border) = colors.selection_highlight_border {
                        highlight = highlight.border_1().border_dashed().border_color(border);
                    }
                    content = content.child(highlight);
                }
                // wordHighlighter.ts stops on blur: a notebook cell without the focus has none.
                for (_, columns) in editor.highlights.iter().filter(|(line, _)| *line == index && (!in_cell || focused)) {
                    let (start, end) = layout.range(columns.start, columns.end);
                    let (start, end) = (start * char_width, end * char_width);
                    let mut highlight = div().absolute().top_0().h_full().left(px(start)).w(px(end - start));
                    let (word_background, word_border) = self.word_highlight_colors(index, columns);
                    if let Some(background) = word_background {
                        highlight = highlight.bg(background);
                    }
                    // shortcut: GPUI has no dotted border, so the high contrast word highlight is dashed.
                    if let Some(border) = word_border {
                        highlight = highlight.border_1().border_dashed().border_color(border);
                    }
                    content = content.child(highlight);
                }
                content = content.children(self.content_hover_highlight(editor, index, char_width));
                content = content.children(self.code_action_highlights(editor, index, char_width));
                content = content.children(self.snippet_placeholder_highlights(editor, index, char_width));
                content = content.children(self.symbol_highlight(editor, index, char_width));
                content = content.children(self.definition_link_cursor(editor, index, char_width));
                // bracketMatching.css .bracket-match: background and a 1px border inside the bracket's box.
                for (_, bytes) in bracket_matches.iter().flat_map(|(pair, _)| pair).filter(|(line, _)| *line == index) {
                    let (start, end) = layout.range(text[..bytes.start].chars().count(), text[..bytes.end].chars().count());
                    let (start, end) = (start * char_width, end * char_width);
                    let mut bracket = div().absolute().top_0().h_full().left(px(start)).w(px(end - start));
                    if let Some(background) = bracket_box.0 {
                        bracket = bracket.bg(background);
                    }
                    if let Some(border) = bracket_box.1 {
                        bracket = bracket.border_1().border_color(border);
                    }
                    content = content.child(bracket);
                }
                content = content.children(self.unicode_highlight_boxes(editor, index, unicode.as_ref(), char_width));
                content = content.children(self.keybinding_mark_boxes(editor, index, |column| layout.x(column) * char_width));
                content = content.children(self.marker_line_decorations(editor, index, char_width, |column| layout.x(column) * char_width));
                content = content.children(find_matches);
                // inPlaceReplace.css: .valueSetReplacement only in the light theme (`.monaco-editor.vs`), a 2px outline.
                if let Some((_, line, columns)) = &editor.value_set_replacement
                    && *line == index
                    && self.theme.kind == ThemeKind::Light
                    && let Some(border) = bracket_box.1
                {
                    let (start, end) = layout.range(columns.start, columns.end);
                    let (start, end) = (start * char_width, end * char_width);
                    content = content.child(div().absolute().top(px(-2.)).bottom(px(-2.)).left(px(start - 2.)).w(px(end - start + 4.)).border_2().border_color(border));
                }
                let selected: Vec<_> = selections
                    .iter()
                    .filter_map(|selection| selected_columns(*selection, index, text))
                    .filter(|(start, end, _)| start < end)
                    .map(|(start, end, _)| start..end)
                    .collect();
                let rendered_len = line_overflow(text, self.stop_rendering_line_after).map_or(text.chars().count(), |(limit, _, _)| limit);
                let marks = whitespace_marks(text, render_whitespace, &selected, rendered_len);
                if !marks.is_empty() {
                    let marks: Vec<_> = marks.into_iter().map(|mark| (layout.char_x(mark) * char_width, text.chars().nth(mark) == Some('\t'))).collect();
                    content = content.child(whitespace_canvas(marks, char_width, line_height, whitespace_color));
                }
                let mut highlights = editor.document().tokens.highlights(&self.theme, &self.languages, &editor.document().lines, index, tab_size);
                // bracketMatching.ts theming participant: `editorBracketMatch.foreground` over the bracket pair colors.
                if let Some(color) = self.theme.color("editorBracketMatch.foreground") {
                    let expanded = |byte: usize| expand_tabs(&text[..byte], tab_size).len();
                    let matched = bracket_matches.iter().flat_map(|(pair, _)| pair).filter(|(line, _)| *line == index);
                    let colored: Vec<_> = matched.map(|(_, bytes)| (expanded(bytes.start)..expanded(bytes.end), gpui::Hsla::from(color))).collect();
                    highlights = crate::syntax::overlay_colors(highlights, &colored);
                }
                let highlights = super::semantic_tokens::overlay_semantic_styles(highlights, &self.semantic_token_styles(editor, index, tab_size));
                let highlights = gpui::combine_highlights(highlights, self.link_styles(editor, index, tab_size));
                let highlights = gpui::combine_highlights(highlights, self.linked_editing_styles(editor, index, tab_size));
                let runs = gpui::combine_highlights(gpui::combine_highlights(highlights, self.text_decoration_styles(editor, index, tab_size)), find_colors);
                let runs = gpui::combine_highlights(runs, self.marker_text_styles(editor, index, tab_size));
                let runs = gpui::combine_highlights(runs, self.definition_link_styles(editor, index, tab_size));
                let mut show_more = None;
                match line_overflow(text, self.stop_rendering_line_after) {
                    None => content = content.children(injected_text_view::line_text_pieces(expand_tabs(text, tab_size), runs.collect(), text, &layout, tab_size, char_width)),
                    Some((limit, byte, label)) => {
                        // viewLineRenderer.ts: the first `stopRenderingLineAfter` chars, then the .mtkoverflow "Show more" span.
                        let rendered = expand_tabs(&text[..byte], tab_size);
                        let len = rendered.len();
                        let runs: Vec<_> = runs.filter(|(range, _)| range.start < len).map(|(range, style)| (range.start..range.end.min(len), style)).collect();
                        content = content.child(div().absolute().top_0().left_0().whitespace_nowrap().child(StyledText::new(rendered).with_highlights(runs)));
                        let foreground = self.theme.color("button.foreground").unwrap_or(colors.text);
                        let span = div()
                            .absolute()
                            .left(px(layout.x(limit) * char_width - scroll_left))
                            .w(px(show_more_width(&label, char_width)))
                            .h_full()
                            .rounded(px(2.))
                            .border_1()
                            // An unset `--vscode-contrastBorder` leaves the border `currentColor`.
                            .border_color(self.theme.color("contrastBorder").unwrap_or(foreground))
                            .bg(self.theme.color("button.background").unwrap_or(editor_background))
                            .text_color(foreground)
                            .when_some(self.theme.color("button.hoverBackground"), |span, hover| span.hover(move |span| span.bg(hover)))
                            .cursor_pointer()
                            .child(div().absolute().top(px(2.)).left(px(4.)).line_height(px(metrics.line_height)).whitespace_nowrap().child(label));
                        // The inline span's box reaches 3px above the line and 2px below it (reference pixels), past the
                        // line's own clip, so it gets a clip of its own over the text area.
                        show_more = Some(div().absolute().left(px(metrics.content_left())).right_0().top(px(-3.)).h(px(24.)).overflow_hidden().child(span));
                    }
                }
                content = content.children(self.injected_text_elements(editor, index, &layout, &metrics, cx));
                if render_control_characters {
                    content = content.children(self.control_character_boxes(text, &layout, tab_size, char_width, rendered_len));
                }
                // folding.css .inline-folded:after: an ellipsis 0.2em after the line's last char.
                if collapsed == Some(true) && segment.last {
                    let end = layout.width() * char_width + 0.2 * metrics.font_size;
                    content = content.child(div().absolute().top_0().left(px(end)).text_color(fold_placeholder).cursor_pointer().child("\u{22EF}"));
                }
                for (cursor_index, selection) in selections.iter().enumerate().filter(|(cursor_index, _)| focused && editor.head_view_line(*cursor_index) == view_line) {
                    // shortcut: the cursor does not blink (editor.cursorBlinking "blink" hides it every 500ms).
                    let color = match (selections.len(), cursor_index) {
                        (1, _) => colors.cursor,
                        (_, 0) => colors.primary_cursor,
                        _ => colors.secondary_cursor,
                    };
                    // A cursor past the cut is not drawn.
                    if self.stop_rendering_line_after.is_some_and(|limit| selection.head.column > limit) {
                        continue;
                    }
                    let x = layout.x(selection.head.column) * char_width;
                    if block_cursor {
                        // viewCursor.ts block: one char wide (a tab too), the char under it in `editorCursor.background`,
                        // else the caret color's opposite (viewCursors.ts).
                        let under = text.chars().nth(selection.head.column).filter(|ch| *ch != '\t').map(String::from).unwrap_or_default();
                        let text_color = cursor_background.unwrap_or(gpui::Rgba { r: 1. - color.r, g: 1. - color.g, b: 1. - color.b, a: color.a });
                        content = content.child(div().absolute().top_0().left(px(x)).w(px(char_width)).h_full().bg(color).text_color(text_color).child(under));
                        continue;
                    }
                    // viewCursor.ts: a 2px cursor sits 1px left, between the chars.
                    let left = if x >= 1. { x - 1. } else { x };
                    content = content.child(div().absolute().top_0().left(px(left)).w(px(2.)).h_full().bg(color));
                }
                if let Some(target) = self.text_drag.as_ref().and_then(|drag| drag.target).or(self.drop_target()).filter(|target| target.line == index) {
                    content = content.child(self.drop_target_marker(layout.x(target.column) * char_width));
                }
                let segment_columns = (layout.x(segment.start), if segment.last { f32::MAX } else { layout.x(segment.end) });
                // The click handlers take the model text's visible column; the injected text's x is left out here.
                let visible_at = move |workbench: &Workbench, x: gpui::Pixels| {
                    let x = ((f32::from(x) - workbench.text_area_left.get() - metrics.content_left() + scroll_left + shift) / char_width).clamp(segment_columns.0, segment_columns.1);
                    workbench.editors.active().map_or(x, |editor| editor.line_layout(index).model_visible(x))
                };
                let content = div()
                    .relative()
                    .flex_1()
                    .h_full()
                    .overflow_hidden()
                    .text_color(colors.text)
                    .children(self.stack_frame_line(editor, index))
                    .child(current_line)
                    .child(self.wrapped_segment_clip(content, segment, shift, scroll_left, char_width))
                    .children(self.word_wrap_indicator(editor, segment, scroll_left, char_width))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| {
                        // The empty area under the lines (CONTENT_EMPTY) has its own press.
                        cx.stop_propagation();
                        window.focus(&workbench.editor_focus, cx);
                        let modifiers = event.modifiers;
                        let visible = visible_at(workbench, event.position.x);
                        workbench.definition_link_mouse_down(index, modifiers);
                        if workbench.start_text_drag(index, visible, modifiers, event.click_count) {
                            return;
                        }
                        workbench.selecting_text = true;
                        workbench.fold_mouse_down = workbench.at_line_end(index, visible).then_some((index, false));
                        workbench.click_text(index, visible, modifiers, modifiers.shift, event.click_count, cx);
                        workbench.keep_click_at_row_end(view_line, visible);
                        workbench.show_long_line(index, visible, char_width, cx);
                    }))
                    .on_mouse_down(MouseButton::Right, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        let visible = visible_at(workbench, event.position.x);
                        workbench.show_editor_context_menu(index, visible, event.position, window, cx);
                        workbench.show_long_line(index, visible, char_width, cx);
                    }))
                    .on_mouse_down(MouseButton::Middle, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| {
                        window.focus(&workbench.editor_focus, cx);
                        workbench.selecting_text = true;
                        let visible = visible_at(workbench, event.position.x);
                        workbench.fold_mouse_down = workbench.at_line_end(index, visible).then_some((index, false));
                        workbench.column_select_text(index, visible, event.modifiers.shift, cx);
                        workbench.show_long_line(index, visible, char_width, cx);
                    }))
                    .on_mouse_up(MouseButton::Left, cx.listener(move |workbench, event: &MouseUpEvent, window, cx| {
                        let visible = visible_at(workbench, event.position.x);
                        if workbench.text_drag.is_some() {
                            return workbench.drop_text(index, visible, event.modifiers, cx);
                        }
                        if workbench.links_mouse_up(index, visible, event.modifiers, false, window, cx) {
                            return;
                        }
                        if workbench.at_line_end(index, visible) {
                            workbench.release_fold_click(index, false, event.modifiers, false, cx);
                        }
                        workbench.search_editor_mouse_up(index, visible, event.click_count, window, cx);
                        workbench.definition_link_mouse_up(index, visible, event.modifiers, event.click_count, window, cx);
                    }))
                    .on_mouse_up(MouseButton::Middle, cx.listener(move |workbench, event: &MouseUpEvent, window, cx| {
                        if workbench.links_mouse_up(index, visible_at(workbench, event.position.x), event.modifiers, true, window, cx) {
                            return;
                        }
                        if workbench.at_line_end(index, visible_at(workbench, event.position.x)) {
                            workbench.release_fold_click(index, false, event.modifiers, true, cx);
                        }
                    }))
                    .on_drag_move(cx.listener(move |workbench, event: &DragMoveEvent<ExternalPaths>, _, cx| {
                        if event.bounds.contains(&event.event.position) {
                            workbench.drag_files_over_text(Some((index, visible_at(workbench, event.event.position.x))), event.event.modifiers.shift, cx);
                        }
                    }))
                    .on_drop(cx.listener(move |workbench, paths: &ExternalPaths, window, cx| {
                        let visible = visible_at(workbench, window.mouse_position().x);
                        workbench.drop_files_on_text(index, visible, paths.paths().to_vec(), window, cx);
                    }))
                    .on_mouse_move(cx.listener(move |workbench, event: &MouseMoveEvent, _, cx| {
                        workbench.editor_message_mouse_moved(index, cx);
                        workbench.commenting_hover_moved(Some(index), cx);
                        workbench.content_hover.row_target = Some((index, visible_at(workbench, event.position.x)));
                        workbench.definition_link_mouse_moved(index, visible_at(workbench, event.position.x), event.modifiers, event.pressed_button.is_some(), cx);
                        match event.pressed_button {
                            Some(MouseButton::Left) if workbench.line_select_start.is_some() => workbench.gutter_line_select_drag(index, cx),
                            Some(MouseButton::Left) if workbench.text_drag.is_some() => workbench.drag_text(index, visible_at(workbench, event.position.x), event.modifiers.alt, cx),
                            Some(MouseButton::Left) if workbench.selecting_text => {
                                let visible = visible_at(workbench, event.position.x);
                                workbench.click_text(index, visible, event.modifiers, true, 1, cx);
                                workbench.keep_click_at_row_end(view_line, visible);
                            }
                            Some(MouseButton::Middle) if workbench.selecting_text => workbench.column_select_text(index, visible_at(workbench, event.position.x), true, cx),
                            _ => {}
                        }
                    }));
                let model_id = editor.model_id;
                div()
                    .w_full()
                    .h(px(metrics.line_height))
                    .relative()
                    .flex()
                    .when(settings_file, |row| row.on_mouse_move(cx.listener(move |workbench, _: &MouseMoveEvent, window, cx| workbench.edit_setting_mouse_moved(model_id, index, window, cx))))
                    .children(self.diff_margin(editor, index, metrics))
                    .children(self.quick_diff_glyph(editor, index, metrics, cx))
                    .children(self.comment_glyph(editor, index, metrics, cx))
                    .child(
                        div()
                            .relative()
                            .flex_none()
                            .child(self.breakpoint_glyph_margin(editor, index, metrics.glyph_margin_width(), metrics.line_height, cx))
                            .children(self.lightbulb_glyph(editor, index, metrics.line_height, cx).map(|lightbulb| lightbulb.absolute().top_0().left_0()))
                            .children(self.edit_setting_glyph(editor, index, metrics.glyph_margin_width(), metrics.line_height, edit_setting_line, cx)),
                    )
                    .child(self.gutter_mouse(number, index, cx))
                    .child(self.fold_control(index, collapsed, metrics, fold_control_color, fold_controls, cx))
                    .child(content)
                    .children(show_more)
            })
            .collect()
    }

    /// The line decorations lane with the folding control of a region starting on `index`, collapsed or not
    /// (`folding-collapsed` stays visible, `folding-expanded` shows while the pointer is over the margin). A press
    /// 4px or more into the lane and a release on the same line toggle the region (folding.ts `onEditorMouseDown`).
    fn fold_control(&self, index: usize, collapsed: Option<bool>, metrics: EditorMetrics, color: Rgba, hover_opacity: f32, cx: &mut Context<Self>) -> Div {
        let lane_x = move |workbench: &Workbench, x: Pixels| f32::from(x) - workbench.text_area_left.get() - metrics.glyph_margin_width() - metrics.line_numbers_width;
        let press = move |workbench: &mut Workbench, event: &MouseDownEvent| {
            workbench.fold_mouse_down = (lane_x(workbench, event.position.x) >= 4.).then_some((index, true));
        };
        let mut lane = div()
            .w(px(metrics.decorations_width))
            .flex_none()
            .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, event: &MouseDownEvent, _, _| press(workbench, event)))
            .on_mouse_down(MouseButton::Middle, cx.listener(move |workbench, event: &MouseDownEvent, _, _| press(workbench, event)))
            .on_mouse_up(MouseButton::Left, cx.listener(move |workbench, event: &MouseUpEvent, _, cx| workbench.release_fold_click(index, true, event.modifiers, false, cx)))
            .on_mouse_up(MouseButton::Middle, cx.listener(move |workbench, event: &MouseUpEvent, _, cx| workbench.release_fold_click(index, true, event.modifiers, true, cx)));
        let Some(collapsed) = collapsed.filter(|_| self.editors.active_diff().is_none()) else { return lane };
        let (name, opacity) = if collapsed { ("chevron-right", 1.) } else { ("chevron-down", hover_opacity) };
        // folding.css: the icon box is the lane's width, 2px right, the glyph at 140% of the editor font size.
        let icon = self.icon(name, metrics.line_height, 1.4 * metrics.font_size, Rgba { a: color.a * opacity, ..color });
        // review.css `.inline-comment`: the folding icons move 11px right of the comment glyphs.
        let left = if metrics.decorations_width > DECORATIONS_WIDTH { 13. } else { 2. };
        lane = lane.child(div().w(px(DECORATIONS_WIDTH)).ml(px(left)).h(px(metrics.line_height)).flex().justify_center().cursor_pointer().child(icon));
        lane
    }

    /// Whether `visible` falls at or after the end of `line`, as a click on a collapsed region's ellipsis does.
    fn at_line_end(&self, line: usize, visible: f32) -> bool {
        let editor = self.editors.active().expect("text is clickable only with an editor open");
        let text = &editor.document().lines[line];
        editor.folding.has_hidden() && column_at_visible(text, visible, editor.document().indentation.tab_size) == text.chars().count()
    }

    /// folding.ts `onEditorMouseUp`: a release on the line pressed toggles its region, with Alt the regions around
    /// it, with Shift or the middle button the ones inside it first; then the line's start is revealed.
    fn release_fold_click(&mut self, line: usize, icon: bool, modifiers: gpui::Modifiers, middle: bool, cx: &mut Context<Self>) {
        if self.fold_mouse_down.take() != Some((line, icon)) {
            return;
        }
        let editor = self.editors.active_mut().expect("folding controls show only with an editor open");
        let before = (editor.selections.clone(), editor.folding.hidden().to_vec());
        if editor.click_fold_control(line, icon, modifiers.alt, middle || modifiers.shift) {
            self.cursors_changed(before.0, Reveal::CenterIfOutside(Position::new(line, 0)), "mouse", cx);
            self.folds_changed(&before.1);
        }
    }

    /// The view lines changed with the folds: the host hears the new visible ranges (`onDidScrollChange` fires
    /// for the new scroll height).
    fn folds_changed(&mut self, hidden_before: &[Range<usize>]) {
        let viewport = self.editors.viewport;
        let editor = self.editors.active_mut().expect("folds change in the active editor");
        if editor.folding.hidden() != hidden_before {
            editor.set_scroll(editor.scroll.top, editor.scroll.left, viewport);
            self.host.send_visible_ranges(&self.editors);
        }
    }

    /// bracketMatching.ts `_recomputeBrackets`: the brackets matched at each empty cursor, none past 100 selections;
    /// `editor.matchBrackets` "near" leaves out the enclosing pairs, "never" matches nothing.
    fn bracket_matches(&self, editor: &OpenEditor) -> Vec<([BracketRange; 2], bool)> {
        let overrides = config::configuration::Overrides { resource: Some(&editor.document().path), override_identifier: Some(&editor.document().language_id) };
        let match_brackets = self.settings.lock().unwrap().get_value(Some("editor.matchBrackets"), &overrides);
        if editor.selections.len() > 100 || match_brackets == Some(json!("never")) {
            return Vec::new();
        }
        let near = match_brackets == Some(json!("near"));
        let lines = &editor.document().lines;
        let mut matches = Vec::new();
        for selection in editor.selections.iter().filter(|selection| selection.is_empty()) {
            let found = editor.document().tokens.bracket_match(&self.theme, &self.languages, lines, (selection.head.line, selection.head.column));
            matches.extend(found.filter(|(_, touching)| *touching || !near));
        }
        matches
    }

    /// viewController.ts `dispatchMouse` on the text: a click places the cursor, Shift or a drag extends the
    /// selection, a double click selects the word and a triple click the line. With Alt (`multiCursorModifier`)
    /// a click adds or removes a cursor, a drag extends the cursor added last, a double or triple click
    /// selects its word or line, and with Shift too it selects a column.
    /// shortcut: a drag selects only while the pointer is over the lines, and by chars, not by the words or lines it started with.
    /// longLinesHelper.ts: a press on the text at or past the cut renders every line whole from then on.
    fn show_long_line(&mut self, line: usize, visible: f32, char_width: f32, cx: &mut Context<Self>) {
        let editor = self.editors.active().expect("text is clickable only with an editor open");
        let text = &editor.document().lines[line];
        let Some((limit, _, label)) = line_overflow(text, self.stop_rendering_line_after) else { return };
        let tab_size = editor.document().indentation.tab_size;
        // The "Show more" span is text too; a press right of it is empty content.
        let span_end = visible_column(text, limit, tab_size) as f32 + show_more_width(&label, char_width) / char_width;
        if column_at_visible(text, visible, tab_size) + 1 >= limit && visible < span_end {
            self.stop_rendering_line_after = None;
            cx.notify();
        }
    }

    pub(super) fn click_text(&mut self, line: usize, visible: f32, modifiers: gpui::Modifiers, select: bool, click_count: usize, cx: &mut Context<Self>) {
        let column_selection = self.setting("editor.columnSelection") == json!(true);
        // viewController.ts `_hasMulticursorModifier` and `_hasNonMulticursorModifier`; `ctrlCmd` is Cmd on macOS.
        let (multi_cursor, non_multi_cursor) = match self.multi_cursor_modifier_is_cmd() {
            true => (modifiers.platform, modifiers.control || modifiers.alt),
            false => (modifiers.alt, modifiers.control || modifiers.platform),
        };
        let editor = self.editors.active_mut().expect("text is clickable only with an editor open");
        let document = editor.document();
        let text = &document.lines[line];
        let tab_size = document.indentation.tab_size;
        let column = match line_overflow(text, self.stop_rendering_line_after) {
            // The reference puts a press on the "Show more" span before the last drawn char.
            Some((limit, _, _)) if visible >= visible_column(text, limit, tab_size) as f32 => limit - 1,
            _ => column_at_visible(text, visible, tab_size),
        };
        let word = word_at(text, column).unwrap_or(column..column);
        let line_end = if line + 1 < document.lines.len() { Position::new(line + 1, 0) } else { Position::new(line, text.chars().count()) };
        drop(document);
        let position = Position::new(line, column);
        let before = editor.selections.clone();
        if multi_cursor && click_count < 4 {
            if click_count == 1 && non_multi_cursor {
                return;
            }
            // Only the column selection reveals; the other `_lastCursor*` commands and `createCursor` do not.
            let reveal = match click_count {
                2 => {
                    editor.last_cursor_select_word(position);
                    Reveal::Nothing
                }
                3 => {
                    editor.last_cursor_select_line(line);
                    Reveal::Nothing
                }
                _ if modifiers.shift => editor.column_select_to_pointer(line, mouse_column(visible), true),
                _ if select => {
                    editor.last_cursor_select_to(position);
                    Reveal::Nothing
                }
                _ => {
                    editor.toggle_cursor(position);
                    Reveal::Nothing
                }
            };
            self.cursors_changed(before, reveal, "mouse", cx);
            return;
        }
        match click_count {
            2 => {
                editor.move_to(Position::new(line, word.start), false);
                editor.move_to(Position::new(line, word.end), true);
            }
            3.. => {
                editor.move_to(Position::new(line, 0), false);
                editor.move_to(line_end, true);
            }
            _ if select && (column_selection || modifiers.alt) => {
                let reveal = editor.column_select_to_pointer(line, mouse_column(visible), true);
                self.cursors_changed(before, reveal, "mouse", cx);
                return;
            }
            _ => editor.move_to(Position::new(line, column), select),
        }
        let moved = editor.selections != before;
        self.editor_edited(None, moved, "mouse", cx);
    }

    /// `editor.multiCursorModifier`: any value but `ctrlCmd` is Alt (editorOptions.ts `_multiCursorModifierFromString`).
    pub(super) fn multi_cursor_modifier_is_cmd(&self) -> bool {
        self.setting("editor.multiCursorModifier") == "ctrlCmd"
    }

    /// The middle button selects a column (`editor.scrollOnMiddleClick` off): from the pointer, or with
    /// Shift or while dragging from where the last column selection started.
    fn column_select_text(&mut self, line: usize, visible: f32, extend: bool, cx: &mut Context<Self>) {
        let editor = self.editors.active_mut().expect("text is clickable only with an editor open");
        let before = editor.selections.clone();
        let reveal = editor.column_select_to_pointer(line, mouse_column(visible), extend);
        self.cursors_changed(before, reveal, "mouse", cx);
    }

    /// Runs an editor command; false when its precondition does not hold.
    pub(super) fn run_editor_command(&mut self, command: &str, source: &str, cx: &mut Context<Self>) -> bool {
        let viewport = self.editors.viewport;
        let editor = self.editors.active_mut().expect("editor commands run on the active editor");
        let before = editor.selections.clone();
        let hidden_before = editor.folding.hidden().to_vec();
        let Some(reveal) = commands::run(editor, command, viewport) else {
            return false;
        };
        self.cursors_changed(before, reveal, source, cx);
        self.folds_changed(&hidden_before);
        true
    }

    /// After a command that moves cursors only: scrolls to what it reveals and tells the host.
    pub(super) fn cursors_changed(&mut self, before: Vec<Selection>, reveal: Reveal, source: &str, cx: &mut Context<Self>) {
        let scrolled = self.scroll_smoothly(cx, |editor, viewport| match reveal {
            Reveal::Nothing => {}
            Reveal::All => editor.reveal_cursors(source, viewport),
            Reveal::Position(position) => editor.reveal_position(position, source, viewport),
            Reveal::CenterIfOutside(position) => editor.reveal_range(position, position, VerticalReveal::CenterIfOutsideViewport, viewport),
        });
        let moved = self.editors.active().expect("cursors change in the active editor").selections != before;
        if moved {
            self.record_navigation(Some(crate::navigation::Reason::of_source(source)));
            self.output_cursor_moved();
            self.find_editor_edited(None);
            self.host.send_selection(self.editors.active().expect("still open"), source);
        }
        if scrolled {
            self.editor_scrolled(cx);
        }
        cx.notify();
    }

    /// decorationsOverviewRuler.ts `_render`, in device pixels: word highlights, matched brackets and find matches in the center lane, the cursor
    /// across all lanes, then the 1px border on the left and top edges.
    fn overview_ruler(&self, editor: &OpenEditor) -> gpui::Canvas<()> {
        let line_count = editor.document().lines.len();
        let [(highlight_lines, highlight), (read_lines, read_color), (write_lines, write_color)] = self.word_highlight_ruler_lines(editor);
        let mut bracket_lines = Vec::new();
        for ([open, close], in_ruler) in self.bracket_matches(editor) {
            if in_ruler {
                bracket_lines.extend([(open.0, open.0), (close.0, close.0)]);
            }
        }
        let bracket_color = self.color("editorOverviewRuler.bracketMatchForeground");
        // decorationsOverviewRuler.ts onCursorStateChanged: the cursor colors, each at 70% opacity.
        let ruler_color = |key: &str| self.theme.color(key).map(|color| Rgba { a: color.a * 0.7, ..color });
        let mut cursors = Vec::new();
        for (index, selection) in editor.selections.iter().enumerate() {
            let key = match (editor.selections.len(), index) {
                (1, _) => "editorCursor.foreground",
                (_, 0) => "editorMultiCursor.primary.foreground",
                _ => "editorMultiCursor.secondary.foreground",
            };
            cursors.push((selection.head.line, ruler_color(key)));
        }
        cursors.sort_by_key(|(line, _)| *line);
        bracket_lines.sort();
        let find_color = self.color("editorOverviewRuler.findMatchForeground");
        let range_lines: Vec<(usize, usize)> = self.quick_access.goto_line.iter().filter_map(|state| state.highlight(editor.model_id)).collect();
        let range_color = self.color("editorOverviewRuler.rangeHighlightForeground");
        let (mut find_lines, mut current_find_lines) = (Vec::new(), Vec::new());
        for mark in self.find_marks(editor, self.editors.viewport.height) {
            let lines = if mark.current { &mut current_find_lines } else { &mut find_lines };
            lines.push((mark.range.start.line, mark.range.end.line));
        }
        let [(info_lines, info_color), (error_lines, error_color)] = self.keybinding_ruler_marks(editor);
        let [(marker_info_lines, marker_info), (marker_warning_lines, marker_warning), (marker_error_lines, marker_error)] = self.marker_ruler_lines(editor);
        let quick_diff = self.quick_diff_ruler_lines(editor);
        let decoration_marks = self.text_decoration_ruler_marks(editor);
        let border = self.theme.color("editorOverviewRuler.border");
        // `editorOverviewRuler.background`, else the tokens' default background while the minimap is on the right, so
        // the ruler hides the minimap's line highlights under it.
        let minimap = self.minimap_settings();
        let background = self.theme.color("editorOverviewRuler.background").or_else(|| {
            (minimap.enabled && minimap.layout.side == crate::editor::minimap::MinimapSide::Right)
                .then(|| Rgba::try_from(self.theme.tokens.color_map()[2].as_str()).expect("the token color map holds hex colors"))
        });
        let Viewport { top_whitespace, line_height, .. } = self.editors.viewport;
        let zones: Vec<(usize, f32)> = editor.view_zones.iter().map(|zone| (zone.anchor.map_or(0, |anchor| anchor.line + usize::from(!zone.before)), zone.height)).collect();
        let line_top = move |line: usize| top_whitespace + line as f32 * line_height + zones.iter().filter(|zone| zone.0 <= line).map(|zone| zone.1).sum::<f32>();
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let scale = window.scale_factor();
                let device = |value: f32| px(value / scale);
                let origin = bounds.origin;
                let paint = |window: &mut Window, x: f32, y: f32, width: f32, height: f32, color: Rgba| {
                    window.paint_quad(fill(Bounds::new(origin + point(device(x), device(y)), size(device(width), device(height))), color));
                };
                let canvas_width = (OVERVIEW_RULER_WIDTH * scale).floor();
                let canvas_height = (f32::from(bounds.size.height) * scale).floor();
                let viewport_height = f32::from(bounds.size.height);
                // scrollBeyondLastLine: the last line can scroll to the top of the viewport.
                let scroll_height = line_top(line_count) + viewport_height - line_height;
                let ratio = canvas_height / scroll_height;
                let lanes = RulerLanes::new(canvas_width);
                let min_height = (6. * scale).floor();
                if let Some(background) = background {
                    paint(window, 0., 0., canvas_width, canvas_height, background);
                }
                // In z-index order: Go to Line's range in all lanes, find matches at 10, the current match at 13.
                let center = (lanes.center_x, lanes.center_width);
                for (lines, color, (x, width)) in [
                    (&range_lines, range_color, (lanes.full_x, lanes.full_width)),
                    (&info_lines, info_color, (lanes.right_x, lanes.right_width)),
                    (&error_lines, error_color, (lanes.right_x, lanes.right_width)),
                    (&marker_info_lines, marker_info, (lanes.right_x, lanes.right_width)),
                    (&marker_warning_lines, marker_warning, (lanes.right_x, lanes.right_width)),
                    (&marker_error_lines, marker_error, (lanes.right_x, lanes.right_width)),
                    (&highlight_lines, highlight, center),
                    (&read_lines, read_color, center),
                    (&write_lines, write_color, center),
                    (&bracket_lines, bracket_color, center),
                    (&find_lines, find_color, center),
                    (&current_find_lines, find_color, center),
                ] {
                    let mut previous: Option<(f32, f32)> = None;
                    for (start_line, end_line) in lines {
                        let mut y1 = (line_top(*start_line) * ratio).floor();
                        let mut y2 = ((line_top(*end_line) + line_height) * ratio).floor();
                        if y2 - y1 < min_height {
                            let half = (min_height / 2.).floor();
                            let center = ((y1 + y2) / 2.).floor().clamp(half, canvas_height - half);
                            (y1, y2) = (center - half, center + half);
                        }
                        previous = match previous {
                            Some((start, end)) if y1 <= end + 1. => Some((start, end.max(y2))),
                            Some((start, end)) => {
                                paint(window, x, start, width, end - start, color);
                                Some((y1, y2))
                            }
                            None => Some((y1, y2)),
                        };
                    }
                    if let Some((start, end)) = previous {
                        paint(window, x, start, width, end - start, color);
                    }
                }
                // Extension decorations' `overviewRulerColor` in their `OverviewRulerLane`.
                for (start, end, color, lane) in &decoration_marks {
                    let (x, width) = match lane {
                        1 => (lanes.full_x, lanes.center_x - lanes.full_x),
                        2 => center,
                        4 => (lanes.right_x, lanes.right_width),
                        _ => (lanes.full_x, lanes.full_width),
                    };
                    let y1 = (line_top(*start) * ratio).floor();
                    let y2 = ((line_top(*end) + line_height) * ratio).floor().max(y1 + min_height);
                    paint(window, x, y1, width, y2 - y1, *color);
                }
                // quickDiffDecorator.ts: `OverviewRulerLane.Left`, each change its own mark.
                for (start, end, color) in quick_diff.iter().filter_map(|(start, end, color)| Some((*start, *end, (*color)?))) {
                    let y1 = (line_top(start) * ratio).floor();
                    let y2 = ((line_top(end) + line_height) * ratio).floor().max(y1 + min_height);
                    paint(window, lanes.full_x, y1, lanes.center_x - lanes.full_x, y2 - y1, color);
                }
                // Cursors sorted by line; touching marks of one color paint as one.
                let height = (2. * scale).floor();
                let half = (height / 2.).floor();
                let mut previous: Option<(f32, f32, Rgba)> = None;
                for (line, color) in cursors.iter().filter_map(|(line, color)| Some((*line, (*color)?))) {
                    let y1 = (line_top(line) * ratio).floor().clamp(half, canvas_height - half) - half;
                    let y2 = y1 + height;
                    previous = match previous {
                        Some((start, end, previous_color)) if y1 <= end + 1. && previous_color == color => Some((start, end.max(y2), color)),
                        Some((start, end, previous_color)) => {
                            paint(window, lanes.full_x, start, lanes.full_width, end - start, previous_color);
                            Some((y1, y2, color))
                        }
                        None => Some((y1, y2, color)),
                    };
                }
                if let Some((start, end, color)) = previous {
                    paint(window, lanes.full_x, start, lanes.full_width, end - start, color);
                }
                // A 1px canvas stroke along x = 0 and y = 0 covers half of the first device column and row.
                if let Some(border) = border {
                    let half_covered = Rgba { a: border.a * 0.5, ..border };
                    paint(window, 0., 0., 1., canvas_height, half_covered);
                    paint(window, 1., 0., canvas_width - 1., 1., half_covered);
                }
            },
        )
        .w(px(OVERVIEW_RULER_WIDTH))
    }

    /// The column selection, tab focus and overtype mode items, then with an editor open its selection, indentation, encoding, end of line and
    /// language items (editorStatus.ts), right to left before the notifications bell.
    pub(super) fn editor_status_items(&self, foreground: Rgba, window: &Window, cx: &mut Context<Self>) -> Div {
        // editorStatus.ts `updateColumnSelectionModeElement`, `updateTabFocusModeElement` and `updateInputModeElement`: prominent entries.
        let mut items = Vec::new();
        if self.editors.active().is_some() && self.setting("editor.columnSelection") == true {
            items.push(("Column Selection".to_string(), "Disable Column Selection Mode", "editor.action.toggleColumnSelection"));
        }
        if self.tab_focus_mode() {
            items.push(("Tab Moves Focus".to_string(), "Disable Accessibility Mode", "editor.action.toggleTabFocusMode"));
        }
        if self.overtype {
            items.push(("OVR".to_string(), "Enable Insert Mode", "editor.action.toggleOvertypeInsertMode"));
        }
        let prominent_items = items.len();
        // A webview, extension, walkthrough or multi diff editor shows no editor status entries.
        // notebookIndicators.ts: a notebook editor whose cell editor has no focus shows its active cell instead.
        let notebook_cell = self.active_notebook_editor().filter(|_| !self.editor_focus.is_focused(window)).and_then(|editor| Some((editor.focus, self.host.notebooks.model(&editor.notebook)?.cells.len())));
        if let Some((input, cells)) = self.repl_input_status(window) {
            // Seen in the reference with the REPL input focused: its end of line only, between the indentation and
            // active cell entries of the list once it has a cell.
            let [_, _, _, eol, _] = Self::editor_status_entries(input, &self.languages);
            match cells {
                None => items.push(eol),
                Some((editor, index, count)) => {
                    if let Some(editor) = editor {
                        let [_, indentation, ..] = Self::editor_status_entries(editor, &self.languages);
                        items.push(indentation);
                    }
                    items.push(eol);
                    items.push((format!("Cell {} of {count}", index + 1), "", "notebook.centerActiveCell"));
                }
            }
        } else if let Some((index, count)) = notebook_cell {
            // editorStatus.ts: the indentation of the active cell's editor (`activeCodeEditor`), which a markdown
            // preview has not; seen in the reference with the list focused on a code cell.
            if let Some(editor) = self.editors.active().filter(|editor| editor.document().scheme == crate::notebook::CELL_SCHEME && self.active_cell_has_editor()) {
                let [_, indentation, ..] = Self::editor_status_entries(editor, &self.languages);
                items.push(indentation);
            }
            items.push((format!("Cell {} of {count}", index + 1), "", "notebook.centerActiveCell"));
        } else if let Some(editor) = self.editors.active().filter(|editor| editor.document().open_error.is_some()) {
            // editorStatus.ts `updateMetadataElement`: a binary editor's size; a placeholder has no text editor entries.
            if let Some(crate::editor::OpenError::Binary(size)) = editor.document().open_error {
                items.push((super::media_preview::format_size(size), "", ""));
            }
        } else if let Some(editor) = self.editors.active().filter(|editor| !self.shows_webview(editor) && !self.playground_active() && !matches!(editor.document().scheme.as_str(), super::extension_editor::EXTENSION_EDITOR_SCHEME | super::welcome_page::WALKTHROUGH_SCHEME | MULTI_DIFF_EDITOR_SCHEME)) {
            // WalkThroughPart has no `getControl`: its focused snippet gives the status bar no editor entries.
            items.extend(Self::editor_status_entries(editor, &self.languages));
        }
        items.extend(self.media_preview_status_entries());
        let prominent = (self.theme.color("statusBarItem.prominentBackground"), self.color("statusBarItem.prominentForeground"));
        // GPUI rounds a text box up to a whole px, Chrome keeps the fraction: the shaped width keeps the items in place.
        let mut style = window.text_style();
        style.font_features = FontFeatures(Arc::new(vec![("tnum".into(), 1)]));
        let text_width = |label: &str| window.text_system().shape_line(label.to_owned().into(), px(12.), &[style.to_run(label.len())], None).width;
        // A search editor's and a notebook editor's input support neither encodings nor language changes
        // (toEditorWithEncodingSupport, toEditorWithLanguageSupport).
        let fixed_input = self.editors.active().is_some_and(|editor| editor.document().scheme == SEARCH_EDITOR_SCHEME) || self.active_notebook_editor().is_some();
        let items: Vec<_> = items.into_iter().filter(|(_, _, command)| !fixed_input || !matches!(*command, "workbench.action.editor.changeEncoding" | "workbench.action.editor.changeLanguageMode")).collect();
        let language_index = items.iter().position(|(_, _, command)| *command == "workbench.action.editor.changeLanguageMode");
        // languageStatus.ts: the combined entry sits left of the language entry, the pinned ones right of it.
        let (combined, pinned) = match language_index {
            Some(_) => self.language_status_items(foreground, cx),
            None => (None, Vec::new()),
        };
        // statusbarpart.css `.compact-left`: the language entry next to the compact combined entry.
        let compact_language = combined.is_some();
        let items = items.into_iter().enumerate().map(|(index, (label, tooltip, command))| {
            let compact = compact_language && Some(index) == language_index;
            // The media preview's dimensions and file size entries have neither tooltip nor command.
            let hover = (!tooltip.is_empty()).then(|| self.hover_target(tooltip, super::hover::HoverContent::Text(tooltip.into()), super::hover::HoverOptions::status_bar()));
            let item = div()
                .id(("editor-status", index))
                .relative()
                .h_full()
                .w(text_width(&label) + px(if compact { 6. } else { 10. }))
                .flex_none()
                .when(!compact, |item| item.mx(px(3.)).px(px(5.)))
                .when(compact, |item| item.mr(px(5.)).px(px(3.)))
                .flex()
                .items_center()
                .whitespace_nowrap()
                .child(label)
                .children(hover)
                // statusbarItem.ts executeCommand on click.
                .when(!command.is_empty(), |item| {
                    item.cursor_pointer().on_click(cx.listener(move |workbench, _, window, cx| {
                        let outcome = workbench.execute_command(command, Vec::new(), window, cx);
                        super::quick_access::show_failure(command.to_owned(), outcome, window, cx);
                    }))
                });
            if index >= prominent_items {
                return item.into_any_element();
            }
            // `.prominent-kind` colors the container around the label's 3px margins, with modernUI statusBar.css
            // cornerRadius.small (4px).
            div().h_full().flex_none().rounded(px(4.)).text_color(prominent.1).when_some(prominent.0, |container, background| container.bg(background)).child(item).into_any_element()
        });
        let mut items: Vec<gpui::AnyElement> = items.collect();
        if let Some(language_index) = language_index {
            items.splice(language_index + 1..language_index + 1, self.language_detection_status_item(foreground, cx).into_iter().chain(pinned));
            if let Some(combined) = combined {
                items.insert(language_index, combined);
            }
        }
        div().h_full().flex().text_color(foreground).children(items)
    }

    /// editorStatus.ts entries of the editor: text, tooltip and command.
    fn editor_status_entries(editor: &OpenEditor, languages: &crate::languages::Languages) -> [(String, &'static str, &'static str); 5] {
        let document = &editor.document();
        let Indentation { tab_size, insert_spaces, .. } = document.indentation;
        let indent_size = document.indentation.indent_size();
        let indentation = match (insert_spaces, tab_size == indent_size) {
            (true, true) => format!("Spaces: {indent_size}"),
            (true, false) => format!("Spaces: {indent_size} (Tab Size: {tab_size})"),
            (false, _) => format!("Tab Size: {tab_size}"),
        };
        let encoding = SUPPORTED_ENCODINGS.iter().find(|supported| supported.id == document.encoding).map_or(document.encoding.as_str(), |supported| supported.label_short);
        let eol = if document.eol == "\r\n" { "CRLF" } else { "LF" };
        [
            (selection_label(editor), "Go to Line/Column", "workbench.action.gotoLine"),
            (indentation, "Select Indentation", super::editor_status::CHANGE_INDENTATION),
            (encoding.into(), "Select Encoding", "workbench.action.editor.changeEncoding"),
            (eol.into(), "Select End of Line Sequence", "workbench.action.editor.changeEOL"),
            (languages.name(&document.language_id).to_string(), "Select Language Mode", "workbench.action.editor.changeLanguageMode"),
        ]
    }
}

#[derive(Clone, Copy)]
pub(super) struct EditorMetrics {
    pub(super) font_size: f32,
    pub(super) line_height: f32,
    pub(super) char_width: f32,
    line_numbers: LineNumbers,
    line_numbers_width: f32,
    /// The line decorations lane: lineDecorationsWidth plus the folding controls, without them in a diff editor.
    decorations_width: f32,
}

/// editorOptions.ts `RenderLineNumbersType` from `editor.lineNumbers`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum LineNumbers {
    Off,
    On,
    Relative,
    Interval,
}

/// lineNumbers.ts `_getLineRenderLineNumber` for the zero-based `line` with the primary cursor on `cursor_line`.
fn line_number_label(render: LineNumbers, line: usize, cursor_line: usize, line_count: usize) -> Option<String> {
    let number = line + 1;
    match render {
        LineNumbers::Off => None,
        LineNumbers::On => Some(number.to_string()),
        LineNumbers::Relative if line == cursor_line => Some(number.to_string()),
        LineNumbers::Relative => Some(line.abs_diff(cursor_line).to_string()),
        LineNumbers::Interval => (line == cursor_line || number % 10 == 0 || number == line_count).then(|| number.to_string()),
    }
}

impl EditorMetrics {
    /// One glyph lane as wide as a line is high (editorOptions.ts layout).
    fn glyph_margin_width(&self) -> f32 {
        self.line_height
    }

    pub(super) fn content_left(&self) -> f32 {
        self.glyph_margin_width() + self.line_numbers_width + self.decorations_width
    }

    /// fontInfo.ts `BareFontInfo._create`: the editor zoom scales the font size and line height; then
    /// `typicalHalfwidthCharacterWidth` and `maxDigitWidth` of that font. The line numbers are as wide as the
    /// longest number, at least lineNumbersMinChars digits, rounded, and 0 wide when off (editorOptions.ts).
    pub(super) fn measure(window: &Window, line_count: usize, line_numbers: LineNumbers, zoom: f32) -> Self {
        let (font_size, line_height) = (EDITOR_FONT_SIZE * zoom, LINE_HEIGHT * zoom);
        let text_system = window.text_system();
        let font_id = text_system.resolve_font(&font(EDITOR_FONT));
        let advance = |c: char| f32::from(text_system.advance(font_id, px(font_size), c).expect("Menlo has ASCII glyphs").width);
        let max_digit_width = ('0'..='9').map(advance).fold(0., f32::max);
        let digits = if line_numbers == LineNumbers::Off { 0 } else { line_count.to_string().len().max(LINE_NUMBERS_MIN_CHARS) };
        EditorMetrics { font_size, line_height, char_width: advance('x'), line_numbers, line_numbers_width: (digits as f32 * max_digit_width).round(), decorations_width: DECORATIONS_WIDTH }
    }
}

struct LineColors {
    number: Rgba,
    active_number: Rgba,
    text: Rgba,
    line_background: Option<Rgba>,
    line_border: Option<Rgba>,
    selection_highlight: Option<Rgba>,
    selection_highlight_border: Option<Rgba>,
    guide: Rgba,
    active_guide: Rgba,
    cursor: Rgba,
    primary_cursor: Rgba,
    secondary_cursor: Rgba,
    selection: Rgba,
}

/// editorStatus.ts `getSelectionLabel`: the primary cursor's line and visible column for one cursor, the
/// count for several, and the chars selected (`getCharacterCountInRange`: code points, a line break as the EOL's length).
fn selection_label(editor: &OpenEditor) -> String {
    let document = &editor.document();
    let mut selected = 0;
    for selection in &editor.selections {
        let (start, end) = (selection.start(), selection.end());
        for line in start.line..=end.line {
            let first = if line == start.line { start.column } else { 0 };
            let last = if line == end.line { end.column } else { document.lines[line].chars().count() };
            selected += last - first;
        }
        selected += document.eol.len() * (end.line - start.line);
    }
    if let [selection] = editor.selections.as_slice() {
        let Position { line, column } = selection.head;
        let visible = crate::editor::logical_visible_column(&document.lines[line], column, document.indentation.tab_size);
        return match selected {
            0 => format!("Ln {}, Col {}", line + 1, visible + 1),
            _ => format!("Ln {}, Col {} ({selected} selected)", line + 1, visible + 1),
        };
    }
    match selected {
        0 => format!("{} selections", editor.selections.len()),
        _ => format!("{} selections ({selected} characters selected)", editor.selections.len()),
    }
}

const ROUNDED_PIECE_WIDTH: f32 = 10.;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Corner {
    Extern,
    Intern,
    Flat,
}

#[derive(PartialEq, Debug)]
struct SelectionCorners {
    start_top: Corner,
    start_bottom: Corner,
    end_top: Corner,
    end_bottom: Corner,
}

/// selections.ts `_enrichVisibleRangesWithStyle` for one line's range against the same selection's range on
/// the view lines above and below. Upstream keeps the previous frame's corners at the viewport edges; the
/// neighbor lines past the edge give the same corners.
fn selection_corners(above: Option<(f32, f32)>, (left, right): (f32, f32), below: Option<(f32, f32)>, epsilon: f32) -> SelectionCorners {
    let mut corners = SelectionCorners { start_top: Corner::Extern, start_bottom: Corner::Extern, end_top: Corner::Extern, end_bottom: Corner::Extern };
    if let Some((above_left, above_right)) = above {
        if (left - above_left).abs() < epsilon {
            corners.start_top = Corner::Flat;
        } else if left > above_left {
            corners.start_top = Corner::Intern;
        }
        if (right - above_right).abs() < epsilon {
            corners.end_top = Corner::Flat;
        } else if above_left < right && right < above_right {
            corners.end_top = Corner::Intern;
        }
    }
    if let Some((below_left, below_right)) = below {
        if (left - below_left).abs() < epsilon {
            corners.start_bottom = Corner::Flat;
        } else if below_left < left && left < below_right {
            corners.start_bottom = Corner::Intern;
        }
        if (right - below_right).abs() < epsilon {
            corners.end_bottom = Corner::Flat;
        } else if right < below_right {
            corners.end_bottom = Corner::Intern;
        }
    }
    corners
}

/// The selected chars of a line and whether its line break is selected too.
/// whitespace.ts with `experimentalWhitespaceRendering: svg`: a circle for a space, an arrow for a tab, each at
/// its mark's left edge on the line.
fn whitespace_canvas(marks: Vec<(f32, bool)>, space_width: f32, line_height: f32, color: Rgba) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let center_y = line_height / 2.;
            let at = |x: f32, y: f32| bounds.origin + point(px(x), px(y));
            for (left, tab) in &marks {
                if !tab {
                    let radius = (space_width / 7. * 100.).round() / 100.;
                    let center_x = left + space_width / 2.;
                    let circle = Bounds::new(at(center_x - radius, center_y - radius), size(px(radius * 2.), px(radius * 2.)));
                    window.paint_quad(fill(circle, color).corner_radii(px(radius)));
                    continue;
                }
                // _renderArrow: a shaft half a stroke thick either side of the middle, then the head.
                let stroke = space_width / 7.;
                let tip = 100. / 125. * space_width;
                let p1 = (0., stroke / 2.);
                let p2 = (tip, p1.1);
                let p3 = (p2.0 - 0.2 * tip, p2.1 + 0.2 * tip);
                let p4 = (p3.0 + 0.1 * tip, p3.1 + 0.1 * tip);
                let p5 = (p4.0 + 0.35 * tip, p4.1 - 0.35 * tip);
                let points = [p1, p2, p3, p4, p5, (p5.0, -p5.1), (p4.0, -p4.1), (p3.0, -p3.1), (p2.0, -p2.1), (p1.0, -p1.1)];
                let mut arrow = PathBuilder::fill();
                arrow.move_to(at(left + p1.0, center_y + p1.1));
                for (x, y) in &points[1..] {
                    arrow.line_to(at(left + x, center_y + y));
                }
                arrow.close();
                if let Ok(path) = arrow.build() {
                    window.paint_path(path, color);
                }
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

pub(super) fn selected_columns(selection: Selection, line: usize, text: &str) -> Option<(usize, usize, bool)> {
    let (start, end) = (selection.start(), selection.end());
    if selection.is_empty() || line < start.line || line > end.line {
        return None;
    }
    let first = if line == start.line { start.column } else { 0 };
    let last = if line == end.line { end.column } else { text.chars().count() };
    Some((first, last, line < end.line))
}

/// mouseTarget.ts `mouseColumn` less one: the visible column nearest the pointer, past the line end too.
fn mouse_column(visible: f32) -> usize {
    visible.round().max(0.) as usize
}

/// `_initLanes` with three lanes: one device pixel for the border, then three thirds, the center taking the rest.
struct RulerLanes {
    center_x: f32,
    center_width: f32,
    right_x: f32,
    right_width: f32,
    full_x: f32,
    full_width: f32,
}

impl RulerLanes {
    fn new(canvas_width: f32) -> Self {
        let remaining = canvas_width - 1.;
        let side = (remaining / 3.).floor();
        let center_width = remaining - 2. * side;
        RulerLanes { center_x: 1. + side, center_width, right_x: 1. + side + center_width, right_width: side, full_x: 1., full_width: remaining }
    }
}



/// scrollbarVisibilityController.ts for both scrollbars, with the CSS opacity transition between its states.
#[derive(Default)]
pub(super) struct Scrollbars {
    pub mouse_over: bool,
    pub(super) shown: bool,
    /// When `shown` last flipped, and the opacity the transition started from.
    changed: Option<(Instant, f32)>,
    hide_timer: Option<Task<()>>,
    drag: Option<SliderDrag>,
}

impl Scrollbars {
    pub(super) fn set_shown(&mut self, shown: bool, now: Instant) {
        if self.shown != shown {
            let from = self.opacity(now);
            self.shown = shown;
            self.changed = Some((now, from));
        }
    }

    /// Linear from where the last change started: 100ms to visible, 800ms to invisible.
    pub(super) fn opacity(&self, now: Instant) -> f32 {
        let target = if self.shown { 1. } else { 0. };
        let Some((at, from)) = self.changed else { return target };
        let duration = if self.shown { SCROLLBAR_FADE_IN } else { SCROLLBAR_FADE_OUT };
        let progress = (now.duration_since(at).as_secs_f32() / duration.as_secs_f32()).min(1.);
        from + (target - from) * progress
    }

    pub(super) fn in_transition(&self, now: Instant) -> bool {
        self.opacity(now) != if self.shown { 1. } else { 0. }
    }
}

/// folding.css: the expanded folding controls fade with `transition: opacity 0.5s` (ease) as the pointer enters
/// or leaves the margin.
#[derive(Default)]
pub(super) struct FoldControls {
    shown: bool,
    changed: Option<(Instant, f32)>,
}

impl FoldControls {
    /// folding.css: with reduced motion the icons show and hide without their fade.
    fn set_shown(&mut self, shown: bool, now: Instant, animate: bool) {
        if self.shown != shown {
            let from = self.opacity(now);
            self.shown = shown;
            self.changed = animate.then_some((now, from));
        }
    }

    fn opacity(&self, now: Instant) -> f32 {
        let target = if self.shown { 1. } else { 0. };
        let Some((at, from)) = self.changed else { return target };
        let progress = (now.duration_since(at).as_secs_f32() / FOLD_CONTROLS_FADE.as_secs_f32()).min(1.);
        from + (target - from) * ease(progress)
    }

    fn in_transition(&self, now: Instant) -> bool {
        self.opacity(now) != if self.shown { 1. } else { 0. }
    }
}

/// CSS `ease`, cubic-bezier(0.25, 0.1, 0.25, 1): the curve's x solved by bisection, then its y.
pub(super) fn ease(x: f32) -> f32 {
    let bezier = |p1: f32, p2: f32, t: f32| 3. * p1 * t * (1. - t).powi(2) + 3. * p2 * t * t * (1. - t) + t.powi(3);
    let (mut low, mut high) = (0., 1.);
    for _ in 0..24 {
        let mid = (low + high) / 2.;
        if bezier(0.25, 0.25, mid) < x { low = mid } else { high = mid }
    }
    bezier(0.1, 1., (low + high) / 2.)
}

#[derive(Clone, Copy)]
struct SliderDrag {
    axis: Axis,
    /// The pointer's window coordinate along the axis at the press, and the scrollbar then.
    pointer: f32,
    state: Scrollbar,
}

fn scrollbar(editor: &OpenEditor, axis: Axis, viewport: Viewport) -> Scrollbar {
    match axis {
        Axis::Vertical => editor.vertical_scrollbar(viewport),
        Axis::Horizontal => editor.horizontal_scrollbar(viewport),
    }
}

fn set_axis_scroll(editor: &mut OpenEditor, axis: Axis, position: f32, viewport: Viewport) -> bool {
    match axis {
        Axis::Vertical => editor.set_scroll(position, editor.scroll.left, viewport),
        Axis::Horizontal => editor.set_scroll(editor.scroll.top, position, viewport),
    }
}

/// The slider spans the scrollbar's thickness (`verticalSliderSize` and `horizontalSliderSize` default to it).
fn slider_bounds(track: Bounds<Pixels>, axis: Axis, bar: Scrollbar) -> Bounds<Pixels> {
    match axis {
        Axis::Vertical => Bounds::new(track.origin + point(px(0.), px(bar.slider_position)), size(track.size.width, px(bar.slider_size))),
        Axis::Horizontal => Bounds::new(track.origin + point(px(bar.slider_position), px(0.)), size(px(bar.slider_size), track.size.height)),
    }
}

/// The complementary error function (Abramowitz and Stegun 7.1.26, error below 1.5e-7).
pub(super) fn erfc(x: f32) -> f32 {
    let t = 1. / (1. + 0.327_591_1 * x);
    let poly = t * (0.254_829_6 + t * (-0.284_496_74 + t * (1.421_413_7 + t * (-1.453_152_1 + t * 1.061_405_4))));
    poly * (-x * x).exp()
}

#[cfg(test)]
mod tests {
    use super::{Corner, LineNumbers, SelectionCorners, line_number_label, selection_corners};
    use Corner::{Extern, Flat, Intern};

    #[test]
    fn line_numbers_render_as_line_numbers_ts() {
        let labels = |render, cursor_line| (0..12).map(|line| line_number_label(render, line, cursor_line, 12).unwrap_or_default()).collect::<Vec<_>>().join(" ");
        assert_eq!(labels(LineNumbers::On, 2), "1 2 3 4 5 6 7 8 9 10 11 12");
        assert_eq!(labels(LineNumbers::Relative, 2), "2 1 3 1 2 3 4 5 6 7 8 9");
        assert_eq!(labels(LineNumbers::Interval, 2), "  3       10  12");
        assert_eq!(labels(LineNumbers::Off, 2), "           ");
    }

    #[test]
    fn selection_corners_follow_the_neighbor_lines() {
        let corners = |start_top, start_bottom, end_top, end_bottom| SelectionCorners { start_top, start_bottom, end_top, end_bottom };
        // One line: every corner rounds out.
        assert_eq!(selection_corners(None, (10., 50.), None, 2.), corners(Extern, Extern, Extern, Extern));
        // A middle line starting left of the line above, ending left of the line below.
        assert_eq!(selection_corners(Some((20., 80.)), (0., 80.), Some((0., 100.)), 2.), corners(Extern, Flat, Flat, Intern));
        // A last line starting right of the line above and ending inside it.
        assert_eq!(selection_corners(Some((0., 80.)), (10., 30.), None, 2.), corners(Intern, Extern, Intern, Extern));
        // A start within epsilon of the line below counts as aligned.
        assert_eq!(selection_corners(None, (20.5, 80.), Some((20., 60.)), 2.), corners(Extern, Flat, Extern, Extern));
    }
}
