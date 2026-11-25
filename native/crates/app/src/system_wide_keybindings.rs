//! System-wide keybindings: `"systemWide": true` in keybindings.json registers the key as an OS global shortcut that
//! runs its command while another application is active. systemWideKeybindings.ts selects the bindings,
//! globalKeybindingsMainService.ts keeps the OS registrations in step with them, and `HotKeys` registers with
//! Carbon's RegisterEventHotKey, as Electron's globalShortcut does on macOS.

use std::collections::{HashMap, HashSet};
use std::ffi::c_void;

use futures::channel::mpsc::UnboundedSender;
use serde_json::Value;

use crate::keybindings::{self, Chord, Item};

#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub accelerator: String,
    pub chord: Chord,
    pub command: String,
    pub args: Option<Value>,
    pub user_settings_label: String,
    pub has_when: bool,
}

#[derive(Debug, PartialEq)]
pub struct Rejection {
    pub command: String,
    pub user_settings_label: String,
}

#[derive(Debug, Default, PartialEq)]
pub struct Selection {
    pub candidates: Vec<Candidate>,
    pub unsupported: Vec<Rejection>,
    pub duplicates: Vec<Rejection>,
}

/// `selectSystemWideKeybindings` without a window context: the user's system-wide keybindings with an Electron
/// accelerator, the first one winning an accelerator. Only chat's screenshot action marks a default rule
/// system-wide, so the defaults branch selects nothing in the port.
pub fn select(items: &[Item]) -> Selection {
    let mut selection = Selection::default();
    let mut seen = HashSet::new();
    for item in items {
        if !item.system_wide || item.is_default || item.command.is_empty() || item.keybinding.is_empty() {
            continue;
        }
        let user_settings_label = keybindings::user_settings_label(&item.keybinding);
        let Some(accelerator) = keybindings::electron_accelerator(&item.keybinding) else {
            selection.unsupported.push(Rejection { command: item.command.clone(), user_settings_label });
            continue;
        };
        if !seen.insert(accelerator.clone()) {
            selection.duplicates.push(Rejection { command: item.command.clone(), user_settings_label });
            continue;
        }
        let chord = item.keybinding[0];
        selection.candidates.push(Candidate { accelerator, chord, command: item.command.clone(), args: item.args.clone(), user_settings_label, has_when: item.when.is_some() });
    }
    selection
}

/// Electron's `globalShortcut`: `register` is false when the OS or another application holds the accelerator.
pub trait ShortcutRegistry {
    fn register(&mut self, binding: &Candidate) -> bool;
    fn unregister(&mut self, accelerator: &str);
}

/// `GlobalKeybindingsMainService` for the port's one window.
pub struct GlobalShortcuts<R> {
    pub registry: R,
    bindings: Vec<Candidate>,
    registered: HashSet<String>,
    failed: HashSet<String>,
}

impl<R: ShortcutRegistry> GlobalShortcuts<R> {
    pub fn new(registry: R) -> Self {
        GlobalShortcuts { registry, bindings: Vec::new(), registered: HashSet::new(), failed: HashSet::new() }
    }

    /// `updateKeybindings` and `reconcile`: unregisters what is no longer wanted, registers the rest (retrying
    /// earlier failures) and returns the labels that failed.
    pub fn update(&mut self, candidates: Vec<Candidate>) -> Vec<String> {
        let mut bindings: Vec<Candidate> = Vec::new();
        for candidate in candidates {
            if bindings.iter().any(|binding| binding.accelerator == candidate.accelerator) {
                eprintln!("[GlobalKeybindings] duplicate accelerator '{}', keeping first", candidate.accelerator);
                continue;
            }
            bindings.push(candidate);
        }
        self.bindings = bindings;

        let unwanted: Vec<String> = self.registered.iter().filter(|accelerator| self.binding(accelerator).is_none()).cloned().collect();
        for accelerator in unwanted {
            self.registry.unregister(&accelerator);
            self.registered.remove(&accelerator);
            self.failed.remove(&accelerator);
        }
        for binding in &self.bindings {
            if self.registered.contains(&binding.accelerator) {
                continue;
            }
            if self.registry.register(binding) {
                self.registered.insert(binding.accelerator.clone());
                self.failed.remove(&binding.accelerator);
            } else {
                self.failed.insert(binding.accelerator.clone());
                eprintln!("[GlobalKeybindings] failed to register accelerator '{}' (already taken by the OS or another application)", binding.accelerator);
            }
        }
        self.bindings.iter().filter(|binding| self.failed.contains(&binding.accelerator)).map(|binding| binding.user_settings_label.clone()).collect()
    }

