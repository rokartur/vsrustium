//! The accessible view and accessibility help (accessibility/browser/accessibleView.ts, accessibleViewActions.ts):
//! a read-only text view over the workbench that a screen reader reads line by line. Help (Alt+F1) comes from
//! the editor (editorAccessibilityHelp.ts), the hover (hoverAccessibleViews.ts) or the shown view itself
//! (`showAccessibleViewHelp`); the view (Alt+F2) from a focused notification (notificationAccessibleView.ts).

use std::collections::HashMap;

use gpui::{ClickEvent, ClipboardItem, Context, FocusHandle, Keystroke, Role, ScrollWheelEvent, Window, div, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use super::content_hover::PartContent;
use crate::commands::Outcome;
use crate::editor::{Position, Selection, cursor};
use crate::main_thread_quick_open::Owner;
use crate::markdown::{MarkdownString, parse, plain_text};
use crate::notifications::{Action, Severity};
use crate::quick_input::{Entry, Event, Item, Kind, QuickInput};

/// accessibleView.ts `DIMENSIONS`.
const MAX_WIDTH: f32 = 900.;
const WIDTH_RATIO: f32 = 0.75;
const MAX_HEIGHT_RATIO: f32 = 0.6;
const LINE_HEIGHT: f32 = 18.;
/// The editor's `padding: { top: 2, bottom: 2 }` and `lineDecorationsWidth: 6`.
const PADDING: f32 = 2.;
const LINE_DECORATIONS_WIDTH: f32 = 6.;
const TITLE_HEIGHT: f32 = 24.;
/// `layoutService.activeContainerOffset.quickPickTop` with the command center.
const TOP: f32 = 6.;
const READ_MORE_URL: &str = "https://go.microsoft.com/fwlink/?linkid=851010";

pub struct AccessibleViewState {
    shown: Option<Shown>,
    pub focus: FocusHandle,
    toolbar_focus: FocusHandle,
    previous_focus: Option<FocusHandle>,
    /// `_lastProviderPosition`: where the cursor was when each provider's view closed.
    last_positions: HashMap<&'static str, Position>,
}

impl AccessibleViewState {
    pub fn new(cx: &mut Context<Workbench>) -> Self {
        AccessibleViewState { shown: None, focus: cx.focus_handle(), toolbar_focus: cx.focus_handle(), previous_focus: None, last_positions: HashMap::new() }
    }
}

/// `Owner::AccessibleViewKeybindings`: the help's command ids by item handle and the help to show again.
pub struct KeybindingPick {
    provider: Provider,
    commands: Vec<String>,
    accepted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Provider {
    Editor,
    Hover,
    Notification(u64),
    Comments,
    /// terminalAccessibleBufferProvider.ts: the terminal instance's buffer.
    Terminal(u32),
    /// terminalAccessibilityHelp.ts.
    TerminalHelp(u32),
}

impl Provider {
    /// `AccessibleViewProviderId`.
    fn id(self) -> &'static str {
        match self {
            Provider::Editor => "editor",
            Provider::Hover => "hover",
            Provider::Notification(_) => "notification",
            Provider::Comments => "comments",
            Provider::Terminal(_) => "terminal",
            Provider::TerminalHelp(_) => "terminal-help",
        }
    }

    fn verbosity_key(self) -> &'static str {
        match self {
            Provider::Editor => "accessibility.verbosity.editor",
            Provider::Hover => "accessibility.verbosity.hover",
            Provider::Notification(_) => "accessibility.verbosity.notification",
            Provider::Comments => "accessibility.verbosity.comments",
            Provider::Terminal(_) | Provider::TerminalHelp(_) => "accessibility.verbosity.terminal",
        }
    }
}

struct Shown {
    provider: Provider,
    help: bool,
    /// `showAccessibleViewHelp`: Escape shows the provider's view again instead of closing.
    view_help: bool,
    lines: Vec<String>,
    selection: Selection,
    /// The visible column `cursorUp`/`cursorDown` keep.
    leftover: isize,
    top_line: usize,
    left_column: usize,
    unassigned: Vec<String>,
    assigned: Vec<String>,
    /// The toolbar action the keyboard is on, while the toolbar has focus.
    toolbar_index: usize,
}

/// A `MenuId.AccessibleView` item or a provider action: title, codicon and what it runs.
struct ToolbarAction {
    title: String,
    icon: &'static str,
    run: ToolbarRun,
}

