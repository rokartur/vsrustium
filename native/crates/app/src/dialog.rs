//! `nativeHostService.showMessageBox` with a checkbox: Electron's macOS message box is an NSAlert sheet whose
//! suppression button carries `checkboxLabel`. GPUI's `Window::prompt` has no suppression button.

use std::cell::Cell;

use block2::RcBlock;
use futures::channel::oneshot;
use gpui::{App, PromptLevel, Window};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSAlert, NSAlertStyle, NSControlStateValueOn, NSModalResponse, NSView, NSWindow};
use objc2_foundation::NSString;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// The pressed button's index (buttons in `massageMessageBoxOptions` order: primary first, cancel last) and
/// whether the checkbox was checked.
pub fn show_message_box(window: &Window, level: PromptLevel, message: &str, detail: Option<&str>, buttons: &[&str], checkbox: &str, cx: &App) -> oneshot::Receiver<(usize, bool)> {
    let (sender, receiver) = oneshot::channel();
    let mtm = MainThreadMarker::new().expect("dialogs open on the main thread");
    let alert = NSAlert::new(mtm);
    // Electron's message_box_mac.mm: warning and error types draw the warning style, the others informational.
    alert.setAlertStyle(match level {
        PromptLevel::Info => NSAlertStyle::Informational,
        PromptLevel::Warning | PromptLevel::Critical => NSAlertStyle::Warning,
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
    alert.setShowsSuppressionButton(true);
    let suppression = alert.suppressionButton().expect("an alert showing its suppression button has one");
    suppression.setTitle(&NSString::from_str(checkbox));
    let count = buttons.len();
    let sender = Cell::new(Some(sender));
    let handler = RcBlock::new(move |response: NSModalResponse| {
        let checked = suppression.state() == NSControlStateValueOn;
        // A negative response (NSModalResponseAbort) is a sheet closed without a button: cancel.
        let index = usize::try_from(response).ok().filter(|index| *index < count).unwrap_or(count - 1);
        if let Some(sender) = sender.take() {
            let _ = sender.send((index, checked));
        }
    });
    let ns_window = ns_window(window);
    // Begun after this update returns: the sheet takes the key window and GPUI's handlers must not run re-entrantly.
    cx.foreground_executor()
        .spawn(async move {
            alert.beginSheetModalForWindow_completionHandler(&ns_window, Some(&handler));
        })
        .detach();
    receiver
}

fn ns_window(window: &Window) -> Retained<NSWindow> {
    let handle = HasWindowHandle::window_handle(window).expect("an open window has a handle");
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else { unreachable!("macOS windows are AppKit windows") };
    // SAFETY: GPUI's AppKit handle points at the window's content NSView, alive while the window is.
    let view: &NSView = unsafe { handle.ns_view.cast().as_ref() };
    view.window().expect("the content view is in its window")
}
