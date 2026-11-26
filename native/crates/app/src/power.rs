//! IPowerService on macOS (workbench/services/power, the powerMonitor and powerSaveBlocker calls of
//! nativeHostMainService.ts): the queries MainThreadPower forwards and the events it sends the extension host.

use std::collections::HashMap;
use std::ptr::NonNull;
use std::sync::{LazyLock, Mutex, OnceLock};

use block2::RcBlock;
use core_foundation::base::{CFType, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};
use futures::channel::mpsc::UnboundedSender;
use objc2_app_kit::{NSWorkspace, NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification, NSWorkspaceDidWakeNotification, NSWorkspaceWillPowerOffNotification, NSWorkspaceWillSleepNotification};
use objc2_foundation::{NSDistributedNotificationCenter, NSNotification, NSNotificationCenter, NSOperationQueue, NSProcessInfo, NSProcessInfoThermalStateDidChangeNotification, NSString};
use serde_json::{Value, json};

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventSourceSecondsSinceLastEventType(source: i32, event_type: u32) -> f64;
    fn CGSessionCopyCurrentDictionary() -> CFDictionaryRef;
}

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOPSCopyPowerSourcesInfo() -> *const std::ffi::c_void;
    fn IOPSGetProvidingPowerSourceType(snapshot: *const std::ffi::c_void) -> CFStringRef;
    fn IOPSNotificationCreateRunLoopSource(callback: extern "C" fn(*mut std::ffi::c_void), context: *mut std::ffi::c_void) -> *const std::ffi::c_void;
    fn IOPMAssertionCreateWithName(assertion_type: CFStringRef, level: u32, name: CFStringRef, id: *mut u32) -> i32;
    fn IOPMAssertionRelease(id: u32) -> i32;
    fn IOPMCopyCPUPowerStatus(status: *mut CFDictionaryRef) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRunLoopGetMain() -> *const std::ffi::c_void;
    fn CFRunLoopAddSource(run_loop: *const std::ffi::c_void, source: *const std::ffi::c_void, mode: CFStringRef);
    static kCFRunLoopDefaultMode: CFStringRef;
}

unsafe extern "C" {
    fn notify_register_dispatch(name: *const std::ffi::c_char, token: *mut i32, queue: *const std::ffi::c_void, handler: &block2::DynBlock<dyn Fn(i32)>) -> u32;
    static _dispatch_main_q: std::ffi::c_void;
}

/// kCGEventSourceStateCombinedSessionState and kCGAnyInputEventType.
const COMBINED_SESSION_STATE: i32 = 0;
const ANY_INPUT_EVENT: u32 = u32::MAX;
const ASSERTION_LEVEL_ON: u32 = 255;

/// powerMonitor.getSystemIdleTime: whole seconds since the last input event.
pub fn system_idle_time() -> u64 {
    unsafe { CGEventSourceSecondsSinceLastEventType(COMBINED_SESSION_STATE, ANY_INPUT_EVENT) as u64 }
}

/// powerMonitor.getSystemIdleState (Chromium `CalculateIdleState`): locked first, then idle past the threshold.
pub fn system_idle_state(threshold: u64) -> &'static str {
    if screen_locked() {
        return "locked";
    }
    if system_idle_time() >= threshold { "idle" } else { "active" }
}

fn screen_locked() -> bool {
    let dictionary = unsafe { CGSessionCopyCurrentDictionary() };
    if dictionary.is_null() {
        return false;
    }
    let dictionary: CFDictionary<CFString, CFType> = unsafe { CFDictionary::wrap_under_create_rule(dictionary) };
    dictionary.find(CFString::new("CGSSessionScreenIsLocked")).and_then(|value| value.downcast::<CFBoolean>()).is_some_and(bool::from)
}

/// powerMonitor.getCurrentThermalState: NSProcessInfo's state by Electron's names.
pub fn thermal_state() -> &'static str {
    thermal_state_name(NSProcessInfo::processInfo().thermalState().0)
}

fn thermal_state_name(state: isize) -> &'static str {
    match state {
        0 => "nominal",
        1 => "fair",
        2 => "serious",
        3 => "critical",
        _ => "unknown",
    }
}