enum ToolbarRun {
    Command(&'static str),
    Notification(u64, Action),
}

impl Workbench {
    pub(super) fn register_accessible_view_actions(&mut self) {
        // accessibleViewActions.ts `AccessibilityHelpAction`: the implementations by priority, the first whose `when` holds.
        self.commands.register("editor.action.accessibilityHelp", |workbench, _, window, cx| {
            if workbench.accessibility.view.shown.as_ref().is_some_and(|shown| !shown.help) {
                workbench.show_accessible_view_help(window, cx);
            } else if workbench.comments.reply_focus.is_focused(window) {
                workbench.show_accessible_view(Provider::Comments, true, window, cx);
            } else if workbench.content_hover_visible() {
                workbench.show_accessible_view(Provider::Hover, true, window, cx);
            } else if workbench.terminal_view.focus.is_focused(window) || workbench.accessibility.view.shown.as_ref().is_some_and(|shown| matches!(shown.provider, Provider::Terminal(_))) {
                // terminal.accessibility.contribution.ts `TerminalAccessibilityHelpContribution` (priority 105).
                if let Some(id) = workbench.terminal_view.active_instance_id() {
                    workbench.show_accessible_view(Provider::TerminalHelp(id), true, window, cx);
                }
            } else {
                if workbench.editors.active().is_none() {
                    let outcome = workbench.execute_command("workbench.action.files.newUntitledFile", Vec::new(), window, cx);
                    super::quick_access::show_failure("New Untitled Text File".into(), outcome, window, cx);
                }
                if workbench.editors.active().is_some() {
                    workbench.show_accessible_view(Provider::Editor, true, window, cx);
                }
            }
            Outcome::done()
        });
        self.commands.register("editor.action.accessibleView", |workbench, _, window, cx| {
            if let Some(key) = workbench.notification_focused(window) {
                workbench.show_accessible_view(Provider::Notification(key), false, window, cx);
            } else if workbench.terminal_view.focus.is_focused(window) {
                workbench.show_terminal_accessible_buffer(window, cx);
            }
            Outcome::done()
        });
        // terminal.accessibility.contribution.ts.
        self.commands.register("workbench.action.terminal.focusAccessibleBuffer", |workbench, _, window, cx| {
            if workbench.terminal_view.instances.is_empty() {
                workbench.create_terminal(None, false, window, cx);
            }
            workbench.show_terminal_accessible_buffer(window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.terminal.accessibleBufferGoToNextCommand", |workbench, _, window, cx| {
            workbench.accessible_buffer_go_to_command(true, window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.terminal.accessibleBufferGoToPreviousCommand", |workbench, _, window, cx| {
            workbench.accessible_buffer_go_to_command(false, window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.terminal.scrollToBottomAccessibleView", |workbench, _, window, cx| {
            workbench.set_terminal_accessible_view_line(None, window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.terminal.scrollToTopAccessibleView", |workbench, _, window, cx| {
            workbench.set_terminal_accessible_view_line(Some(0), window, cx);
            Outcome::done()
        });
        self.commands.register("editor.action.accessibleViewDisableHint", |workbench, _, _, cx| {
            let Some(shown) = &workbench.accessibility.view.shown else { return Outcome::done() };
            let key = shown.provider.verbosity_key();
            workbench.update_user_setting(key, json!(false));
            workbench.aria_alert(&format!("{key} accessibility verbosity is now disabled"));
            cx.notify();
            Outcome::done()
        });
        self.commands.register("editor.action.accessibilityHelpOpenHelpLink", |workbench, _, _, cx| {
            if workbench.accessibility.view.shown.as_ref().is_some_and(|shown| shown.provider == Provider::Editor && shown.help) {
                cx.open_url(READ_MORE_URL);
            }
            Outcome::done()
        });
        self.commands.register("editor.action.accessibilityHelpConfigureKeybindings", |workbench, _, window, cx| {
            workbench.configure_accessible_view_keybindings(true, window, cx);
            Outcome::done()
        });
        self.commands.register("editor.action.accessibilityHelpConfigureAssignedKeybindings", |workbench, _, window, cx| {
            workbench.configure_accessible_view_keybindings(false, window, cx);
            Outcome::done()
        });
        self.commands.register("editor.action.accessibleViewNext", |workbench, _, window, cx| {
            workbench.accessible_view_navigate(true, window, cx);
            Outcome::done()
        });
        self.commands.register("editor.action.accessibleViewPrevious", |workbench, _, window, cx| {
            workbench.accessible_view_navigate(false, window, cx);
            Outcome::done()
        });
    }

    /// The context keys of accessibilityConfiguration.ts the view sets.
    pub(super) fn accessible_view_context(&self, context: &mut HashMap<String, Value>) {
        let Some(shown) = &self.accessibility.view.shown else { return };
        context.insert("accessibleViewIsShown".into(), json!(!shown.help));
        context.insert("accessibilityHelpIsShown".into(), json!(shown.help));
        context.insert("accessibleViewCurrentProviderId".into(), json!(shown.provider.id()));
        context.insert("accessibleViewVerbosityEnabled".into(), json!(self.setting(shown.provider.verbosity_key()) == json!(true)));
        context.insert("accessibleViewSupportsNavigation".into(), json!(!shown.help && matches!(shown.provider, Provider::Notification(_))));
        context.insert("accessibleViewHasUnassignedKeybindings".into(), json!(!shown.unassigned.is_empty()));
        context.insert("accessibleViewHasAssignedKeybindings".into(), json!(!shown.assigned.is_empty()));
        context.insert("accessibleViewOnLastLine".into(), json!(shown.selection.head.line + 1 == shown.lines.len()));
    }

    fn notification_focused(&self, window: &Window) -> Option<u64> {
        let ui = &self.notifications;
        ui.focus.is_focused(window).then_some(ui.focused).flatten().filter(|key| ui.model.item(*key).is_some())
    }

    /// `AccessibleView.show` and `_render`: the content, the cursor where this provider's view last closed, and
    /// focus in the view.
    fn show_accessible_view(&mut self, provider: Provider, help: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(content) = self.accessible_view_content(provider, help, false, window) else { return };
        self.show_accessible_view_content(provider, help, false, content, window, cx);
    }

    fn show_accessible_view_content(&mut self, provider: Provider, help: bool, view_help: bool, (text, unassigned, assigned): (String, Vec<String>, Vec<String>), window: &mut Window, cx: &mut Context<Self>) {
        let lines: Vec<String> = text.split('\n').map(str::to_owned).collect();
        let same = self.accessibility.view.shown.as_ref().is_some_and(|shown| shown.provider.id() == provider.id() && shown.help == help);
        let position = match &self.accessibility.view.shown {
            Some(shown) if same => Some(shown.selection.head),
            _ => self.accessibility.view.last_positions.get(provider.id()).copied(),
        };
        let position = position.filter(|position| position.line < lines.len()).map(|position| Position::new(position.line, position.column.min(cursor::line_len(&lines, position.line)))).unwrap_or_default();
        if self.accessibility.view.shown.is_none() {
            self.accessibility.view.previous_focus = window.focused(cx);
        }
        self.accessibility.view.shown = Some(Shown { provider, help, view_help, lines, selection: Selection::caret(position), leftover: 0, top_line: 0, left_column: 0, unassigned, assigned, toolbar_index: 0 });
        self.reveal_accessible_view_cursor(window);
        window.focus(&self.accessibility.view.focus, cx);
        cx.notify();
    }

    /// `_updateContent`: a view's content as the provider gives it; help with the screen reader mode hint before
    /// it and the read more, verbosity and exit hints after it, `<keybinding:id>` resolved, then the configure hints.
    fn accessible_view_content(&self, provider: Provider, help: bool, view_help: bool, window: &Window) -> Option<(String, Vec<String>, Vec<String>)> {
        let content = match (provider, view_help) {
            // The terminal buffer's `customHelp` is the terminal's accessibility help.
            (Provider::Terminal(id), true) => self.terminal_accessibility_help(id),
            (_, true) => self.accessible_view_help_dialog_content(provider),
            (Provider::Terminal(id), _) => self.terminal_view.instance(id)?.accessible_buffer().lines.join("\n"),
            (Provider::TerminalHelp(id), _) => self.terminal_accessibility_help(id),
            (Provider::Editor, _) => self.editor_accessibility_help(),
            (Provider::Hover, _) => self.hover_accessible_content()?,
            (Provider::Comments, _) => super::commenting_ranges::COMMENTS_ACCESSIBILITY_HELP.join("\n"),
            (Provider::Notification(key), _) => self.notification_accessible_content(key)?,
        };
        if !help {
            return Some((content, Vec::new(), Vec::new()));
        }
        let verbose = self.setting(provider.verbosity_key()) == json!(true);
        let mut text = self.screen_reader_mode_hint(provider) + &content;
        if provider == Provider::Editor && !view_help {
            text += "\nOpen a browser window with more information related to accessibility<keybinding:editor.action.accessibilityHelpOpenHelpLink>.";
        }
        if verbose {
            text += "\nDisable accessibility verbosity for this feature<keybinding:editor.action.accessibleViewDisableHint>.";
            text += "\nExit this dialog (Escape).";
        }
        let context = self.key_context(window);
        let aria_label = |command: &str| self.keybindings.resolver.lookup_primary_keybinding(command, &context).map(|item| crate::keybindings::aria_label(&item.keybinding));
        let (mut text, unassigned, assigned) = resolve_keybindings(&text, aria_label);
        if !unassigned.is_empty() {
            let keybinding = aria_label("editor.action.accessibilityHelpConfigureKeybindings").map_or("by assigning a keybinding to the command Accessibility Help Configure Unassigned Keybindings.".into(), |label| format!("({label})"));
            text += &format!("\nConfigure keybindings for commands that lack them {keybinding}.");
        }
        if !assigned.is_empty() {
            let keybinding = aria_label("editor.action.accessibilityHelpConfigureAssignedKeybindings").map_or("by assigning a keybinding to the command Accessibility Help Configure Assigned Keybindings.".into(), |label| format!("({label})"));
            text += &format!("\nConfigure keybindings for commands that already have assignments {keybinding}.");
        }
        Some((text, unassigned, assigned))
    }

    /// `_screenReaderModeHint`.
    fn screen_reader_mode_hint(&self, provider: Provider) -> String {
        let optimized = self.is_screen_reader_optimized();
        if optimized && provider == Provider::Editor {
            "The application is configured to be optimized for usage with a Screen Reader.\n".into()
        } else if !optimized {
            "The application is configured to never be optimized for usage with a Screen Reader.\nConfigure the application to be optimized for usage with a Screen Reader (Command+E).\n".into()
        } else {
            String::new()
        }
    }

    /// editorAccessibilityHelp.ts `provideContent`, without its chat and dictation parts: no AI by decision, and no
    /// speech provider in the port.
    fn editor_accessibility_help(&self) -> String {
        let read_only = self.active_editor_readonly().is_some();
        let mut content = vec![match (self.editors.active_diff().is_some(), read_only) {
            (true, true) => "You are in a read-only pane of a diff editor.",
            (true, false) => "You are in a pane of a diff editor.",
            (false, true) => "You are in a read-only code editor.",
            (false, false) => "You are in a code editor.",
        }];
        content.push(match self.is_screen_reader_optimized() && self.setting("accessibility.windowTitleOptimized") == json!(true) {
            true => "activeEditorState - such as modified, problems, and more, is included as a part of the window.title setting by default. Disable it with accessibility.windowTitleOptimized.",
            false => "activeEditorState - such as modified, problems, and more, is currently not included as a part of the window.title setting by default. Enable it with accessibility.windowTitleOptimized.",
        });
        content.extend([
            "In the simple file dialog, the pre-filled path is selected when a screen reader is detected, so typing replaces it. Change this with the files.simpleDialog.selectInitialPath setting.",
            "Around the workbench, when the screen reader announces you've landed in a toolbar, use narrow keys to navigate between the toolbar's actions.",
            "In the Reopen Editor With menu, hold Alt to show alternatives labelled with (Open to the Side) after the editor name. On Windows and Linux, Shift also works. Native menus select the alternative when the menu opens; custom menus update while open. Click an entry or press Enter to run the displayed action. Editors that cannot be split do not offer this alternative for their current type. Switching editor types preserves the selection when supported by the editor.",
            "Run the command: List Signal Sounds for an overview of all sounds and their current status.",
            "Run the command: List Signal Announcements for an overview of announcements and their current status.",
            "Run the command: Announce Cursor Position<keybinding:editor.action.announceCursorPosition> to hear the current line and column.",
            "Focus notification toasts<keybinding:notifications.focusToasts> to navigate them with the keyboard. Accept the primary action of a focused notification<keybinding:notification.acceptPrimaryAction>.",
        ]);
        if let Some(info) = self.comment_command_info() {
            content.push(info);
        }
        content.extend([
            "Trigger the suggest widget<keybinding:editor.action.triggerSuggest> to show possible inline suggestions.",
            "Accept suggestion<keybinding:acceptSelectedSuggestion> to accept the currently selected suggestion.",
            "Toggle focus between the suggest widget and the editor<keybinding:focusSuggestion> and toggle details focus with<keybinding:toggleSuggestionFocus> to learn more about the suggestion.",
        ]);
        if self.setting("editor.stickyScroll.enabled") == json!(true) {
            content.push("Focus Sticky Scroll<keybinding:editor.action.focusStickyDebugConsole> to focus the currently nested scopes.");
        }
        content.push(match self.tab_focus_mode() {
            true => "Pressing Tab in the current editor will move focus to the next focusable element. Toggle this behavior<keybinding:editor.action.toggleTabFocusMode>.",
            false => "Pressing Tab in the current editor will insert the tab character. Toggle this behavior<keybinding:editor.action.toggleTabFocusMode>.",
        });
        content.extend([
            "Use code folding to collapse blocks of code and focus on the code you're interested in via the Toggle Folding Command<keybinding:editor.toggleFold>.",
            "Use Intellisense to improve coding efficiency and reduce errors. Trigger suggestions<keybinding:editor.action.triggerSuggest>.",
            "Show or focus the hover<keybinding:editor.action.showHover> to read information about the current symbol.",
            "Go to Symbol<keybinding:workbench.action.gotoSymbol> to quickly navigate between symbols in the current file.",
            "The Debug: Start Debugging command<keybinding:workbench.action.debug.start> will start a debug session.",
            "The Debug: Inline Breakpoint command<keybinding:editor.debug.action.toggleInlineBreakpoint> will set or unset a breakpoint at the current cursor position in the active editor.",
            "The Debug: Execute Selection command<keybinding:editor.debug.action.selectionToRepl> will execute the selected text in the debug console.",
            "The Debug: Add to Watch command<keybinding:editor.debug.action.selectionToWatch> will add the selected text to the watch view.",
        ]);
        content.join("\n")
    }

    /// contentHoverRendered.ts `getAccessibleContent`: each part's text, in hover order, a blank line apart.
    fn hover_accessible_content(&self) -> Option<String> {
        let shown = self.content_hover.shown.as_ref()?;
        let mut parts: Vec<_> = shown.parts.iter().collect();
        parts.sort_by_key(|part| (part.participant, part.ordinal));
        let mut texts = Vec::new();
        for part in parts {
            texts.push(match &part.content {
                PartContent::Marker(marker) => super::content_hover_view::marker_accessible_content(marker),
                PartContent::Markdown(contents) => contents.iter().filter(|content| !crate::markdown::is_empty(content)).map(|content| plain_text(&parse(&MarkdownString::from_value(content)))).collect::<Vec<_>>().join("\n"),
                PartContent::ColorPicker => "There is a color picker here.".to_owned(),
            });
        }
        Some(texts.join("\n\n"))
    }

    /// notificationAccessibleView.ts `getContentForNotification`.
    fn notification_accessible_content(&self, key: u64) -> Option<String> {
        let item = self.notifications.model.item(key)?;
        let message = item.message.raw.clone();
        let message = match &item.source {
            Some(source) => format!("{message} Source: {source}"),
            None => message,
        };
        Some(with_severity_prefix(&message, item.severity))
    }

    /// notificationsList.ts `NotificationAccessibilityProvider.getAriaLabel`: the message, its source and, with
    /// `accessibility.verbosity.notification`, how to open it in the accessible view.
    pub(super) fn notification_aria_label(&self, item: &crate::notifications::Item, window: &Window) -> String {
        let hint = (self.setting("accessibility.verbosity.notification") == json!(true)).then(|| {
            let context = self.key_context(window);
            match self.keybindings.resolver.lookup_primary_keybinding("editor.action.accessibleView", &context) {
                Some(item) => format!("Inspect the response in the accessible view with {}", crate::keybindings::aria_label(&item.keybinding)),
                None => "Inspect the response in the accessible view via the command Open Accessible View which is currently not triggerable via keybinding".into(),
            }
        });
        let message = &item.message.raw;
        let label = match (&item.source, hint) {
            (None, Some(hint)) => format!("{message}, notification, {hint}"),
            (None, None) => format!("{message}, notification"),
            (Some(source), Some(hint)) => format!("{message}, source: {source}, notification, {hint}"),
            (Some(source), None) => format!("{message}, source: {source}, notification"),
        };
        with_severity_prefix(&label, item.severity)
    }

    /// `TerminalAccessibleViewContribution.show` with the `terminal.integrated.accessibleViewPreserveCursorPosition`
    /// position: the bottom line, or with the setting on, where the view last closed.
    fn show_terminal_accessible_buffer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.terminal_view.active_instance_id() else { return };
        let provider = Provider::Terminal(id);
        let Some(content) = self.accessible_view_content(provider, false, false, window) else { return };
        let preserve = self.setting("terminal.integrated.accessibleViewPreserveCursorPosition") == json!(true);
        let remembered = preserve && self.accessibility.view.last_positions.contains_key(provider.id());
        self.show_accessible_view_content(provider, false, false, content, window, cx);
        if !remembered {
            self.set_terminal_accessible_view_line(None, window, cx);
        }
    }

    /// `setPosition(new Position(line, 1), true)`: the cursor at the line's start, revealed; the last line for none.
    fn set_terminal_accessible_view_line(&mut self, line: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(shown) = self.accessibility.view.shown.as_mut().filter(|shown| matches!(shown.provider, Provider::Terminal(_)) && !shown.help) else { return };
        let line = line.unwrap_or(shown.lines.len().saturating_sub(1));
        shown.selection = Selection::caret(Position::new(line, 0));
        shown.leftover = 0;
        self.reveal_accessible_view_cursor(window);
        cx.notify();
    }

    /// `navigateToCommand`: the cursor to the next or previous command's line, its command line announced.
    fn accessible_buffer_go_to_command(&mut self, next: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some((id, current)) = self.accessibility.view.shown.as_ref().and_then(|shown| match shown.provider {
            Provider::Terminal(id) if !shown.help => Some((id, shown.selection.head.line + 1)),
            _ => None,
        }) else {
            return;
        };
        let Some(instance) = self.terminal_view.instance(id) else { return };
        let commands = instance.accessible_buffer().commands;
        let target = match next {
            true => commands.into_iter().filter(|(line, _, _)| *line > current).min_by_key(|(line, _, _)| *line),
            false => commands.into_iter().filter(|(line, _, _)| *line < current).max_by_key(|(line, _, _)| *line),
        };
        let Some((line, command, _)) = target else { return };
        self.set_terminal_accessible_view_line(Some(line - 1), window, cx);
        if !command.is_empty() {
            self.aria_alert(&command);
        }
    }

    /// terminalAccessibilityHelp.ts `provideContent`, without the voice and Windows parts.
    fn terminal_accessibility_help(&self, id: u32) -> String {
        let mut content = vec![
            "The Focus Accessible Terminal View command<keybinding:workbench.action.terminal.focusAccessibleBuffer> enables screen readers to read terminal contents.".to_owned(),
            "Customize the behavior of the cursor when toggling between the terminal and accessible view with `terminal.integrated.accessibleViewPreserveCursorPosition.`".to_owned(),
            "The Open Detected Link command<keybinding:workbench.action.terminal.openDetectedLink> enables screen readers to easily open links found in the terminal.".to_owned(),
            "The Create New Terminal (With Profile) command<keybinding:workbench.action.terminal.newWithProfile> allows for easy terminal creation using a specific profile.".to_owned(),
            "Configure what gets focused after running selected text in the terminal with `terminal.integrated.focusAfterRun`.".to_owned(),
            "Change Icon opens a searchable icon grid. Enter a name to filter, then press Down Arrow to enter the grid. Arrow keys move between icons, Home and End move to the first and last icon, and Up Arrow from the top row returns to search. Tab and Shift+Tab move between search, the grid, and the Reset to default button below the grid. Press Enter to choose an icon or reset the default terminal icon without changing its color. Escape cancels without changing the icon.".to_owned(),
        ];
        if self.setting("terminal.integrated.accessibleViewFocusOnCommandExecution") != json!(true) {
            content.push("Enable `terminal.integrated.accessibleViewFocusOnCommandExecution` to automatically focus the terminal accessible view when a command is executed in the terminal.".into());
        }
        if self.setting("terminal.integrated.suggest.enabled") == json!(true) {
            content.extend([
                "The terminal request completions command can be invoked manually<keybinding:workbench.action.terminal.triggerSuggest>, but also appears while typing.",
                "When the terminal suggest widget is focused:",
                "- Accept the suggestion<keybinding:workbench.action.terminal.acceptSelectedSuggestion> and configure suggest settings<keybinding:workbench.action.terminal.configureSuggestSettings>.",
                "- Toggle between the widget and terminal<keybinding:workbench.action.terminal.suggestToggleDetails> and toggle details focus<keybinding:workbench.action.terminal.suggestToggleDetailsFocus> to learn more about the suggestion.",
                "- Learn more about the suggestion<keybinding:workbench.action.terminal.suggestLearnMore>.",
                "-Configure suggest settings<keybinding:workbench.action.terminal.configureSuggestSettings> ",
            ].map(str::to_owned));
        }
        match self.terminal_view.instance(id).is_some_and(|instance| instance.shell_integration.has_command_detection) {
            true => content.extend([
                "The terminal has a feature called shell integration that offers an enhanced experience and provides useful commands for screen readers such as:",
                "- Go to Next Command<keybinding:workbench.action.terminal.accessibleBufferGoToNextCommand> in the accessible view",
                "- Go to Previous Command<keybinding:workbench.action.terminal.accessibleBufferGoToPreviousCommand> in the accessible view",
                "- Go to Symbol<keybinding:editor.action.accessibleViewGoToSymbol>",
                "- Run Recent Command<keybinding:workbench.action.terminal.runRecentCommand>",
                "- Go to Recent Directory<keybinding:workbench.action.terminal.goToRecentDirectory>",
            ].map(str::to_owned)),
            false => content.push("Shell integration is not enabled. Some accessibility features may not be available.".into()),
        }
        content.join("\n")
    }

    /// `_accessibleViewHelpDialogContent`.
    fn accessible_view_help_dialog_content(&self, provider: Provider) -> String {
        let mut hint = "In the accessible view, you can:\n".to_owned();
        if matches!(provider, Provider::Notification(_)) {
            hint += " - Show the next item<keybinding:editor.action.accessibleViewNext> or previous item<keybinding:editor.action.accessibleViewPrevious>.\n";
        }
        hint += " - Navigate to the toolbar (Shift+Tab).\n";
        if matches!(provider, Provider::Notification(_)) {
            hint += " - If a notification contains links, they are available in the toolbar. Use Left and Right Arrow to find a link and Enter to open it.\n";
        }
        hint
    }

    /// `showAccessibleViewHelp`: the help of the shown view, which Escape leaves for the view again.
    fn show_accessible_view_help(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(provider) = self.accessibility.view.shown.as_ref().map(|shown| shown.provider) else { return };
        self.save_accessible_view_position();
        let content = self.accessible_view_content(provider, true, true, window).expect("help content always exists");
        self.show_accessible_view_content(provider, true, true, content, window, cx);
    }

    fn save_accessible_view_position(&mut self) {
        let Some(shown) = &self.accessibility.view.shown else { return };
        if !shown.view_help {
            self.accessibility.view.last_positions.insert(shown.provider.id(), shown.selection.head);
        }
    }

    /// The `hide` of `_render`: the provider's `onClose` puts focus back where it belongs.
    fn hide_accessible_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_accessible_view_position();
        let Some(shown) = self.accessibility.view.shown.take() else { return };
        cx.notify();
        if shown.view_help {
            self.show_accessible_view(shown.provider, false, window, cx);
            return;
        }
        match shown.provider {
            Provider::Notification(key) => {
                let outcome = self.execute_command("notifications.showList", Vec::new(), window, cx);
                super::quick_access::show_failure("Show Notifications".into(), outcome, window, cx);
                if self.notifications.model.item(key).is_some() {
                    self.notifications.focused = Some(key);
                }
                window.focus(&self.notifications.focus, cx);
            }
            Provider::Editor | Provider::Hover => window.focus(&self.editor_focus, cx),
            Provider::Comments => window.focus(&self.comments.reply_focus, cx),
            Provider::Terminal(_) | Provider::TerminalHelp(_) => window.focus(&self.terminal_view.focus, cx),
        }
        self.accessibility.view.previous_focus = None;
    }

    /// The notification provider's `provideNextContent`/`providePreviousContent`: the list moves its focus.
    fn accessible_view_navigate(&mut self, next: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Provider::Notification(key)) = self.accessibility.view.shown.as_ref().filter(|shown| !shown.help).map(|shown| shown.provider) else { return };
        let items = &self.notifications.model.items;
        let Some(index) = items.iter().position(|item| item.key == key) else { return };
        let target = if next { index + 1 } else { index.wrapping_sub(1) };
        let Some(target) = items.get(target).map(|item| item.key) else { return };
        let outcome = self.execute_command("notifications.showList", Vec::new(), window, cx);
        super::quick_access::show_failure("Show Notifications".into(), outcome, window, cx);
        self.notifications.focused = Some(target);
        let content = self.accessible_view_content(Provider::Notification(target), false, false, window).expect("the target notification exists");
        self.show_accessible_view_content(Provider::Notification(target), false, false, content, window, cx);
    }

    /// `configureKeybindings`: a quick pick of the help's command ids; accepting one opens the Keyboard Shortcuts
    /// editor on it.
    fn configure_accessible_view_keybindings(&mut self, unassigned: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(shown) = self.accessibility.view.shown.as_ref().filter(|shown| shown.help) else { return };
        let items = if unassigned { shown.unassigned.clone() } else { shown.assigned.clone() };
        if items.is_empty() {
            return;
        }
        let provider = shown.provider;
        self.save_accessible_view_position();
        self.accessibility.view.shown = None;
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.title = Some("Configure keybindings".into());
        input.placeholder = Some("Select a command ID to configure a keybinding for it".into());
        input.set_items(items.iter().enumerate().map(|(handle, id)| Entry::Item(Item { handle: handle as i64, label: id.clone(), ..Item::default() })).collect());
        let id = self.quick_inputs.add(input, Owner::AccessibleViewKeybindings(KeybindingPick { provider, commands: items, accepted: false }));
        self.show_quick_input(id, window, cx);
        self.flush_quick_input_events(window, cx);
    }

    /// Accepting opens the Keyboard Shortcuts editor on the command; hiding without a pick shows the help again.
    pub fn accessible_view_keybindings_event(&mut self, id: i64, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Owner::AccessibleViewKeybindings(pick)) = self.quick_inputs.owner_mut(id) else { return };
        match event {
            Event::Accept(_) => {
                let Some(handle) = self.quick_inputs.get(id).and_then(|input| input.selected_handles().first().copied()) else { return };
                let Some(Owner::AccessibleViewKeybindings(pick)) = self.quick_inputs.owner_mut(id) else { return };
                pick.accepted = true;
                let command = pick.commands[handle as usize].clone();
                self.hide_quick_input_by_id(id, window, cx);
                let outcome = self.execute_command("workbench.action.openGlobalKeybindings", vec![json!(command)], window, cx);
                super::quick_access::show_failure("Preferences: Open Keyboard Shortcuts".into(), outcome, window, cx);
            }
            Event::Hide => {
                let (provider, accepted) = (pick.provider, pick.accepted);
                self.quick_inputs.remove(id);
                if !accepted {
                    self.show_accessible_view(provider, true, window, cx);
                }
            }
            _ => {}
        }
    }

