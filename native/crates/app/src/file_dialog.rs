//! dialogMainService.ts `showOpenDialog` and `showSaveDialog` through Electron's file_dialog_mac.mm: an NSOpenPanel or
//! NSSavePanel as a sheet on the window, with the filters, the default path, the button label and the title.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

use block2::RcBlock;
use futures::channel::oneshot;
use gpui::{App, PathPromptOptions};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{NSApplication, NSModalResponse, NSModalResponseOK, NSOpenPanel, NSPopUpButton, NSSavePanel, NSTextField, NSView};
use objc2_foundation::{NSArray, NSObject, NSPoint, NSRect, NSSize, NSString, NSURL};
use icu_normalizer::ComposingNormalizerBorrowed;

/// Options of both panels; the `files`, `folders` and `multiple` flags only count for the open panel.
#[derive(Clone, Debug, Default, Hash, PartialEq)]
pub struct FileDialog {
    pub title: Option<String>,
    pub button_label: Option<String>,
    /// `{ name, extensions }` pairs; the first one applies, more than one adds the Format picker.
    pub filters: Vec<(String, Vec<String>)>,
    pub default_path: Option<PathBuf>,
    pub files: bool,
    pub folders: bool,
    pub multiple: bool,
    /// `treatPackageAsDirectory`, which `doPick` adds on macOS.
    pub packages_as_directories: bool,
}

thread_local! {
    /// `windowFileDialogLocks`: the window and options hash of every panel showing.
    static LOCKS: RefCell<HashSet<(usize, u64)>> = RefCell::default();
}

/// The picked paths, empty when cancelled or when the same dialog already shows for this window.
pub fn show_open_dialog(mut dialog: FileDialog, cx: &App) -> oneshot::Receiver<Vec<PathBuf>> {
    if cfg!(test) {
        let picked = cx.prompt_for_paths(PathPromptOptions { files: dialog.files, directories: dialog.folders, multiple: dialog.multiple, prompt: None });
        return simulated(picked, cx, |picked| picked.unwrap_or_default());
    }
    // "Ensure the path exists (if provided)".
    if dialog.default_path.as_ref().is_some_and(|path| !path.exists()) {
        dialog.default_path = None;
    }
    let mtm = MainThreadMarker::new().expect("dialogs open on the main thread");
    let panel = NSOpenPanel::openPanel(mtm);
    panel.setCanChooseFiles(dialog.files);
    panel.setCanChooseDirectories(dialog.folders);
    panel.setAllowsMultipleSelection(dialog.multiple);
    panel.setCanCreateDirectories(true);
    panel.setTreatsFilePackagesAsDirectories(dialog.packages_as_directories);
    let format = setup(&panel, &dialog, mtm);
    if format.is_some() {
        panel.setAccessoryViewDisclosed(true);
    }
    let reader = panel.clone();
    show(&dialog, &panel, format, cx, Vec::new(), move |ok| match ok {
        true => reader.URLs().iter().filter_map(|url| url_path(&url)).collect(),
        false => Vec::new(),
    })
}

/// The picked path, `None` when cancelled or when the same dialog already shows for this window.
pub fn show_save_dialog(dialog: FileDialog, cx: &App) -> oneshot::Receiver<Option<PathBuf>> {
    if cfg!(test) {
        let (directory, name) = default_location(dialog.default_path.as_deref());
        let picked = cx.prompt_for_new_path(&directory.unwrap_or_default(), name.as_deref());
        return simulated(picked, cx, |picked| picked);
    }
    let mtm = MainThreadMarker::new().expect("dialogs open on the main thread");
    let panel = NSSavePanel::savePanel(mtm);
    let format = setup(&panel, &dialog, mtm);
    let reader = panel.clone();
    show(&dialog, &panel, format, cx, None, move |ok| ok.then(|| reader.URL().and_then(|url| url_path(&url))).flatten())
}

/// Tests answer through GPUI's simulated path prompts (`simulate_new_path_selection`); AppKit panels need the main thread.
fn simulated<P: 'static, T: 'static>(picked: oneshot::Receiver<anyhow::Result<Option<P>>>, cx: &App, map: impl FnOnce(Option<P>) -> T + 'static) -> oneshot::Receiver<T> {
    let (sender, receiver) = oneshot::channel();
    cx.foreground_executor()
        .spawn(async move {
            let picked = picked.await.ok().and_then(Result::ok).flatten();
            let _ = sender.send(map(picked));
        })
        .detach();
    receiver
}

