use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;

use super::{Candidate, GlobalShortcuts, Rejection, Selection, ShortcutRegistry, select};
use crate::keybindings::{Item, parse_keybinding, user_keybindings};

fn user_items(entries: serde_json::Value) -> Vec<Item> {
    static FILES: AtomicUsize = AtomicUsize::new(0);
    let file = std::env::temp_dir().join(format!("code-gpui-system-wide-{}-{}.json", std::process::id(), FILES.fetch_add(1, Ordering::Relaxed)));
    std::fs::write(&file, entries.to_string()).unwrap();
    let items = user_keybindings(&file);
    std::fs::remove_file(&file).unwrap();
    items
}

fn candidate(accelerator: &str, key: &str, command: &str, args: Option<serde_json::Value>, has_when: bool) -> Candidate {
    Candidate { accelerator: accelerator.into(), chord: parse_keybinding(key)[0], command: command.into(), args, user_settings_label: key.into(), has_when }
}

fn rejection(command: &str, label: &str) -> Rejection {
    Rejection { command: command.into(), user_settings_label: label.into() }
}

#[test]
fn selects_only_user_system_wide_single_combo_bindings_and_preserves_args_and_when() {
    let mut items = user_items(json!([
        { "key": "ctrl+cmd+a", "command": "workbench.action.openAgentsWindow", "args": { "foo": 1 }, "when": "editorFocus", "systemWide": true },
        { "key": "cmd+b", "command": "noop.notSystemWide" },
        { "key": "cmd+c", "command": "noop.default", "systemWide": true },
        { "key": "cmd+d", "command": "", "systemWide": true },
        { "key": "cmd+e", "command": "noop.stringFlag", "systemWide": "true" },
    ]));
    items[2].is_default = true;

    let expected = Selection { candidates: vec![candidate("Ctrl+Cmd+A", "ctrl+cmd+a", "workbench.action.openAgentsWindow", Some(json!({ "foo": 1 })), true)], ..Selection::default() };
    assert_eq!(select(&items), expected);
}

#[test]
fn reports_chords_and_single_modifier_bindings_as_unsupported() {
    let items = user_items(json!([
        { "key": "cmd+k cmd+c", "command": "noop.chord", "systemWide": true },
        { "key": "shift+shift", "command": "noop.modifier", "systemWide": true },
        { "key": "cmd+numpad0", "command": "noop.numpad", "systemWide": true },
    ]));

    let unsupported = vec![rejection("noop.chord", "cmd+k cmd+c"), rejection("noop.modifier", "shift"), rejection("noop.numpad", "cmd+numpad0")];
    assert_eq!(select(&items), Selection { unsupported, ..Selection::default() });
}

#[test]
fn keeps_the_first_binding_on_accelerator_conflicts() {
    let items = user_items(json!([
        { "key": "cmd+a", "command": "first.wins", "systemWide": true },
        { "key": "cmd+a", "command": "second.loses", "systemWide": true },
    ]));

    let expected = Selection { candidates: vec![candidate("Cmd+A", "cmd+a", "first.wins", None, false)], duplicates: vec![rejection("second.loses", "cmd+a")], ..Selection::default() };
    assert_eq!(select(&items), expected);
}

#[test]
fn accelerators_use_electrons_key_names() {
    let accelerator = |key: &str| crate::keybindings::electron_accelerator(&parse_keybinding(key));
    assert_eq!(accelerator("ctrl+shift+alt+cmd+up"), Some("Ctrl+Shift+Alt+Cmd+Up".into()));
    assert_eq!(accelerator("alt+delete"), Some("Alt+Delete".into()));
    assert_eq!(accelerator("cmd+;"), Some("Cmd+;".into()));
    assert_eq!(accelerator("ctrl+f19"), Some("Ctrl+F19".into()));
}

#[derive(Default)]
struct FakeGlobalShortcut {
    registered: Vec<String>,
    fail_for: HashSet<String>,
    unregister_calls: Vec<String>,
}

impl ShortcutRegistry for FakeGlobalShortcut {
    fn register(&mut self, binding: &Candidate) -> bool {
        if self.fail_for.contains(&binding.accelerator) {
            return false;
        }
        self.registered.push(binding.accelerator.clone());
        true
    }

    fn unregister(&mut self, accelerator: &str) {
        self.unregister_calls.push(accelerator.into());
        self.registered.retain(|registered| registered != accelerator);
    }
}

fn binding(accelerator: &str, command: &str) -> Candidate {
    candidate(accelerator, "ctrl+cmd+a", command, None, false)
}

#[test]
fn registers_desired_accelerators_and_reports_no_failures() {
    let mut service = GlobalShortcuts::new(FakeGlobalShortcut::default());
    let failed = service.update(vec![binding("Control+Cmd+A", "a"), binding("Control+Cmd+B", "b")]);

    assert_eq!(failed, Vec::<String>::new());
    assert_eq!(service.registry.registered, ["Control+Cmd+A", "Control+Cmd+B"]);
}

#[test]
fn unregisters_accelerators_removed_on_a_subsequent_update() {
    let mut service = GlobalShortcuts::new(FakeGlobalShortcut::default());
    service.update(vec![binding("Control+Cmd+A", "a"), binding("Control+Cmd+B", "b")]);
    service.update(vec![binding("Control+Cmd+A", "a")]);

    assert_eq!(service.registry.registered, ["Control+Cmd+A"]);
    assert_eq!(service.registry.unregister_calls, ["Control+Cmd+B"]);
}

#[test]
fn returns_the_labels_that_failed_to_register_and_retries_them_later() {
    let mut service = GlobalShortcuts::new(FakeGlobalShortcut::default());
    service.registry.fail_for.insert("Control+Cmd+A".into());

    assert_eq!(service.update(vec![binding("Control+Cmd+A", "a")]), ["ctrl+cmd+a"]);
    service.registry.fail_for.clear();
    assert_eq!(service.update(vec![binding("Control+Cmd+A", "a")]), Vec::<String>::new());
    assert_eq!(service.registry.registered, ["Control+Cmd+A"]);
}

#[test]
fn deduplicates_accelerators_within_one_payload() {
    let mut service = GlobalShortcuts::new(FakeGlobalShortcut::default());
    let failed = service.update(vec![binding("Control+Cmd+A", "first"), binding("Control+Cmd+A", "second")]);

    assert_eq!(failed, Vec::<String>::new());
    assert_eq!(service.registry.registered.len(), 1);
    assert_eq!(service.binding("Control+Cmd+A").map(|binding| binding.command.as_str()), Some("first"));
}

#[test]
fn a_trigger_reads_the_current_binding_with_its_args() {
    let mut service = GlobalShortcuts::new(FakeGlobalShortcut::default());
    service.update(vec![binding("Control+Cmd+A", "old")]);
    let with_args = Candidate { args: Some(json!({ "foo": "bar" })), ..binding("Control+Cmd+A", "workbench.action.openAgentsWindow") };
    service.update(vec![with_args.clone()]);

    assert_eq!(service.registry.registered, ["Control+Cmd+A"], "registered once, with a callback reading the current binding");
    assert_eq!(service.binding("Control+Cmd+A"), Some(&with_args));
    assert_eq!(service.binding("Control+Cmd+B"), None);
}