    fn reveal_accessible_view_cursor(&mut self, window: &Window) {
        let (visible_lines, visible_columns) = self.accessible_view_visible_size(window);
        let Some(shown) = &mut self.accessibility.view.shown else { return };
        let head = shown.selection.head;
        if head.line < shown.top_line {
            shown.top_line = head.line;
        } else if head.line >= shown.top_line + visible_lines {
            shown.top_line = head.line + 1 - visible_lines;
        }
        if head.column < shown.left_column {
            shown.left_column = head.column;
        } else if head.column >= shown.left_column + visible_columns {
            shown.left_column = head.column + 1 - visible_columns;
        }
    }

    /// `_layout`: three quarters of the window up to 900px wide, its content's height up to 60% of the window.
    fn accessible_view_size(&self, window: &Window) -> (f32, f32) {
        let viewport = window.viewport_size();
        let width = (f32::from(viewport.width) * WIDTH_RATIO).min(MAX_WIDTH);
        let lines = self.accessibility.view.shown.as_ref().map_or(1, |shown| shown.lines.len());
        let height = (lines as f32 * LINE_HEIGHT + 2. * PADDING).min(f32::from(viewport.height) * MAX_HEIGHT_RATIO);
        (width, height)
    }

    fn accessible_view_visible_size(&self, window: &Window) -> (usize, usize) {
        let (width, height) = self.accessible_view_size(window);
        let char_width = self.editors.viewport.char_width.max(1.);
        (((height - 2. * PADDING) / LINE_HEIGHT).floor().max(1.) as usize, ((width - LINE_DECORATIONS_WIDTH) / char_width).floor().max(1.) as usize)
    }