    /// The binding a registered accelerator runs when it fires (`onTrigger` reads the current one).
    pub fn binding(&self, accelerator: &str) -> Option<&Candidate> {
        self.bindings.iter().find(|binding| binding.accelerator == accelerator)
    }
}

/// SystemWideKeybindingsContribution: the registrations once started, and the warnings each sync returns for
/// the workbench to notify.
#[derive(Default)]
pub struct SystemWide {
    pub shortcuts: Option<GlobalShortcuts<HotKeys>>,
    candidates: Vec<Candidate>,
    warned_when_labels: HashSet<String>,
    reported_failures: Vec<String>,
}

impl SystemWide {
    pub fn set_keybindings(&mut self, items: &[Item]) -> Vec<String> {
        let selection = select(items);
        for rejection in &selection.unsupported {
            eprintln!("[SystemWideKeybindings] '{}' cannot be registered as a system-wide shortcut (only single key combinations are supported).", rejection.user_settings_label);
        }
        for rejection in &selection.duplicates {
            eprintln!("[SystemWideKeybindings] duplicate system-wide accelerator for '{}', keeping the first binding.", rejection.user_settings_label);
        }
        let mut newly_warned = Vec::new();
        for candidate in &selection.candidates {
            if candidate.has_when && self.warned_when_labels.insert(candidate.user_settings_label.clone()) {
                newly_warned.push(candidate.user_settings_label.clone());
            }
        }
        let mut warnings = Vec::new();
        if !newly_warned.is_empty() {
            warnings.push(format!("The \"when\" clause is ignored for system-wide keybindings ({}); they are always active while Code - OSS is running.", newly_warned.join(", ")));
        }
        self.candidates = selection.candidates;
        warnings.extend(self.sync());
        warnings
    }

    pub fn start(&mut self, hot_keys: HotKeys) -> Option<String> {
        self.shortcuts = Some(GlobalShortcuts::new(hot_keys));
        self.sync()
    }

    /// The binding of the hot key Carbon reported pressed.
    pub fn pressed(&self, id: u32) -> Option<&Candidate> {
        let shortcuts = self.shortcuts.as_ref()?;
        shortcuts.binding(shortcuts.registry.accelerator(id)?)
    }

    fn sync(&mut self) -> Option<String> {
        let shortcuts = self.shortcuts.as_mut()?;
        let mut failed = shortcuts.update(self.candidates.clone());
        failed.sort();
        if failed == self.reported_failures {
            return None;
        }
        let warning = (!failed.is_empty()).then(|| {
            format!("Some system-wide keybindings could not be registered ({}); the key combination may already be taken by the operating system or another application.", failed.join(", "))
        });
        self.reported_failures = failed;
        warning
    }
}

/// Carbon's `controlKey`, `shiftKey`, `optionKey` and `cmdKey`.
const CONTROL_KEY: u32 = 1 << 12;
const SHIFT_KEY: u32 = 1 << 9;
const OPTION_KEY: u32 = 1 << 11;
const CMD_KEY: u32 = 1 << 8;
/// Four-char codes: `kEventClassKeyboard` 'keyb', `kEventParamDirectObject` '----', `typeEventHotKeyID` 'hkid'.
const EVENT_CLASS_KEYBOARD: u32 = u32::from_be_bytes(*b"keyb");
const EVENT_HOT_KEY_PRESSED: u32 = 5;
const EVENT_PARAM_DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");
const TYPE_EVENT_HOT_KEY_ID: u32 = u32::from_be_bytes(*b"hkid");
const SIGNATURE: u32 = u32::from_be_bytes(*b"vscd");

#[repr(C)]
#[derive(Default)]
struct EventHotKeyId {
    signature: u32,
    id: u32,
}