/// `acquireFileDialogLock`, then the sheet on the focused window (`targetWindowId` is the active window), else a
/// panel of its own; `read` turns the response into the answer.
fn show<T: 'static>(dialog: &FileDialog, panel: &NSSavePanel, format: Option<Retained<FormatHandler>>, cx: &App, refused: T, read: impl Fn(bool) -> T + 'static) -> oneshot::Receiver<T> {
    let (sender, receiver) = oneshot::channel();
    let mtm = MainThreadMarker::new().expect("dialogs open on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    let ns_window = app.keyWindow().or_else(|| app.mainWindow());
    let mut hasher = DefaultHasher::new();
    dialog.hash(&mut hasher);
    let lock = (ns_window.as_ref().map_or(0, |window| Retained::as_ptr(window) as usize), hasher.finish());
    // Without a window as many dialogs as needed may show, they are not modal to one.
    if ns_window.is_some() && !LOCKS.with_borrow_mut(|locks| locks.insert(lock)) {
        eprintln!("[DialogMainService]: file open dialog is already or will be showing for the window with the same configuration");
        let _ = sender.send(refused);
        return receiver;
    }
    let sender = Cell::new(Some(sender));
    let handler = RcBlock::new(move |response: NSModalResponse| {
        LOCKS.with_borrow_mut(|locks| locks.remove(&lock));
        // The Format pop-up's target lives as long as its sheet.
        let _target = &format;
        if let Some(sender) = sender.take() {
            let _ = sender.send(read(response == NSModalResponseOK));
        }
    });
    let panel = panel.retain();
    // Begun after this update returns, as the message box sheets are.
    cx.foreground_executor()
        .spawn(async move {
            match ns_window {
                Some(ns_window) => panel.beginSheetModalForWindow_completionHandler(&ns_window, &handler),
                None => panel.beginWithCompletionHandler(&handler),
            }
        })
        .detach();
    receiver
}

/// file_dialog_mac.mm `SetupDialog` and `SetAllowedFileTypes`; the Format pop-up's target, if any.
fn setup(panel: &NSSavePanel, dialog: &FileDialog, mtm: MainThreadMarker) -> Option<Retained<FormatHandler>> {
    if let Some(title) = &dialog.title {
        panel.setTitle(Some(&NSString::from_str(title)));
    }
    if let Some(label) = &dialog.button_label {
        panel.setPrompt(Some(&NSString::from_str(&strip_mnemonic(label))));
    }
    let (directory, name) = default_location(dialog.default_path.as_deref());
    if let Some(directory) = directory {
        panel.setDirectoryURL(Some(&NSURL::fileURLWithPath(&NSString::from_str(&directory.to_string_lossy()))));
    }
    if let Some(name) = name {
        panel.setNameFieldStringValue(&NSString::from_str(&name));
    }
    let types = filter_types(&dialog.filters);
    if let Some(first) = types.first() {
        set_allowed_types(panel, first);
    }
    (types.len() > 1).then(|| add_format_picker(panel, &dialog.filters, types, mtm))
}

/// A directory opens the panel in it; another path opens in its parent with its name filled in.
fn default_location(path: Option<&Path>) -> (Option<PathBuf>, Option<String>) {
    let Some(path) = path else { return (None, None) };
    if path.is_dir() {
        return (Some(path.to_path_buf()), None);
    }
    let directory = path.is_absolute().then(|| path.parent().map(Path::to_path_buf)).flatten();
    (directory, path.file_name().map(|name| name.to_string_lossy().into_owned()))
}

/// Per filter its extensions, only the last part of `tar.gz`-like ones; `None` for a filter with `*` or with no
/// extension left, which allows every file as an empty UTType list does.
fn filter_types(filters: &[(String, Vec<String>)]) -> Vec<Option<Vec<String>>> {
    let mut types = Vec::new();
    for (_, extensions) in filters {
        let mut filter = Vec::new();
        let mut any = false;
        for extension in extensions.iter().filter(|extension| !extension.is_empty()) {
            let last = extension.rsplit('.').next().unwrap_or(extension);
            if last == "*" {
                any = true;
                break;
            }
            if !filter.iter().any(|seen| seen == last) {
                filter.push(last.to_owned());
            }
        }
        types.push((!any && !filter.is_empty()).then_some(filter));
    }
    types
}

// The UTType API Electron uses needs the UniformTypeIdentifiers framework; the extension list picks the same files.
#[allow(deprecated)]
fn set_allowed_types(panel: &NSSavePanel, types: &Option<Vec<String>>) {
    match types {
        Some(types) => {
            let types: Vec<Retained<NSString>> = types.iter().map(|extension| NSString::from_str(extension)).collect();
            panel.setAllowedFileTypes(Some(&NSArray::from_retained_slice(&types)));
        }
        None => {
            panel.setAllowedFileTypes(None);
            panel.setAllowsOtherFileTypes(true);
        }
    }
}

/// `ElectronAccessoryView`: "Format:" and a pop-up of the filter names that switches the allowed types.
fn add_format_picker(panel: &NSSavePanel, filters: &[(String, Vec<String>)], types: Vec<Option<Vec<String>>>, mtm: MainThreadMarker) -> Retained<FormatHandler> {
    let rect = |x: f64, y: f64, width: f64, height: f64| NSRect::new(NSPoint::new(x, y), NSSize::new(width, height));
    let view = NSView::initWithFrame(NSView::alloc(mtm), rect(0., 0., 200., 32.));
    let label = NSTextField::initWithFrame(NSTextField::alloc(mtm), rect(0., 0., 60., 22.));
    label.setEditable(false);
    label.setStringValue(&NSString::from_str("Format:"));
    label.setBordered(false);
    label.setBezeled(false);
    label.setDrawsBackground(false);
    let popup = NSPopUpButton::initWithFrame_pullsDown(NSPopUpButton::alloc(mtm), rect(50., 2., 140., 22.), false);
    let names: Vec<Retained<NSString>> = filters.iter().map(|(name, _)| NSString::from_str(name)).collect();
    popup.addItemsWithTitles(&NSArray::from_retained_slice(&names));
    let handler = FormatHandler::new(mtm, FormatState { panel: panel.retain(), types });
    let target: &AnyObject = &handler;
    // SAFETY: the handler answers `selectFormat:` and lives until the sheet ends (`show`); a target is not retained.
    unsafe {
        popup.setTarget(Some(target));
        popup.setAction(Some(sel!(selectFormat:)));
    }
    view.addSubview(&label);
    view.addSubview(&popup);
    panel.setAccessoryView(Some(&view));
    handler
}

struct FormatState {
    panel: Retained<NSSavePanel>,
    types: Vec<Option<Vec<String>>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "VSRustiumFileDialogFormatHandler"]
    #[ivars = FormatState]
    struct FormatHandler;

    unsafe impl NSObjectProtocol for FormatHandler {}

    impl FormatHandler {
        #[unsafe(method(selectFormat:))]
        fn select_format(&self, sender: &NSPopUpButton) {
            let state = self.ivars();
            if let Some(types) = usize::try_from(sender.indexOfSelectedItem()).ok().and_then(|index| state.types.get(index)) {
                set_allowed_types(&state.panel, types);
            }
        }
    }
);