    /// The view's and its toolbar's own keys, before the keybindings as the editor's `onKeyDown` runs before the
    /// window's listener; false lets the key reach the keybindings.
    pub(super) fn accessible_view_key(&mut self, keystroke: &Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.accessibility.view.shown.is_none() {
            return false;
        }
        if self.accessibility.view.toolbar_focus.is_focused(window) {
            return self.accessible_view_toolbar_key(keystroke, window, cx);
        }
        self.accessibility.view.focus.is_focused(window) && self.accessible_view_text_key(keystroke, window, cx)
    }

    /// As its read-only code editor takes them: cursor moves with Shift selecting, Cmd+A, Cmd+C, Escape, Shift+Tab to
    /// the toolbar, Enter (`editor.action.openLink`, no links in this content), and with
    /// `accessibility.accessibleView.closeOnKeyPress` a typed key closes the view.
    fn accessible_view_text_key(&mut self, keystroke: &Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let modifiers = keystroke.modifiers;
        let (visible_lines, _) = self.accessible_view_visible_size(window);
        let tab_size = 4;
        let close_on_key = self.setting("accessibility.accessibleView.closeOnKeyPress") == json!(true);
        let Some(shown) = &mut self.accessibility.view.shown else { return false };
        let lines = &shown.lines;
        let head = shown.selection.head;
        let mut leftover = 0;
        let moved = match (keystroke.key.as_str(), modifiers.platform, modifiers.alt || modifiers.control) {
            ("escape", false, false) => {
                self.hide_accessible_view(window, cx);
                return true;
            }
            ("enter", false, false) => return true,
            ("tab", false, false) if modifiers.shift => {
                shown.toolbar_index = 0;
                window.focus(&self.accessibility.view.toolbar_focus, cx);
                cx.notify();
                return true;
            }
            ("a", true, false) if !modifiers.shift => {
                shown.selection = Selection { anchor: Position::default(), head: cursor::last_position(lines) };
                cx.notify();
                return true;
            }
            ("c", true, false) if !modifiers.shift => {
                let text = selected_text(lines, shown.selection);
                if !text.is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
                return true;
            }
            ("left", false, false) => cursor::left(lines, head),
            ("right", false, false) => cursor::right(lines, head),
            ("up", false, false) | ("down", false, false) => {
                let (position, rest) = cursor::vertical(lines, &[], head, shown.leftover, keystroke.key == "down", tab_size);
                leftover = rest;
                position
            }
            ("up", true, false) => Position::default(),
            ("down", true, false) => cursor::last_position(lines),
            ("left", true, false) | ("home", false, false) => cursor::line_start(lines, head),
            ("right", true, false) | ("end", false, false) => cursor::line_end(lines, head),
            ("pageup", false, false) | ("pagedown", false, false) => {
                let down = keystroke.key == "pagedown";
                let line = if down { (head.line + visible_lines).min(lines.len() - 1) } else { head.line.saturating_sub(visible_lines) };
                Position::new(line, cursor::column_from_visible(&lines[line], crate::editor::visible_column(&lines[head.line], head.column, tab_size), tab_size))
            }
            // `hide` stops the key; the focus is back where it was for the next one.
            (key, false, false) if !modifiers.shift && close_on_key && closes_on_key(key) => {
                self.hide_accessible_view(window, cx);
                return true;
            }
            _ => return false,
        };
        shown.selection = shown.selection.moved(moved, modifiers.shift);
        shown.leftover = leftover;
        self.reveal_accessible_view_cursor(window);
        cx.notify();
        true
    }

