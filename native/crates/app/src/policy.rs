//! nativePolicyService.ts over @vscode/policy-watcher's macOS side: each policy is read with
//! `CFPreferencesCopyAppValue(name, darwinBundleIdentifier)`, so a managed profile (or `defaults write`) sets it.

use std::ffi::{CString, c_void};

use config::policy::{PolicyValues, policy_configurations};
use config::registry::Registry;
use serde_json::Value;

/// product.json `darwinBundleIdentifier`.
pub const BUNDLE_IDENTIFIER: &str = "com.rokartur.vsrustium";

const UTF8: u32 = 0x0800_0100;
const LONG_LONG: isize = 11;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringCreateWithCString(allocator: *const c_void, text: *const i8, encoding: u32) -> *const c_void;
    fn CFPreferencesCopyAppValue(key: *const c_void, application: *const c_void) -> *const c_void;
    fn CFGetTypeID(object: *const c_void) -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFNumberGetTypeID() -> usize;
    fn CFBooleanGetTypeID() -> usize;
    fn CFStringGetLength(text: *const c_void) -> isize;
    fn CFStringGetMaximumSizeForEncoding(length: isize, encoding: u32) -> isize;
    fn CFStringGetCString(text: *const c_void, buffer: *mut i8, size: isize, encoding: u32) -> bool;
    fn CFNumberGetValue(number: *const c_void, kind: isize, value: *mut c_void) -> bool;
    fn CFBooleanGetValue(boolean: *const c_void) -> bool;
    fn CFRelease(object: *const c_void);
}

/// `createWatcher(productName, policyDefinitions)`'s first update: the value of every policy the registry
/// defines, read as the type `PolicyConfiguration.toPolicyDefinitionType` gives its setting.
pub fn read_native_policies(registry: &Registry, application: &str) -> PolicyValues {
    let mut values = PolicyValues::new();
    for (name, settings) in policy_configurations(registry) {
        // The owner's schema decides the type; a reference only stands in when no owner is registered.
        let schema = settings.iter().find(|(_, schema)| schema.get("policy").is_some()).unwrap_or(&settings[0]).1;
        let Some(kind) = definition_type(&schema["type"]) else { continue };
        if let Some(value) = read_preference(application, &name, kind) {
            values.insert(name, value);
        }
    }
    values
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    String,
    Number,
    Boolean,
}

/// `toPolicyDefinitionType`: number over boolean over string; arrays and objects travel as JSON strings.
fn definition_type(schema_type: &Value) -> Option<Kind> {
    let types: Vec<&Value> = match schema_type {
        Value::Array(types) => types.iter().collect(),
        kind => vec![kind],
    };
    let supported: Vec<&str> = types.iter().filter_map(|kind| kind.as_str()).filter(|kind| ["string", "number", "array", "object", "boolean"].contains(kind)).collect();
    if supported.is_empty() {
        return None;
    }
    Some(if supported.contains(&"number") {
        Kind::Number
    } else if supported.contains(&"boolean") {
        Kind::Boolean
    } else {
        Kind::String
    })
}

/// StringPolicy, NumberPolicy and BooleanPolicy `read()`: a value of another type counts as unset.
fn read_preference(application: &str, name: &str, kind: Kind) -> Option<Value> {
    let application = cf_string(application);
    let key = cf_string(name);
    let value = unsafe { CFPreferencesCopyAppValue(key, application) };
    unsafe {
        CFRelease(key);
        CFRelease(application);
    }
    if value.is_null() {
        return None;
    }
    let result = unsafe {
        let type_id = CFGetTypeID(value);
        match kind {
            Kind::String if type_id == CFStringGetTypeID() => {
                let size = CFStringGetMaximumSizeForEncoding(CFStringGetLength(value), UTF8) + 1;
                let mut buffer = vec![0i8; size as usize];
                CFStringGetCString(value, buffer.as_mut_ptr(), size, UTF8).then(|| {
                    let bytes: Vec<u8> = buffer.iter().take_while(|byte| **byte != 0).map(|byte| *byte as u8).collect();
                    Value::from(String::from_utf8_lossy(&bytes).into_owned())
                })
            }
            Kind::Number if type_id == CFNumberGetTypeID() => {
                let mut number: i64 = 0;
                CFNumberGetValue(value, LONG_LONG, (&mut number as *mut i64).cast()).then(|| Value::from(number))
            }
            Kind::Boolean if type_id == CFBooleanGetTypeID() => Some(Value::from(CFBooleanGetValue(value))),
            _ => None,
        }
    };
    unsafe { CFRelease(value) };
    result
}

fn cf_string(text: &str) -> *const c_void {
    let text = CString::new(text).expect("policy names and bundle ids have no NUL");
    unsafe { CFStringCreateWithCString(std::ptr::null(), text.as_ptr(), UTF8) }
}

/// PolicyWatcher.cc's stream: FSEvents on the folder managed profiles write, with its 1 second latency.
pub const MANAGED_PREFERENCES: &str = "/Library/Managed Preferences/";

/// `FSEventStreamCreate` over `path` on its own serial queue (PolicyWatcher::Start); `on_change` runs on that queue
/// for each batch of events. FSEvents watches a path that does not exist yet. The stream lives as long as the process.
pub fn watch_policy_changes(path: &str, on_change: impl Fn() + Send + 'static) {
    type Callback = Box<dyn Fn() + Send>;
    extern "C" fn fs_events(_stream: *const c_void, info: *mut c_void, _count: usize, _paths: *mut c_void, _flags: *const u32, _ids: *const u64) {
        let on_change = unsafe { &*(info as *const Callback) };
        on_change();
    }
    let info: *mut Callback = Box::into_raw(Box::new(Box::new(on_change)));
    let label = CString::new(format!("{BUNDLE_IDENTIFIER}.policy-watcher")).expect("no NUL");
    unsafe {
        let path = cf_string(path);
        let paths = CFArrayCreate(std::ptr::null(), &path, 1, std::ptr::null());
        let context = FSEventStreamContext { version: 0, info: info.cast(), retain: std::ptr::null(), release: std::ptr::null(), copy_description: std::ptr::null() };
        let stream = FSEventStreamCreate(std::ptr::null(), fs_events, &context, paths, SINCE_NOW, 1.0, 0);
        CFRelease(paths);
        CFRelease(path);
        let queue = dispatch_queue_create(label.as_ptr(), std::ptr::null());
        FSEventStreamSetDispatchQueue(stream, queue);
        FSEventStreamStart(stream);
    }
}

const SINCE_NOW: u64 = u64::MAX;

#[repr(C)]
struct FSEventStreamContext {
    version: isize,
    info: *mut c_void,
    retain: *const c_void,
    release: *const c_void,
    copy_description: *const c_void,
}

type FSEventStreamCallback = extern "C" fn(*const c_void, *mut c_void, usize, *mut c_void, *const u32, *const u64);

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn CFArrayCreate(allocator: *const c_void, values: *const *const c_void, count: isize, callbacks: *const c_void) -> *const c_void;
    fn FSEventStreamCreate(allocator: *const c_void, callback: FSEventStreamCallback, context: *const FSEventStreamContext, paths: *const c_void, since_when: u64, latency: f64, flags: u32) -> *mut c_void;
    fn FSEventStreamSetDispatchQueue(stream: *mut c_void, queue: *mut c_void);
    fn FSEventStreamStart(stream: *mut c_void) -> bool;
}

unsafe extern "C" {
    fn dispatch_queue_create(label: *const i8, attributes: *const c_void) -> *mut c_void;
}

#[cfg(test)]
mod tests;
