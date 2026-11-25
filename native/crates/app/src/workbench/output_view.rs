//! The Output view (outputView.ts `OutputViewPane` with output.contribution.ts): the active output channel's text in
//! a read-only editor.
// shortcut: the view is an editor tab of the group, not a panel view; the panel part is milestone 11's.

use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use gpui::{Bounds, Context, Div, FocusHandle, MouseButton, MouseDownEvent, Pixels, Task, Window, canvas, div, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use super::panel::PanelView;
use super::bulk_text_edits::document_end;
use super::input_box_view::{InputId, InputLook, InputView};
use crate::editor::{EditReason, OpenEditor, PANEL_GROUP, TextDocument};
use crate::input_box::InputBox;
use crate::output::{Content, OUTPUT_SCHEME};
use crate::theme::ThemeKind;

#[cfg(test)]
mod tests;

/// output.ts `OUTPUT_VIEW_ID`.
pub const OUTPUT_VIEW_ID: &str = "workbench.panel.output";

/// outputChannelModel.ts `FileContentProvider` polls its file every 500 ms (`ThrottledDelayer(500)`).
const POLL_INTERVAL: Duration = Duration::from_millis(500);

pub struct OutputView {
    /// A task reads the shown channel's file while the view is visible.
    polling: bool,
    /// The channel `$setVisibleChannel` last told the host about.
    visible_channel_sent: Option<String>,
    /// The shown channel's text since its last reset, before the filters hide lines.
    pub(super) text: String,
    /// `outputView.scrollLock`: the view stops following the end of the text.
    pub(super) scroll_lock: bool,
    pub(super) filter: InputBox,
    pub(super) filter_view: InputView,
    pub(super) filter_focus: FocusHandle,
    /// The filter text applies 300 ms after the last edit.
    pub(super) filtering: Option<Task<()>>,
    /// The channels with a `workbench.action.output.show.<id>` command.
    pub(super) channel_commands: Vec<String>,
    /// Where the title's dropdowns last painted, to anchor their menus.
    channel_menu_bounds: Rc<Cell<Bounds<Pixels>>>,
    filter_menu_bounds: Rc<Cell<Bounds<Pixels>>>,
    level_menu_bounds: Rc<Cell<Bounds<Pixels>>>,
}

impl OutputView {
    pub fn new(cx: &mut gpui::App) -> Self {
        OutputView {
            polling: false,
            visible_channel_sent: None,
            text: String::new(),
            scroll_lock: false,
            filter: InputBox::default(),
            filter_view: InputView::default(),
            filter_focus: cx.focus_handle(),
            filtering: None,
            channel_commands: Vec::new(),
            channel_menu_bounds: Rc::default(),
            filter_menu_bounds: Rc::default(),
            level_menu_bounds: Rc::default(),
        }
    }
}

impl Workbench {
    /// The Output view's code editor, in the panel's group, once a channel was shown.
    pub(super) fn output_editor(&self) -> Option<&OpenEditor> {
        let (editors, _) = self.editors.group_tabs(PANEL_GROUP);
        let in_panel = self.editors.id == PANEL_GROUP || self.editors.group(PANEL_GROUP).is_some();
        editors.first().filter(|editor| in_panel && editor.document().scheme == OUTPUT_SCHEME)
    }

    /// `isViewVisible(OUTPUT_VIEW_ID)`: the panel shows the Output view.
    pub(crate) fn output_view_visible(&self) -> bool {
        self.panel_view_visible(OUTPUT_VIEW_ID)
    }

    /// The Output view's editor has focus.
    pub(super) fn output_focused(&self, window: &Window) -> bool {
        self.output_view_visible() && self.editors.id == PANEL_GROUP && self.editor_focus.is_focused(window)
    }

    /// output.contribution.ts registers the Output view container in the panel.
    pub(super) fn register_output_panel_view(&mut self) {
        self.register_panel_view(PanelView {
            id: OUTPUT_VIEW_ID,
            title: "Output",
            render: |workbench, window, cx| workbench.output_panel_body(window, cx).into_any_element(),
            actions: |workbench, window, cx| workbench.output_title_actions(window, cx).into_any_element(),
        });
    }

    /// `OutputViewPane`: the channel's read-only code editor; a press in it moves the focus from the editor groups.
    fn output_panel_body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        if self.output_editor().is_none() {
            return div().flex_1();
        }
        let body = |workbench: &mut Self, window: &mut Window, cx: &mut Context<Self>| {
            let editor = workbench.editors.active().expect("the Output view's editor");
            div().flex_1().min_h_0().flex().flex_col().child(workbench.text_area(editor, window, cx))
        };
        match self.editors.id == PANEL_GROUP {
            true => body(self, window, cx),
            false => self.swapped_group_body(PANEL_GROUP, body, window, cx),
        }
    }

    /// `viewsService.openView(OUTPUT_VIEW_ID, focus)`: shows the active channel (`OutputViewPane.showChannel`).
    pub(super) fn open_output_view(&mut self, focus: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.output.active.clone() else { return };
        self.show_output_channel(&id, !focus, window, cx);
    }

    /// `OutputService.showChannel`: the channel becomes the active one and the view opens with its text.
    pub fn show_output_channel(&mut self, id: &str, preserve_focus: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(channel) = self.output.channel_mut(id) else { return };
        let text = channel.read_all();
        let language = channel.language().to_owned();
        let path = Path::new(id);
        let shown = self.output.filters.shown_text(&text, id);
        let document = TextDocument::from_text(OUTPUT_SCHEME, path, &shown, "utf8".into(), Some(&language), &self.languages, &self.settings.lock().unwrap());
        self.output_view.text = text;
        self.output.active = Some(id.to_owned());
        self.editors.ensure_panel_group();
        self.with_group(PANEL_GROUP, |workbench| match workbench.editors.editors.first() {
            Some(editor) if editor.document().path == path => workbench.editors.activate(0),
            Some(_) => workbench.editors.replace(0, document),
            None => workbench.editors.open(document, false),
        });
        self.reveal_output_end();
        self.open_panel_view(OUTPUT_VIEW_ID, cx);
        match preserve_focus {
            true => self.editors_changed(cx),
            false => self.focus_group(PANEL_GROUP, window, cx),
        }
        self.sync_visible_output_channel();
        self.start_output_poll(cx);
    }

    /// `MainThreadOutputService.$reveal`: with `workbench.view.showQuietly` on for the Output view and the view
    /// hidden, the channel is not shown.
    // shortcut: the quiet reveal shows no status bar entry; extension status bar entries are milestone 11's.
    pub(crate) fn reveal_output_channel(&mut self, id: &str, preserve_focus: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.output.channel(id).is_none() {
            return;
        }
        if !self.output_view_visible() && self.setting("workbench.view.showQuietly")[OUTPUT_VIEW_ID] == true {
            return;
        }
        self.show_output_channel(id, preserve_focus, window, cx);
    }

    /// `viewsService.closeView(OUTPUT_VIEW_ID)`.
    pub(crate) fn close_output_view(&mut self, cx: &mut Context<Self>) {
        self.close_panel_view(OUTPUT_VIEW_ID, cx);
        self.sync_visible_output_channel();
    }

    /// Channels were registered or removed: a removed shown channel gives way to the active one
    /// (outputServices.ts `onDidRemoveChannel`).
    pub(crate) fn output_channels_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_output_channel_commands();
        let Some(editor) = self.output_editor() else { return };
        let shown = editor.document().path.to_string_lossy().into_owned();
        if self.output.channel(&shown).is_some() {
            return;
        }
        match self.output.active.clone() {
            Some(active) if self.output_view_visible() => self.show_output_channel(&active, true, window, cx),
            _ => self.close_output_view(cx),
        }
    }

    /// `IOutputChannel.update`: an append reads what the file gained; a clear or a replace restarts the text at
    /// `till`, or at what was read without it.
    pub(crate) fn update_output_channel(&mut self, id: &str, mode: u64, till: Option<u64>, cx: &mut Context<Self>) {
        let shown = self.output_editor_shows(id);
        let Some(channel) = self.output.channel_mut(id) else { return };
        if mode != 1 {
            channel.reset(till);
        }
        if !shown {
            return;
        }
        // OutputChannelUpdateMode: 1 Append, 2 Replace, 3 Clear.
        let content = match mode {
            1 => channel.read_appended(),
            2 => Content::Reset(channel.read_all()),
            _ => Content::Reset(String::new()),
        };
        self.apply_output_content(id, content, cx);
    }

    fn output_editor_shows(&self, id: &str) -> bool {
        self.output_editor().is_some_and(|editor| editor.document().path == Path::new(id))
    }

    fn apply_output_content(&mut self, id: &str, content: Content, cx: &mut Context<Self>) {
        let path = Path::new(id);
        let filters = &self.output.filters;
        let filtered = !filters.include.is_empty() || !filters.exclude.is_empty() || filters.more_filters(Some(id));
        match content {
            Content::Append(text) if text.is_empty() => return,
            Content::Append(text) if filtered => {
                self.output_view.text.push_str(&text);
                let shown = self.output.filters.shown_text(&self.output_view.text, id);
                self.replace_model_text(OUTPUT_SCHEME, path, shown, true, cx);
            }
            Content::Append(text) => {
                self.output_view.text.push_str(&text);
                let model = self.editors.model_mut(OUTPUT_SCHEME, path).expect("the output editor shows the channel");
                let end = document_end(&model.document().lines);
                let change = model.execute_edits(vec![(end, end, text, false)], None, EditReason::ApplyEdits);
                model.mark_saved();
                if let Some(change) = change {
                    self.send_model_edit(OUTPUT_SCHEME, path, &change);
                }
            }
            Content::Reset(text) => {
                let shown = self.output.filters.shown_text(&text, id);
                self.output_view.text = text;
                self.replace_model_text(OUTPUT_SCHEME, path, shown, true, cx);
            }
        }
        if !self.output_view.scroll_lock {
            self.reveal_output_end();
        }
        cx.notify();
    }

    /// `FilterController.filter` on `filters.onDidChange`: the shown text is the channel's text without the lines
    /// the filters hide.
    pub(super) fn output_filters_changed(&mut self, cx: &mut Context<Self>) {
        cx.notify();
        let Some(id) = self.output.active.clone().filter(|id| self.output_editor_shows(id)) else { return };
        let shown = self.output.filters.shown_text(&self.output_view.text, &id);
        self.replace_model_text(OUTPUT_SCHEME, Path::new(&id), shown, true, cx);
        if !self.output_view.scroll_lock {
            self.reveal_output_end();
        }
    }

    /// The view's title actions in the panel title: the filter, the channel dropdown, Clear
    /// Output, Toggle Auto Scrolling and the log level gear (output.contribution.ts `MenuId.ViewTitle` items).
    // shortcut: no "..." menu; its Save Output As, Export Logs, Add Compound Log and Import Log are not ported.
    pub(super) fn output_title_actions(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let channel = self.output.active_channel();
        let label = channel.map(|channel| channel.label.clone()).unwrap_or_default();
        let level_settable = channel.is_some_and(|channel| channel.log);
        let scroll_icon = if self.output_view.scroll_lock { "unlock" } else { "lock" };
        div()
            .flex()
            .items_center()
            .gap(px(4.))
            .child(self.output_filter_box(window, cx))
            .child(self.output_select_box(label, cx))
            .child(self.search_action("clear-all", "workbench.output.action.clearOutput", true, cx))
            .child(self.search_action(scroll_icon, "workbench.output.action.toggleAutoScroll", true, cx))
            .when(level_settable, |actions| actions.child(self.output_dropdown_action("gear", self.output_view.level_menu_bounds.clone(), Workbench::show_output_log_level_menu, cx)))
    }

    /// viewFilter.ts `FilterWidget` in a panel title (views.css): 150px to 200px, 400px while focused, 10px right
    /// margin; the filter dropdown at the input's right shows as checked while level or category filters hide lines.
    fn output_filter_box(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let focused = self.output_view.filter_focus.is_focused(window);
        let height = if self.theme.kind == ThemeKind::Light { 25. } else { 24. };
        let border = if focused { self.color("focusBorder") } else { self.theme.color("input.border").unwrap_or(self.color("input.background")) };
        let checked = self.output.filters.more_filters(self.output.active.as_deref());
        let foreground = if checked { self.foreground_or("inputOption.activeForeground") } else { self.foreground_or("icon.foreground") };
        let bounds = self.output_view.filter_menu_bounds.clone();
        let filter_button = self
            .icon("filter", 20., 16., foreground)
            .id("output-filter-menu")
            .relative()
            .rounded(px(4.))
            .border_1()
            .border_color(gpui::transparent_black())
            .cursor_pointer()
            .when(checked, |button| button.bg(self.color("inputOption.activeBackground")).border_color(self.color("inputOption.activeBorder")))
            .when(!checked, |button| button.hover(|button| button.bg(self.color("toolbar.hoverBackground"))))
            .child(bounds_tracker(bounds.clone()))
            .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, _: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                workbench.show_output_filter_menu(bounds.get(), window, cx);
            }));
        let placeholder = Some("Filter (e.g. text, !excludeText, text1,text2)".to_owned());
        div()
            .flex_1()
            .min_w(px(150.))
            .max_w(px(if focused { 400. } else { 200. }))
            .mr(px(10.))
            .h(px(height))
            .flex()
            .items_center()
            .pl(px(4.))
            .pr(px(2.))
            .border_1()
            .rounded(px(4.))
            .cursor_text()
            .text_size(px(12.))
            .bg(self.color("input.background"))
            .border_color(border)
            .text_color(self.foreground_or("input.foreground"))
            .child(div().flex_1().min_w_0().h_full().flex().child(self.input_text(InputId::OutputFilter, InputLook { password: false, placeholder, focused }, cx)))
            .child(filter_button)
    }

    /// selectBox.css `.monaco-select-box` in an action bar on macOS: 11px text, 24px high, 100px wide at least.
    fn output_select_box(&self, label: String, cx: &mut Context<Self>) -> impl IntoElement {
        let bounds = self.output_view.channel_menu_bounds.clone();
        let foreground = self.foreground_or("dropdown.foreground");
        div()
            .id("output-channel-select")
            .relative()
            .min_w(px(100.))
            .h(px(24.))
            .flex()
            .items_center()
            .pl(px(8.))
            .pr(px(23.))
            .border_1()
            .rounded(px(4.))
            .cursor_pointer()
            .text_size(px(11.))
            .whitespace_nowrap()
            .bg(self.color("dropdown.background"))
            .border_color(self.color("dropdown.border"))
            .text_color(foreground)
            .child(label)
            .child(self.icon("chevron-down", 16., 16., foreground).absolute().right(px(4.)))
            .child(bounds_tracker(bounds.clone()))
            .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, _: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                workbench.show_output_channel_menu(bounds.get(), window, cx);
            }))
    }

    /// A title action that opens a submenu below itself.
    fn output_dropdown_action(&self, icon: &'static str, bounds: Rc<Cell<Bounds<Pixels>>>, show: fn(&mut Workbench, Bounds<Pixels>, &mut Window, &mut Context<Workbench>), cx: &mut Context<Self>) -> impl IntoElement {
        let hover = self.color("toolbar.hoverBackground");
        self.icon(icon, 22., 16., self.foreground_or("icon.foreground"))
            .id(icon)
            .relative()
            .rounded(px(4.))
            .cursor_pointer()
            .hover(|action| action.bg(hover))
            .child(bounds_tracker(bounds.clone()))
            .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, _: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                show(workbench, bounds.get(), window, cx);
            }))
    }

    /// `OutputViewPane.onDidChangeContent`: the view follows the end of the text.
    fn reveal_output_end(&mut self) {
        if self.output_editor().is_none() {
            return;
        }
        self.with_group(PANEL_GROUP, |workbench| {
            let viewport = workbench.editors.viewport;
            let editor = &mut workbench.editors.editors[0];
            let end = document_end(&editor.document().lines);
            editor.reveal_range(end, end, crate::editor::VerticalReveal::Simple, viewport);
        });
    }

    fn start_output_poll(&mut self, cx: &mut Context<Self>) {
        if self.output_view.polling {
            return;
        }
        self.output_view.polling = true;
        cx.spawn(async move |workbench, cx| {
            loop {
                cx.background_executor().timer(POLL_INTERVAL).await;
                let visible = workbench.update(cx, |workbench, cx| workbench.poll_output(cx));
                if !matches!(visible, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// One `FileContentProvider.doWatch` of the shown channel; false once the view is hidden, which ends the poll.
    fn poll_output(&mut self, cx: &mut Context<Self>) -> bool {
        self.sync_visible_output_channel();
        if !self.output_view_visible() {
            self.output_view.polling = false;
            return false;
        }
        let Some(id) = self.output.active.clone().filter(|id| self.output_editor_shows(id)) else { return true };
        let content = self.output.channel_mut(&id).expect("the active channel is registered").read_appended();
        self.apply_output_content(&id, content, cx);
        true
    }

    /// mainThreadOutputService.ts `setVisibleChannel`: the host flushes a visible channel on every write.
    fn sync_visible_output_channel(&mut self) {
        let visible = self.output.active.clone().filter(|_| self.output_view_visible());
        if visible == self.output_view.visible_channel_sent {
            return;
        }
        self.host.send("ExtHostOutputService", "$setVisibleChannel", vec![visible.clone().map_or(Value::Null, |id| json!(id))]);
        self.output_view.visible_channel_sent = visible;
    }
}

/// Records where its parent painted.
pub(super) fn bounds_tracker(bounds: Rc<Cell<Bounds<Pixels>>>) -> impl IntoElement {
    canvas(move |painted, _, _| bounds.set(painted), |_, _, _, _| {}).absolute().size_full()
}