    fn accessible_view_toolbar_key(&mut self, keystroke: &Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let count = self.accessible_view_toolbar_actions(window).len();
        let Some(shown) = &mut self.accessibility.view.shown else { return false };
        match keystroke.key.as_str() {
            "escape" => self.hide_accessible_view(window, cx),
            "tab" => window.focus(&self.accessibility.view.focus, cx),
            "right" if count > 0 => shown.toolbar_index = (shown.toolbar_index + 1) % count,
            "left" if count > 0 => shown.toolbar_index = (shown.toolbar_index + count - 1) % count,
            "enter" | "space" => {
                let index = shown.toolbar_index;
                self.run_accessible_view_toolbar_action(index, window, cx);
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    /// `_updateToolbar`: the provider's actions, then `MenuId.AccessibleView` by order and title.
    fn accessible_view_toolbar_actions(&self, window: &Window) -> Vec<ToolbarAction> {
        let Some(shown) = &self.accessibility.view.shown else { return Vec::new() };
        let mut actions = Vec::new();
        if let (Provider::Notification(key), false) = (shown.provider, shown.help)
            && let Some(item) = self.notifications.model.item(key)
        {
            for action in item.primary.iter().chain(&item.secondary) {
                actions.push(ToolbarAction { title: action.label.clone(), icon: "primitive-square", run: ToolbarRun::Notification(key, action.clone()) });
            }
        }
        let context = self.key_context(window);
        let holds = |key: &str| context.get(key) == Some(&json!(true));
        let mut menu: Vec<(u8, &str, &'static str, &'static str)> = Vec::new();
        if holds("accessibleViewIsShown") && holds("accessibleViewSupportsNavigation") {
            menu.push((0, "Show Next in Accessible View", "arrow-down", "editor.action.accessibleViewNext"));
            menu.push((0, "Show Previous in Accessible View", "arrow-up", "editor.action.accessibleViewPrevious"));
        }
        if holds("accessibleViewVerbosityEnabled") {
            menu.push((0, "Disable Accessible View Hint", "bell-slash", "editor.action.accessibleViewDisableHint"));
        }
        if holds("accessibleViewHasUnassignedKeybindings") {
            menu.push((3, "Accessibility Help Configure Unassigned Keybindings", "record-keys", "editor.action.accessibilityHelpConfigureKeybindings"));
        }
        if holds("accessibleViewHasAssignedKeybindings") {
            menu.push((4, "Accessibility Help Configure Assigned Keybindings", "record-keys", "editor.action.accessibilityHelpConfigureAssignedKeybindings"));
        }
        menu.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| crate::command_palette::locale_compare(a.1, b.1)));
        actions.extend(menu.into_iter().map(|(_, title, icon, command)| ToolbarAction { title: title.into(), icon, run: ToolbarRun::Command(command) }));
        actions
    }

    fn run_accessible_view_toolbar_action(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(action) = self.accessible_view_toolbar_actions(window).into_iter().nth(index) else { return };
        match action.run {
            ToolbarRun::Command(command) => {
                let outcome = self.execute_command(command, Vec::new(), window, cx);
                super::quick_access::show_failure(action.title, outcome, window, cx);
            }
            ToolbarRun::Notification(key, action) => {
                self.hide_accessible_view(window, cx);
                let outcome = self.run_action(key, action, window, cx);
                super::quick_access::show_failure(String::new(), outcome, window, cx);
            }
        }
    }

    /// The view's `aria-label` (`_render`): its kind, with the actions hint while verbosity is on.
    fn accessible_view_aria_label(&self, shown: &Shown) -> String {
        let verbose = self.setting(shown.provider.verbosity_key()) == json!(true);
        let has_actions = !shown.help && matches!(shown.provider, Provider::Notification(_)) || verbose;
        let actions_hint = (verbose && has_actions).then_some("Explore actions such as disabling this hint (Shift+Tab).");
        match (shown.help, actions_hint) {
            (true, Some(hint)) => format!("Accessibility Help, {hint}"),
            (true, None) => "Accessibility Help".into(),
            (false, Some(hint)) => format!("Accessible View, {hint}"),
            (false, None) => "Accessible View".into(),
        }
    }

    /// The context view of accessibility.css `.accessible-view`: the title bar with its toolbar over the read-only
    /// editor, at the quick pick's top. With `accessibility.hideAccessibleView` it sits off screen, still read.
    pub(super) fn accessible_view_widget(&self, window: &Window, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let shown = self.accessibility.view.shown.as_ref()?;
        let (width, height) = self.accessible_view_size(window);
        let (visible_lines, _) = self.accessible_view_visible_size(window);
        let viewport = window.viewport_size();
        let hidden = self.setting("accessibility.hideAccessibleView") == json!(true);
        let (left, top) = if hidden { (-2000., -2000.) } else { (((f32::from(viewport.width) - width) / 2.).round(), TOP) };
        let toolbar_focused = self.accessibility.view.toolbar_focus.is_focused(window);
        let icon_color = self.color("icon.foreground");
        let focus_border = self.color("focusBorder");
        let mut toolbar = div().id("accessible-view-toolbar").track_focus(&self.accessibility.view.toolbar_focus).flex().justify_end().flex_1().mr(px(4.)).role(Role::Toolbar).aria_label(if shown.help { "Accessibility Help" } else { "Accessible View" });
        for (index, action) in self.accessible_view_toolbar_actions(window).into_iter().enumerate() {
            let focused = toolbar_focused && index == shown.toolbar_index;
            toolbar = toolbar.child(
                self.icon(action.icon, 22., 16., icon_color)
                    .id(("accessible-view-action", index))
                    .cursor_pointer()
                    .rounded(px(5.))
                    .border_1()
                    .border_color(if focused { focus_border } else { gpui::transparent_black().into() })
                    .role(Role::Button)
                    .aria_label(action.title.clone())
                    .child(self.hover_target(("accessible-view-action-hover", index), super::hover::HoverContent::Text(action.title), super::hover::HoverOptions::instant()))
                    .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| workbench.run_accessible_view_toolbar_action(index, window, cx))),
            );
        }
        let title_bar = div().h(px(TITLE_HEIGHT)).flex().items_center().rounded_t(px(5.)).child(div().w_full().py(px(3.)).flex().justify_center().overflow_hidden().child(if shown.help { "Accessibility Help" } else { "Accessible View" })).child(toolbar);
        let char_width = self.editors.viewport.char_width;
        let selection_color = self.color("editor.selectionBackground");
        let cursor_color = self.color("editorCursor.foreground");
        let view_focused = self.accessibility.view.focus.is_focused(window);
        let mut text = div().relative().size_full().overflow_hidden().pt(px(PADDING)).pl(px(LINE_DECORATIONS_WIDTH)).font_family("Menlo").text_size(px(12.)).line_height(px(LINE_HEIGHT)).text_color(self.color("editor.foreground"));
        let (start, end) = (shown.selection.start(), shown.selection.end());
        for line in shown.top_line..(shown.top_line + visible_lines).min(shown.lines.len()) {
            let chars: Vec<char> = shown.lines[line].chars().collect();
            let selected = (start.line <= line && line <= end.line).then(|| (if line == start.line { start.column } else { 0 }, if line == end.line { end.column } else { chars.len() + 1 }));
            let mut row = div().h(px(LINE_HEIGHT)).flex().whitespace_nowrap().ml(px(-(shown.left_column as f32) * char_width));
            let mut column = 0;
            let mut boundaries = vec![chars.len()];
            if let Some((from, to)) = selected {
                boundaries.extend([from.min(chars.len()), to.min(chars.len())]);
            }
            boundaries.sort_unstable();
            for boundary in boundaries {
                if boundary > column {
                    let in_selection = selected.is_some_and(|(from, to)| from <= column && column < to);
                    row = row.child(div().when(in_selection, |run| run.bg(selection_color)).child(chars[column..boundary].iter().collect::<String>()));
                    column = boundary;
                }
            }
            if selected.is_some_and(|(_, to)| to > chars.len()) && line != end.line {
                row = row.child(div().w(px(char_width)).h_full().bg(selection_color));
            }
            if view_focused && line == shown.selection.head.line {
                let x = LINE_DECORATIONS_WIDTH + (shown.selection.head.column as f32 - shown.left_column as f32) * char_width;
                text = text.child(div().absolute().left(px(x)).top(px(PADDING + (line - shown.top_line) as f32 * LINE_HEIGHT)).w(px(1.)).h(px(LINE_HEIGHT)).bg(cursor_color));
            }
            text = text.child(row);
        }
        let page = window.is_a11y_active().then(|| super::accessibility::screen_reader_page(&shown.lines, shown.selection.anchor, shown.selection.head, self.setting("editor.accessibilityPageSize").as_u64().unwrap_or(500).max(1) as usize));
        let body = div()
            .id("accessible-view-editor")
            .track_focus(&self.accessibility.view.focus)
            .h(px(height))
            .cursor_text()
            .bg(self.color("editor.background"))
            .role(Role::MultilineTextInput)
            .aria_label(self.accessible_view_aria_label(shown))
            .when_some(page, |body, page| body.a11y_synthetic_children(move |builder| page.build(builder)))
            .on_scroll_wheel(cx.listener(move |workbench, event: &ScrollWheelEvent, window, cx| {
                let delta = event.delta.pixel_delta(px(LINE_HEIGHT));
                let Some(shown) = &mut workbench.accessibility.view.shown else { return };
                let lines = (-f32::from(delta.y) / LINE_HEIGHT).round() as isize;
                shown.top_line = (shown.top_line as isize + lines).clamp(0, shown.lines.len().saturating_sub(visible_lines) as isize) as usize;
                let _ = window;
                cx.notify();
            }))
            .on_click(cx.listener(|workbench, _: &ClickEvent, window, cx| window.focus(&workbench.accessibility.view.focus, cx)))
            .child(text);
        Some(
            div()
                .id("accessible-view")
                .absolute()
                .occlude()
                .left(px(left))
                .top(px(top))
                .w(px(width + 4.))
                .flex()
                .flex_col()
                .bg(self.color("editorWidget.background"))
                .text_color(self.color("editorWidget.foreground"))
                .border_2()
                .border_color(focus_border)
                .rounded(px(6.))
                .shadow(vec![gpui::BoxShadow { color: gpui::rgba(0x00000026).into(), offset: gpui::point(px(0.), px(0.)), blur_radius: px(10.), spread_radius: px(0.), inset: false }])
                .child(title_bar)
                .child(body)
                .into_any_element(),
        )
    }
}