#[repr(C)]
struct EventTypeSpec {
    event_class: u32,
    event_kind: u32,
}

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn GetApplicationEventTarget() -> *mut c_void;
    fn InstallEventHandler(target: *mut c_void, handler: extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> i32, count: usize, types: *const EventTypeSpec, user_data: *mut c_void, handler_ref: *mut *mut c_void) -> i32;
    fn RegisterEventHotKey(key_code: u32, modifiers: u32, id: EventHotKeyId, target: *mut c_void, options: u32, hot_key: *mut *mut c_void) -> i32;
    fn UnregisterEventHotKey(hot_key: *mut c_void) -> i32;
    fn GetEventParameter(event: *mut c_void, name: u32, desired_type: u32, actual_type: *mut u32, size: usize, actual_size: *mut usize, data: *mut c_void) -> i32;
}

/// The hot keys registered with Carbon; a press sends its id to the receiver `HotKeys::new` was given.
pub struct HotKeys {
    hot_keys: HashMap<String, (u32, *mut c_void)>,
    next_id: u32,
}

impl HotKeys {
    /// Installs the application's hot key handler; call it once, the handler lives as long as the process.
    pub fn new(presses: UnboundedSender<u32>) -> HotKeys {
        let spec = EventTypeSpec { event_class: EVENT_CLASS_KEYBOARD, event_kind: EVENT_HOT_KEY_PRESSED };
        let presses = Box::into_raw(Box::new(presses)) as *mut c_void;
        let status = unsafe { InstallEventHandler(GetApplicationEventTarget(), hot_key_pressed, 1, &spec, presses, std::ptr::null_mut()) };
        assert_eq!(status, 0, "InstallEventHandler");
        HotKeys { hot_keys: HashMap::new(), next_id: 1 }
    }

    pub fn accelerator(&self, id: u32) -> Option<&str> {
        self.hot_keys.iter().find(|(_, (hot_key_id, _))| *hot_key_id == id).map(|(accelerator, _)| accelerator.as_str())
    }
}

impl ShortcutRegistry for HotKeys {
    fn register(&mut self, binding: &Candidate) -> bool {
        let chord = &binding.chord;
        // Electron maps the accelerator's key to the virtual key code of its US position.
        let Some(key_code) = keybindings::us_scan_code(chord.key).and_then(crate::keyboard_layout::mac_key_code) else { return false };
        let modifiers = [(chord.ctrl, CONTROL_KEY), (chord.shift, SHIFT_KEY), (chord.alt, OPTION_KEY), (chord.meta, CMD_KEY)];
        let modifiers = modifiers.iter().filter(|(pressed, _)| *pressed).fold(0, |all, (_, modifier)| all | modifier);
        let id = self.next_id;
        self.next_id += 1;
        let mut hot_key = std::ptr::null_mut();
        let status = unsafe { RegisterEventHotKey(key_code as u32, modifiers, EventHotKeyId { signature: SIGNATURE, id }, GetApplicationEventTarget(), 0, &mut hot_key) };
        if status != 0 {
            return false;
        }
        self.hot_keys.insert(binding.accelerator.clone(), (id, hot_key));
        true
    }

    fn unregister(&mut self, accelerator: &str) {
        if let Some((_, hot_key)) = self.hot_keys.remove(accelerator) {
            unsafe { UnregisterEventHotKey(hot_key) };
        }
    }
}

extern "C" fn hot_key_pressed(_call: *mut c_void, event: *mut c_void, presses: *mut c_void) -> i32 {
    let mut id = EventHotKeyId::default();
    let status = unsafe { GetEventParameter(event, EVENT_PARAM_DIRECT_OBJECT, TYPE_EVENT_HOT_KEY_ID, std::ptr::null_mut(), size_of::<EventHotKeyId>(), std::ptr::null_mut(), &mut id as *mut EventHotKeyId as *mut c_void) };
    if status == 0 && id.signature == SIGNATURE {
        let presses = unsafe { &*(presses as *const UnboundedSender<u32>) };
        let _ = presses.unbounded_send(id.id);
    }
    status
}

#[cfg(test)]
mod tests;