impl FormatHandler {
    fn new(mtm: MainThreadMarker, state: FormatState) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(state);
        unsafe { msg_send![super(this), init] }
    }
}

/// `normalizePath`: macOS answers paths in NFD, upstream hands them on in NFC.
fn url_path(url: &NSURL) -> Option<PathBuf> {
    let path = url.path()?.to_string();
    Some(PathBuf::from(ComposingNormalizerBorrowed::new_nfc().normalize(&path).into_owned()))
}

/// `mnemonicButtonLabel` on macOS: `&&` marks a mnemonic, which mac buttons do not show.
pub fn strip_mnemonic(label: &str) -> String {
    label.replace("&&", "")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_keep_the_last_extension_part_and_star_allows_all() {
        let filters = vec![("Archives".to_owned(), vec!["tar.gz".to_owned(), "zip".to_owned(), "gz".to_owned()]), ("All Files".to_owned(), vec!["*".to_owned()])];
        assert_eq!(filter_types(&filters), vec![Some(vec!["gz".to_owned(), "zip".to_owned()]), None]);
    }

    #[test]
    fn a_default_file_opens_its_folder_with_its_name() {
        let folder = std::env::temp_dir();
        assert_eq!(default_location(Some(&folder)), (Some(folder.clone()), None));
        assert_eq!(default_location(Some(&folder.join("no-such-file.txt"))), (Some(folder), Some("no-such-file.txt".to_owned())));
        assert_eq!(default_location(Some(Path::new("name.txt"))), (None, Some("name.txt".to_owned())));
    }
}