/// `withSeverityPrefix` of notification.ts.
fn with_severity_prefix(label: &str, severity: Severity) -> String {
    match severity {
        Severity::Error => format!("Error: {label}"),
        Severity::Warning => format!("Warning: {label}"),
        _ => format!("Info: {label}"),
    }
}

/// accessibleViewKeybindingResolver.ts `resolveContentAndKeybindingItems`: each `<keybinding:id>` becomes ` (label)`,
/// or ` (unassigned keybinding)`; returns the text and the unassigned and assigned command ids.
fn resolve_keybindings(text: &str, aria_label: impl Fn(&str) -> Option<String>) -> (String, Vec<String>, Vec<String>) {
    let (mut unassigned, mut assigned) = (Vec::new(), Vec::new());
    let mut resolved = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("<keybinding:") {
        let Some(length) = rest[start..].find('>') else { break };
        let command = &rest[start + "<keybinding:".len()..start + length];
        resolved += &rest[..start];
        match aria_label(command) {
            Some(label) => {
                resolved += &format!(" ({label})");
                assigned.push(command.to_owned());
            }
            None => {
                resolved += " (unassigned keybinding)";
                unassigned.push(command.to_owned());
            }
        }
        rest = &rest[start + length + 1..];
    }
    resolved += rest;
    (resolved, unassigned, assigned)
}