/// powerMonitor.isOnBatteryPower: the providing power source is the battery (kIOPMBatteryPowerKey).
pub fn on_battery_power() -> bool {
    unsafe {
        let snapshot = IOPSCopyPowerSourcesInfo();
        if snapshot.is_null() {
            return false;
        }
        let source = IOPSGetProvidingPowerSourceType(snapshot);
        let on_battery = !source.is_null() && CFString::wrap_under_get_rule(source).to_string() == "Battery Power";
        CFType::wrap_under_create_rule(snapshot as _);
        on_battery
    }
}

/// The macOS CPU speed limit in percent (Electron's `speed-limit-change`), 100 when the system reports none.
fn speed_limit() -> i64 {
    let mut status: CFDictionaryRef = std::ptr::null();
    if unsafe { IOPMCopyCPUPowerStatus(&mut status) } != 0 || status.is_null() {
        return 100;
    }
    let status: CFDictionary<CFString, CFType> = unsafe { CFDictionary::wrap_under_create_rule(status) };
    status.find(CFString::new("CPU_Speed_Limit")).and_then(|value| value.downcast::<CFNumber>()).and_then(|number| number.to_i64()).unwrap_or(100)
}

/// powerSaveBlocker: an IOPM assertion per blocker, by Electron's ids, which count from 0.
#[derive(Default)]
pub struct PowerSaveBlockers {
    next_id: i64,
    assertions: HashMap<i64, u32>,
}

impl PowerSaveBlockers {
    pub fn start(&mut self, blocker_type: &str) -> i64 {
        let assertion_type = match blocker_type {
            "prevent-display-sleep" => "NoDisplaySleepAssertion",
            _ => "NoIdleSleepAssertion",
        };
        let mut assertion = 0;
        let name = CFString::new("Electron");
        let created = unsafe { IOPMAssertionCreateWithName(CFString::new(assertion_type).as_concrete_TypeRef(), ASSERTION_LEVEL_ON, name.as_concrete_TypeRef(), &mut assertion) };
        let id = self.next_id;
        self.next_id += 1;
        if created == 0 {
            self.assertions.insert(id, assertion);
        }
        id
    }

    pub fn stop(&mut self, id: i64) -> bool {
        let Some(assertion) = self.assertions.remove(&id) else { return false };
        unsafe { IOPMAssertionRelease(assertion) };
        true
    }

    pub fn is_started(&self, id: i64) -> bool {
        self.assertions.contains_key(&id)
    }
}

static BLOCKERS: LazyLock<Mutex<PowerSaveBlockers>> = LazyLock::new(Default::default);

/// mainThreadPower.ts: the answer to one `MainThreadPower` request.
pub fn request(method: &str, args: &[Value]) -> Result<Value, String> {
    let blockers = &BLOCKERS;
    let arg = |index: usize| args.get(index).cloned().unwrap_or(Value::Null);
    match method {
        "$getSystemIdleState" => Ok(json!(system_idle_state(arg(0).as_u64().unwrap_or(0)))),
        "$getSystemIdleTime" => Ok(json!(system_idle_time())),
        "$getCurrentThermalState" => Ok(json!(thermal_state())),
        "$isOnBatteryPower" => Ok(json!(on_battery_power())),
        "$startPowerSaveBlocker" => Ok(json!(blockers.lock().unwrap().start(arg(0).as_str().unwrap_or_default()))),
        "$stopPowerSaveBlocker" => Ok(json!(blockers.lock().unwrap().stop(arg(0).as_i64().unwrap_or(-1)))),
        "$isPowerSaveBlockerStarted" => Ok(json!(blockers.lock().unwrap().is_started(arg(0).as_i64().unwrap_or(-1)))),
        _ => Err(format!("MainThreadPower has no method {method}")),
    }
}

