//! The Explorer of a window without a folder (emptyView.ts) with its welcome content (explorerViewlet.ts
//! `noFolderHelp`, `noFolderButEditorsHelp`).

use gpui::{Context, Div, InteractiveElement, InteractiveText, ParentElement, StatefulInteractiveElement, StyledText, Styled, div, prelude::FluentBuilder, px};

use super::Workbench;

const OPEN_FOLDER: &str = "workbench.action.files.openFolder";

impl Workbench {
    /// The body of the Explorer's first pane without a folder: the welcome content (viewPane.ts `WelcomeView`).
    pub(super) fn empty_view(&self, cx: &mut Context<Self>) -> Div {
        let foreground = self.foreground_or("sideBar.foreground");
        let paragraph = |text: &'static str| div().mt(px(13.)).line_height(px(18.2)).child(text);
        // noFolderButEditorsHelp shows while editors are open (`editorIsOpen`).
        let editors_open = !self.editors.editors.is_empty();
        div()
            .px(px(20.))
            .pb(px(13.))
            .text_size(px(13.))
            .text_color(foreground)
            .child(paragraph("You have not yet opened a folder."))
            .child(self.welcome_button("Open Folder", OPEN_FOLDER, cx))
            .when(editors_open, |content| content.child(div().mt(px(13.)).line_height(px(18.2)).child(self.add_a_folder_text(cx))))
    }

    /// `noFolderButEditorsHelp`'s last line with its "add a folder" link.
    // shortcut: `workbench.action.addRootFolder` (untitled multi-root workspaces) is not ported, so the link runs a missing command.
    fn add_a_folder_text(&self, cx: &mut Context<Self>) -> InteractiveText {
        let before = "Opening a folder will close all currently open editors. To keep them open, ";
        let link = before.len()..before.len() + "add a folder".len();
        let text = format!("{before}add a folder instead.");
        let style = gpui::HighlightStyle { color: Some(self.color("textLink.foreground").into()), ..Default::default() };
        let weak = cx.entity().downgrade();
        InteractiveText::new("add-a-folder", StyledText::new(text).with_highlights([(link.clone(), style)])).on_click(vec![link], move |_, window, cx| {
            let _ = weak.update(cx, |workbench, cx| workbench.execute_command("workbench.action.addRootFolder", Vec::new(), window, cx));
        })
    }

    /// A welcome view's `monaco-text-button`: full width, 13px under the paragraph above.
    fn welcome_button(&self, label: &'static str, command: &'static str, cx: &mut Context<Self>) -> Div {
        let hover = self.color("button.hoverBackground");
        div().mt(px(13.)).child(
            div()
                .id(command)
                .h(px(26.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .text_size(px(12.))
                .line_height(px(16.))
                .cursor_pointer()
                .bg(self.color("button.background"))
                .text_color(self.color("button.foreground"))
                .when_some(self.theme.color("button.border"), |button, border| button.border_1().border_color(border))
                .hover(move |button| button.bg(hover))
                .child(label)
                .on_click(cx.listener(move |workbench, _, window, cx| {
                    workbench.execute_command(command, Vec::new(), window, cx);
                })),
        )
    }
}