/// `shouldHandleKey`: letters, digits, the punctuation keys, Space and Enter.
fn closes_on_key(key: &str) -> bool {
    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        (Some(char), None) => char.is_ascii_alphanumeric() || "=,./'`\\-;".contains(char),
        _ => matches!(key, "space" | "enter"),
    }
}

fn selected_text(lines: &[String], selection: Selection) -> String {
    let (start, end) = (selection.start(), selection.end());
    let mut text = String::new();
    for line in start.line..=end.line {
        let chars: Vec<char> = lines[line].chars().collect();
        let from = if line == start.line { start.column } else { 0 };
        let to = if line == end.line { end.column } else { chars.len() };
        text.extend(&chars[from..to]);
        if line != end.line {
            text.push('\n');
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keybindings_resolve_to_labels_or_unassigned() {
        let label = |command: &str| (command == "a.b").then(|| "Option+F1".to_owned());
        let (text, unassigned, assigned) = resolve_keybindings("Run<keybinding:a.b> or<keybinding:c.d>.", label);
        assert_eq!(text, "Run (Option+F1) or (unassigned keybinding).");
        assert_eq!((unassigned, assigned), (vec!["c.d".to_owned()], vec!["a.b".to_owned()]));
    }

    #[test]
    fn typed_keys_close_and_navigation_keys_do_not() {
        assert!(closes_on_key("a") && closes_on_key("7") && closes_on_key("space") && closes_on_key("enter") && closes_on_key("/"));
        assert!(!closes_on_key("down") && !closes_on_key("escape") && !closes_on_key("f2"));
    }

    #[gpui::test]
    fn alt_f1_shows_the_editor_help_and_escape_returns_to_the_editor(cx: &mut gpui::TestAppContext) {
        use std::path::Path;
        let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
        let themes = exthost::scan::scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let theme = crate::theme::load_theme(&[themes], "Dark Modern").unwrap();
        let dir = std::env::temp_dir().join(format!("code-gpui-accessible-view-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, "ab").unwrap();
        let (workbench, cx) = cx.add_window_view(|window, cx| {
            let mut workbench = Workbench::new(theme, app_root.clone(), None, crate::languages::Languages::new(&[]), crate::default_settings(), crate::host_sync::HostSync::new(std::sync::mpsc::channel().0), cx);
            workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), dir.join("keybindings.json"), cx);
            workbench.open_file(file.clone(), true, window, cx);
            workbench.watch_editor_focus(window, cx);
            workbench
        });
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        let state = |cx: &mut gpui::VisualTestContext| {
            workbench.update_in(cx, |workbench, window, _| {
                let shown = workbench.accessibility.view.shown.as_ref().map(|shown| (shown.help, shown.lines.clone(), shown.selection.head));
                (shown, workbench.accessibility.view.focus.is_focused(window), workbench.editor_focus.is_focused(window))
            })
        };

        cx.simulate_keystrokes("alt-f1");
        let (shown, view_focused, _) = state(cx);
        let (help, lines, _) = shown.unwrap();
        assert!(help && view_focused);
        assert_eq!(lines[0], "The application is configured to never be optimized for usage with a Screen Reader.");
        assert_eq!(lines[2], "You are in a code editor.");
        assert!(lines.iter().any(|line| line == "Go to Symbol (Shift+Command+O) to quickly navigate between symbols in the current file."), "{lines:#?}");
        assert!(lines.iter().any(|line| line.contains("(unassigned keybinding)")));
        assert_eq!(lines.last().unwrap(), "Configure keybindings for commands that already have assignments (Option+A).");
        cx.simulate_keystrokes("down down end");
        assert_eq!(state(cx).0.unwrap().2, Position::new(2, "You are in a code editor.".chars().count()));
        cx.simulate_keystrokes("escape");
        assert_eq!((state(cx).0.is_none(), state(cx).2), (true, true));
        // Typing a key closes the view (`closeOnKeyPress`), and the position comes back on reopening.
        cx.simulate_keystrokes("alt-f1");
        assert_eq!(state(cx).0.unwrap().2.line, 2);
        cx.simulate_keystrokes("x");
        assert_eq!((state(cx).0.is_none(), state(cx).2), (true, true));

        // A focused notification: its row's label names Alt+F2, which shows its message; Escape goes back to the list.
        let label = workbench.update_in(cx, |workbench, window, cx| {
            let key = workbench.notify(crate::notifications::Notification::new(Severity::Warning, "Disk is full")).unwrap();
            cx.notify();
            let item = workbench.notifications.model.item(key).unwrap().clone();
            workbench.notification_aria_label(&item, window)
        });
        assert_eq!(label, "Warning: Disk is full, notification, Inspect the response in the accessible view with Option+F2");
        cx.run_until_parked();
        let outcome = workbench.update_in(cx, |workbench, window, cx| workbench.execute_command("notifications.focusToasts", Vec::new(), window, cx));
        drop(outcome);
        cx.simulate_keystrokes("alt-f2");
        let (shown, _, _) = state(cx);
        let (help, lines, _) = shown.unwrap();
        assert_eq!((help, lines), (false, vec!["Warning: Disk is full".to_owned()]));
        cx.simulate_keystrokes("escape");
        let notifications_focused = workbench.update_in(cx, |workbench, window, _| workbench.notifications.focus.is_focused(window));
        assert_eq!((state(cx).0.is_none(), notifications_focused), (true, true));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn selected_text_spans_lines() {
        let lines = vec!["one".to_owned(), "two".to_owned()];
        assert_eq!(selected_text(&lines, Selection { anchor: Position::new(0, 1), head: Position::new(1, 2) }), "ne\ntw");
    }

    /// The terminal's accessible view: its buffer with the cursor on the last line, command navigation, the
    /// terminal help as its help, and Escape back to the terminal.
    #[gpui::test]
    fn the_terminal_buffer_shows_in_the_accessible_view(cx: &mut gpui::TestAppContext) {
        use std::path::Path;
        let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
        let themes = exthost::scan::scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let theme = crate::theme::load_theme(&[themes], "Dark Modern").unwrap();
        let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), None, crate::languages::Languages::new(&[]), crate::default_settings(), crate::host_sync::HostSync::new(std::sync::mpsc::channel().0), cx));
        cx.update(|window, _| window.activate_window());
        let command = |name: &str| format!("\x1b]633;A\x07$ \x1b]633;B\x07{name}\r\n\x1b]633;C\x07out\r\n\x1b]633;D;0\x07");
        workbench.update_in(cx, |workbench, window, cx| {
            let launch = crate::terminal::ShellLaunchConfig { extension_pty: true, cwd: std::env::temp_dir(), ..Default::default() };
            let id = workbench.add_terminal(launch, None, window, cx);
            let colors = crate::terminal::colors::TerminalColors::new(&workbench.theme);
            let instance = workbench.terminal_view.instance_mut(id).unwrap();
            instance.process(format!("{}{}\x1b]633;A\x07$ ", command("a"), command("b")).as_bytes(), &colors);
            window.focus(&workbench.terminal_view.focus, cx);
            drop(workbench.execute_command("editor.action.accessibleView", Vec::new(), window, cx));
        });
        let shown = |cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.accessibility.view.shown.as_ref().map(|shown| (shown.provider, shown.help, shown.lines.clone(), shown.selection.head.line)));
        let (provider, help, lines, line) = shown(cx).unwrap();
        assert_eq!((provider.id(), help, lines.as_slice(), line), ("terminal", false, ["$ a", "out", "$ b", "out", "$"].map(str::to_owned).as_slice(), 4));
        let run = |cx: &mut gpui::VisualTestContext, command: &str| workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command(command, Vec::new(), window, cx)));
        run(cx, "workbench.action.terminal.accessibleBufferGoToPreviousCommand");
        assert_eq!(shown(cx).unwrap().3, 2);
        run(cx, "workbench.action.terminal.accessibleBufferGoToPreviousCommand");
        assert_eq!(shown(cx).unwrap().3, 0);
        run(cx, "workbench.action.terminal.accessibleBufferGoToNextCommand");
        assert_eq!(shown(cx).unwrap().3, 2);
        run(cx, "editor.action.accessibilityHelp");
        let (_, help, lines, _) = shown(cx).unwrap();
        assert!(help && lines.iter().any(|line| line.starts_with("The Focus Accessible Terminal View command")), "{lines:#?}");
        assert!(lines.iter().any(|line| line.starts_with("The terminal has a feature called shell integration")));
        cx.simulate_keystrokes("escape");
        assert!(!shown(cx).unwrap().1, "Escape leaves the help for the buffer");
        cx.simulate_keystrokes("escape");
        let terminal_focused = workbench.update_in(cx, |workbench, window, _| workbench.terminal_view.focus.is_focused(window));
        assert!(shown(cx).is_none() && terminal_focused);
    }
}