pub type PowerEvent = (&'static str, Vec<Value>);

static EVENTS: OnceLock<UnboundedSender<PowerEvent>> = OnceLock::new();
static ON_BATTERY: Mutex<Option<bool>> = Mutex::new(None);

/// powerMonitor's events as ExtHostPower methods, delivered on the main thread: suspend and resume, shutdown,
/// screen lock, thermal state, battery and speed limit changes. Call once; the observers live as long as the app.
pub fn watch(events: UnboundedSender<PowerEvent>) {
    if EVENTS.set(events).is_err() {
        return;
    }
    let send = |event: PowerEvent| {
        if let Some(events) = EVENTS.get() {
            let _ = events.unbounded_send(event);
        }
    };
    let queue = NSOperationQueue::mainQueue();
    let observe = |center: &NSNotificationCenter, name: &NSString, event: fn() -> PowerEvent| {
        let block = RcBlock::new(move |_: NonNull<NSNotification>| send(event()));
        let observer = unsafe { center.addObserverForName_object_queue_usingBlock(Some(name), None, Some(&queue), &block) };
        // The center keeps the block; the token is never removed, the app observes until it quits.
        std::mem::forget(observer);
    };
    let workspace = NSWorkspace::sharedWorkspace().notificationCenter();
    observe(&workspace, unsafe { NSWorkspaceWillSleepNotification }, || ("$onDidSuspend", Vec::new()));
    observe(&workspace, unsafe { NSWorkspaceDidWakeNotification }, || ("$onDidResume", Vec::new()));
    observe(&workspace, unsafe { NSWorkspaceWillPowerOffNotification }, || ("$onWillShutdown", Vec::new()));
    let distributed = NSDistributedNotificationCenter::defaultCenter();
    observe(&distributed, &NSString::from_str("com.apple.screenIsLocked"), || ("$onDidLockScreen", Vec::new()));
    observe(&distributed, &NSString::from_str("com.apple.screenIsUnlocked"), || ("$onDidUnlockScreen", Vec::new()));
    observe(&NSNotificationCenter::defaultCenter(), unsafe { NSProcessInfoThermalStateDidChangeNotification }, || ("$onDidChangeThermalState", vec![json!(thermal_state())]));

    *ON_BATTERY.lock().unwrap() = Some(on_battery_power());
    unsafe {
        let source = IOPSNotificationCreateRunLoopSource(power_sources_changed, std::ptr::null_mut());
        CFRunLoopAddSource(CFRunLoopGetMain(), source, kCFRunLoopDefaultMode);
    }
    let mut token = 0;
    let handler = RcBlock::new(move |_: i32| send(("$onDidChangeSpeedLimit", vec![json!(speed_limit())])));
    unsafe { notify_register_dispatch(c"com.apple.system.power.CPU".as_ptr(), &mut token, &_dispatch_main_q as *const _, &handler) };
}

/// Electron's `nativeTheme` `updated` for `prefersReducedMotion` (accessibilityService.ts): the macOS accessibility
/// display options changed. Call once; the observer lives as long as the app.
pub fn watch_accessibility_display(changed: UnboundedSender<()>) {
    let block = RcBlock::new(move |_: NonNull<NSNotification>| drop(changed.unbounded_send(())));
    let center = NSWorkspace::sharedWorkspace().notificationCenter();
    let observer = unsafe { center.addObserverForName_object_queue_usingBlock(Some(NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification), None, Some(&NSOperationQueue::mainQueue()), &block) };
    std::mem::forget(observer);
}

/// powerMonitor `on-ac` and `on-battery`: sent when the providing source changes, not on every battery update.
extern "C" fn power_sources_changed(_: *mut std::ffi::c_void) {
    let on_battery = on_battery_power();
    let mut last = ON_BATTERY.lock().unwrap();
    if *last == Some(on_battery) {
        return;
    }
    *last = Some(on_battery);
    if let Some(events) = EVENTS.get() {
        let _ = events.unbounded_send(("$onDidChangeOnBatteryPower", vec![json!(on_battery)]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thermal_states_use_electrons_names() {
        let names: Vec<&str> = (-1..5).map(thermal_state_name).collect();
        assert_eq!(names, ["unknown", "nominal", "fair", "serious", "critical", "unknown"]);
    }

    #[test]
    fn power_save_blockers_count_from_zero_and_stop_once() {
        let mut blockers = PowerSaveBlockers::default();
        let first = blockers.start("prevent-app-suspension");
        let second = blockers.start("prevent-display-sleep");
        assert_eq!((first, second), (0, 1));
        assert!(blockers.is_started(first) && blockers.is_started(second));
        assert!(blockers.stop(first));
        assert!(!blockers.stop(first));
        assert!(!blockers.is_started(first));
        assert!(blockers.stop(second));
    }

    #[test]
    fn idle_state_follows_the_threshold() {
        assert_eq!(system_idle_state(u64::MAX), if screen_locked() { "locked" } else { "active" });
    }
}
