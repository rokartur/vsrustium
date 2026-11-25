//! platform/meteredConnection on macOS: whether the OS reports the network path as metered, the native side of
//! meteredConnection.ts `getIsBrowserConnectionMetered`, and the `network.meteredConnection` setting over it.

use std::ffi::c_void;
use std::sync::OnceLock;

use block2::RcBlock;
use futures::channel::mpsc::UnboundedSender;

#[link(name = "Network", kind = "framework")]
unsafe extern "C" {
    fn nw_path_monitor_create() -> *mut c_void;
    fn nw_path_monitor_set_queue(monitor: *mut c_void, queue: *const c_void);
    fn nw_path_monitor_set_update_handler(monitor: *mut c_void, handler: &block2::DynBlock<dyn Fn(*mut c_void)>);
    fn nw_path_monitor_start(monitor: *mut c_void);
    fn nw_path_is_expensive(path: *mut c_void) -> bool;
    fn nw_path_is_constrained(path: *mut c_void) -> bool;
}

unsafe extern "C" {
    static _dispatch_main_q: c_void;
}

static UPDATES: OnceLock<UnboundedSender<bool>> = OnceLock::new();

/// Sends whether the current path is metered, first when the monitor starts, then on every path change.
/// Chromium's `navigator.connection` on macOS reports neither `metered` nor `saveData`; the OS knows a path is
/// metered when it is expensive (cellular, a personal hotspot) or constrained (Low Data Mode).
pub fn watch(updates: UnboundedSender<bool>) {
    if UPDATES.set(updates).is_err() {
        return;
    }
    let handler = RcBlock::new(|path: *mut c_void| {
        let metered = unsafe { nw_path_is_expensive(path) || nw_path_is_constrained(path) };
        if let Some(updates) = UPDATES.get() {
            let _ = updates.unbounded_send(metered);
        }
    });
    unsafe {
        let monitor = nw_path_monitor_create();
        nw_path_monitor_set_queue(monitor, &_dispatch_main_q as *const _);
        nw_path_monitor_set_update_handler(monitor, &handler);
        // The monitor runs as long as the app; it is never cancelled or released.
        nw_path_monitor_start(monitor);
    }
}

/// `AbstractMeteredConnectionService`: `on` always, `off` never, `auto` (or any other value) the OS state.
pub fn is_metered(setting: Option<&str>, os_metered: bool) -> bool {
    setting == Some("on") || (setting != Some("off") && os_metered)
}

#[cfg(test)]
mod tests {
    use super::is_metered;

    #[test]
    fn the_setting_overrides_the_os() {
        assert!(is_metered(Some("on"), false));
        assert!(!is_metered(Some("off"), true));
        assert!(is_metered(Some("auto"), true));
        assert!(!is_metered(Some("auto"), false));
        assert!(is_metered(None, true));
    }
}
