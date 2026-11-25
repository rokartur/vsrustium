//! `nativeHostService.showMessageBox` with a checkbox: Electron's macOS message box is an NSAlert sheet whose
//! suppression button carries `checkboxLabel`. GPUI's `Window::prompt` has no suppression button.

use std::cell::Cell;
use std::rc::Rc;

use block2::RcBlock;
use futures::channel::oneshot;
use gpui::{App, PromptLevel, Window};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSAlert, NSAlertStyle, NSControlStateValueOn, NSModalResponse, NSView, NSWindow};
use objc2_foundation::NSString;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use crate::workbench::Workbench;

/// The pressed button's index (buttons in `massageMessageBoxOptions` order: primary first, cancel last) and
/// whether the checkbox, if any, was checked.
/// With `window.dialogStyle: custom` the workbench's custom dialog shows it instead (dialog.contribution.ts).
pub fn show_message_box(window: &Window, level: PromptLevel, message: &str, detail: Option<&str>, buttons: &[&str], checkbox: Option<&str>, cx: &App) -> oneshot::Receiver<(usize, bool)> {
    let (sender, receiver) = oneshot::channel();
    let sender = Rc::new(Cell::new(Some(sender)));
    let mtm = MainThreadMarker::new().expect("dialogs open on the main thread");
    let alert = NSAlert::new(mtm);
    // Electron's message_box_mac.mm: warning and error types take the critical style, the caution icon with an app
    // icon badge (the warning style draws the app icon alone); the others informational.
    alert.setAlertStyle(match level {
        PromptLevel::Info => NSAlertStyle::Informational,
        PromptLevel::Warning | PromptLevel::Critical => NSAlertStyle::Critical,
    });
    alert.setMessageText(&NSString::from_str(message));
    if let Some(detail) = detail {
        alert.setInformativeText(&NSString::from_str(detail));
    }
    for (index, title) in buttons.iter().enumerate() {
        let button = alert.addButtonWithTitle(&NSString::from_str(title));
        button.setTag(index as isize);
        if index == buttons.len() - 1 && index > 0 {
            button.setKeyEquivalent(&NSString::from_str("\u{1b}"));
        }
    }
    let suppression = checkbox.map(|checkbox| {
        alert.setShowsSuppressionButton(true);
        let suppression = alert.suppressionButton().expect("an alert showing its suppression button has one");
        suppression.setTitle(&NSString::from_str(checkbox));
        suppression
    });
    let count = buttons.len();
    let custom = (level, message.to_owned(), detail.map(str::to_owned), buttons.iter().map(|button| button.to_string()).collect::<Vec<_>>(), checkbox.map(str::to_owned));
    let custom_sender = sender.clone();
    let handler = RcBlock::new(move |response: NSModalResponse| {
        let checked = suppression.as_ref().is_some_and(|suppression| suppression.state() == NSControlStateValueOn);
        // A negative response (NSModalResponseAbort) is a sheet closed without a button: cancel.
        let index = usize::try_from(response).ok().filter(|index| *index < count).unwrap_or(count - 1);
        if let Some(sender) = sender.take() {
            let _ = sender.send((index, checked));
        }
    });
    let ns_window = ns_window(window);
    let workbench = window.root::<Workbench>().flatten();
    // Begun after this update returns: the sheet takes the key window and GPUI's handlers must not run re-entrantly,
    // and the workbench the caller updates can be read for its dialog style only then.
    cx.spawn(async move |cx| {
        let workbench = workbench.filter(|workbench| workbench.read_with(cx, |workbench, _| workbench.uses_custom_dialogs()));
        let Some(workbench) = workbench else {
            return alert.beginSheetModalForWindow_completionHandler(&ns_window, Some(&handler));
        };
        let (level, message, detail, buttons, checkbox) = custom;
        let answer = workbench.update(cx, |workbench, cx| workbench.show_custom_message_box(level, message, detail, buttons, checkbox, cx));
        if let (Ok(answer), Some(sender)) = (answer.await, custom_sender.take()) {
            let _ = sender.send(answer);
        }
    })
    .detach();
    receiver
}

pub(crate) fn ns_window(window: &Window) -> Retained<NSWindow> {
    app_kit_window(window).expect("macOS windows are AppKit windows")
}

/// The window's NSWindow; none on GPUI's test platform.
pub(crate) fn app_kit_window(window: &Window) -> Option<Retained<NSWindow>> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else { return None };
    // SAFETY: GPUI's AppKit handle points at the window's content NSView, alive while the window is.
    let view: &NSView = unsafe { handle.ns_view.cast().as_ref() };
    view.window()
}
