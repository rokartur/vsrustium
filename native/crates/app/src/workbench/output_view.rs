//! The Output view (outputView.ts `OutputViewPane` with output.contribution.ts): the active output channel's text in
//! a read-only editor.
// shortcut: the view is an editor tab of the group, not a panel view; the panel part is milestone 11's.

use std::path::Path;
use std::time::Duration;

use gpui::{Context, Window};
use serde_json::{Value, json};

use super::Workbench;
use super::bulk_text_edits::document_end;
use crate::commands::Outcome;
use crate::editor::{EditReason, TextDocument};
use crate::output::{Content, OUTPUT_SCHEME};

#[cfg(test)]
mod tests;

/// output.ts `OUTPUT_VIEW_ID`.
pub const OUTPUT_VIEW_ID: &str = "workbench.panel.output";

/// outputChannelModel.ts `FileContentProvider` polls its file every 500 ms (`ThrottledDelayer(500)`).
const POLL_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Default)]
pub struct OutputView {
    /// A task reads the shown channel's file while the view is visible.
    polling: bool,
    /// The channel `$setVisibleChannel` last told the host about.
    visible_channel_sent: Option<String>,
}

impl Workbench {
    pub(super) fn register_output_actions(&mut self) {
        // viewsService.ts `OpenViewAction` of the view's `openCommandActionDescriptor`.
        self.commands.register("workbench.action.output.toggleOutput", |workbench, args, window, cx| {
            let preserve_focus = args.first().is_some_and(|options| options["preserveFocus"] == true);
            if workbench.output_view_visible() && workbench.editor_focus.is_focused(window) && !preserve_focus {
                workbench.close_output_view(cx);
            } else {
                workbench.open_output_view(!preserve_focus, window, cx);
            }
            Outcome::done()
        });
    }

    /// The tab that hosts the Output view, if it is open.
    pub(super) fn output_editor_index(&self) -> Option<usize> {
        self.editors.editors.iter().position(|editor| editor.document.scheme == OUTPUT_SCHEME)
    }

    /// `isViewVisible(OUTPUT_VIEW_ID)`: the group shows the Output view's tab.
    pub(crate) fn output_view_visible(&self) -> bool {
        self.editors.active().is_some_and(|editor| editor.document.scheme == OUTPUT_SCHEME)
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
        let document = TextDocument::from_text(OUTPUT_SCHEME, path, &text, "utf8".into(), Some(&language), &self.languages, &self.settings.lock().unwrap());
        self.output.active = Some(id.to_owned());
        match self.output_editor_index() {
            Some(index) if self.editors.editors[index].document.path == path => self.editors.activate(index),
            Some(index) => self.editors.replace(index, document),
            None => self.editors.open(document, false),
        }
        self.reveal_output_end();
        self.editors_changed(cx);
        if !preserve_focus {
            window.focus(&self.editor_focus, cx);
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
        let Some(index) = self.output_editor_index() else { return };
        self.editors.close(index);
        self.editors_changed(cx);
        self.sync_visible_output_channel();
    }

    /// Channels were registered or removed: a removed shown channel gives way to the active one
    /// (outputServices.ts `onDidRemoveChannel`).
    pub(crate) fn output_channels_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.output_editor_index() else { return };
        let shown = self.editors.editors[index].document.path.to_string_lossy().into_owned();
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
        self.output_editor_index().is_some_and(|index| self.editors.editors[index].document.path == Path::new(id))
    }

    fn apply_output_content(&mut self, id: &str, content: Content, cx: &mut Context<Self>) {
        let path = Path::new(id);
        match content {
            Content::Append(text) if text.is_empty() => return,
            Content::Append(text) => {
                let model = self.editors.model_mut(OUTPUT_SCHEME, path).expect("the output editor shows the channel");
                let end = document_end(&model.document.lines);
                let change = model.execute_edits(vec![(end, end, text, false)], None, EditReason::ApplyEdits);
                model.mark_saved();
                if let Some(change) = change {
                    self.send_model_edit(OUTPUT_SCHEME, path, &change);
                }
            }
            Content::Reset(text) => self.replace_model_text(OUTPUT_SCHEME, path, text, true, cx),
        }
        self.reveal_output_end();
        cx.notify();
    }

    /// `OutputViewPane.onDidChangeContent`: the view follows the end of the text.
    fn reveal_output_end(&mut self) {
        let viewport = self.editors.viewport;
        let Some(index) = self.output_editor_index() else { return };
        let editor = &mut self.editors.editors[index];
        let end = document_end(&editor.document.lines);
        editor.reveal_range(end, end, crate::editor::VerticalReveal::Simple, viewport);
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
