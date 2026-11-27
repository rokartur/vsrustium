//! Ported from platform/keybinding/test/common/abstractKeybindingService.test.ts: `_doDispatch` through the workbench's
//! window-level key listener. Keys are user settings strings; `shouldPreventDefault` is not observable from a GPUI
//! keystroke interceptor, so the cases check the commands run and the status messages.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use gpui::{Entity, Keystroke, TestAppContext, VisualTestContext};
use serde_json::Value;

use crate::commands::Outcome;
use crate::contextkey::Expr;
use crate::host_sync::HostSync;
use crate::keybindings::{Item, parse_keybinding};
use crate::languages::Languages;
use crate::theme::load_theme;
use crate::workbench::Workbench;

type Calls = Rc<RefCell<Vec<(String, Vec<Value>)>>>;

fn kb_item(keybinding: &str, command: &str, when: Option<&str>) -> Item {
    Item::new(parse_keybinding(keybinding), command, None, when.map(|when| Expr::deserialize(when).unwrap()), true)
}

/// A workbench whose only keybindings are `items`, with every command they name recording its calls.
fn workbench_with(items: Vec<Item>, cx: &mut TestAppContext) -> (Entity<Workbench>, &mut VisualTestContext, Calls) {
    let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
    let themes = exthost::scan::scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let calls: Calls = Rc::default();
    let (workbench, cx) = cx.add_window_view(|_, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), crate::default_settings(), HostSync::new(std::sync::mpsc::channel().0), cx);
        for id in items.iter().map(|item| item.command.clone()).filter(|id| !id.is_empty()) {
            let calls = calls.clone();
            workbench.commands.register(id.clone(), move |_, args, _, _| {
                calls.borrow_mut().push((id.clone(), args));
                Outcome::done()
            });
        }
        workbench.set_default_keybindings(items);
        workbench.set_user_keybindings(Vec::new());
        workbench
    });
    (workbench, cx, calls)
}

/// Presses `key` and returns the commands it ran, the chord status and the status bar message after it.
fn press(workbench: &Entity<Workbench>, key: &str, cx: &mut VisualTestContext, calls: &Calls) -> (Vec<String>, Option<String>, Option<String>) {
    workbench.update_in(cx, |workbench, window, cx| workbench.dispatch_key(&Keystroke::parse(key).unwrap(), window, cx));
    cx.run_until_parked();
    let ran = calls.borrow_mut().drain(..).map(|(id, _)| id).collect();
    workbench.read_with(cx, |workbench, _| (ran, workbench.chord_status(), workbench.status_message.as_ref().map(|(message, _)| message.clone())))
}

fn set_context(workbench: &Entity<Workbench>, key: &str, value: Option<bool>, cx: &mut VisualTestContext) {
    workbench.update(cx, |workbench, _| match value {
        Some(value) => drop(workbench.context_keys.insert(key.to_string(), Value::Bool(value))),
        None => drop(workbench.context_keys.remove(key)),
    });
}

const WAITING: &str = "(⌘K) was pressed. Waiting for second key of chord...";

#[gpui::test]
fn a_single_chord_keybinding_is_dispatched(cx: &mut TestAppContext) {
    let (workbench, cx, calls) = workbench_with(vec![kb_item("cmd+k", "myCommand", None)], cx);
    assert_eq!(press(&workbench, "cmd-k", cx, &calls), (vec!["myCommand".into()], None, None));
}

#[gpui::test]
fn a_multi_chord_keybinding_is_dispatched(cx: &mut TestAppContext) {
    let (workbench, cx, calls) = workbench_with(vec![kb_item("cmd+k cmd+i", "myCommand", None)], cx);
    assert_eq!(press(&workbench, "cmd-k", cx, &calls), (vec![], Some(WAITING.into()), None));
    assert_eq!(press(&workbench, "cmd-i", cx, &calls), (vec!["myCommand".into()], None, None));
}

#[gpui::test]
fn a_single_chord_keybinding_with_an_empty_command_unbinds_it(cx: &mut TestAppContext) {
    let (workbench, cx, calls) = workbench_with(vec![kb_item("cmd+k", "myCommand", None), kb_item("cmd+k", "", None)], cx);
    assert_eq!(press(&workbench, "cmd-k", cx, &calls), (vec![], None, None));
}

#[gpui::test]
fn a_multi_chord_keybinding_with_an_empty_command_is_not_a_command(cx: &mut TestAppContext) {
    let (workbench, cx, calls) = workbench_with(vec![kb_item("cmd+k cmd+i", "myCommand", None), kb_item("cmd+k cmd+i", "", None)], cx);
    assert_eq!(press(&workbench, "cmd-k", cx, &calls), (vec![], Some(WAITING.into()), None));
    assert_eq!(press(&workbench, "cmd-i", cx, &calls), (vec![], None, Some("The key combination (⌘K, ⌘I) is not a command.".into())));
}

#[gpui::test]
fn issue_16498_chord_mode_is_quit_for_invalid_chords(cx: &mut TestAppContext) {
    let (workbench, cx, calls) = workbench_with(vec![kb_item("cmd+k cmd+x", "chordCommand", None), kb_item("backspace", "simpleCommand", None)], cx);
    assert_eq!(press(&workbench, "cmd-k", cx, &calls), (vec![], Some(WAITING.into()), None));
    assert_eq!(press(&workbench, "backspace", cx, &calls), (vec![], None, Some("The key combination (⌘K, Backspace) is not a command.".into())));
    assert_eq!(press(&workbench, "backspace", cx, &calls).0, ["simpleCommand"]);
}

#[gpui::test]
fn keybindings_are_not_dispatched_while_an_ime_composition_is_in_progress(cx: &mut TestAppContext) {
    let (workbench, cx, calls) = workbench_with(vec![kb_item("cmd+k", "myCommand", None)], cx);
    workbench.update(cx, |workbench, _| workbench.marked_text = Some(0..1));
    assert_eq!(press(&workbench, "cmd-k", cx, &calls), (vec![], None, None));
    workbench.update(cx, |workbench, _| workbench.marked_text = None);
    assert_eq!(press(&workbench, "cmd-k", cx, &calls).0, ["myCommand"]);
}

#[gpui::test]
fn can_trigger_command_that_is_sharing_keybinding_with_chord(cx: &mut TestAppContext) {
    let (workbench, cx, calls) = workbench_with(vec![kb_item("cmd+k cmd+x", "chordCommand", None), kb_item("cmd+k", "simpleCommand", Some("key1"))], cx);
    set_context(&workbench, "key1", Some(true), cx);
    assert_eq!(press(&workbench, "cmd-k", cx, &calls), (vec!["simpleCommand".into()], None, None));
    set_context(&workbench, "key1", None, cx);
    assert_eq!(press(&workbench, "cmd-k", cx, &calls), (vec![], Some(WAITING.into()), None));
    assert_eq!(press(&workbench, "cmd-x", cx, &calls), (vec!["chordCommand".into()], None, None));
}

#[gpui::test]
fn cannot_trigger_chord_if_command_is_overwriting(cx: &mut TestAppContext) {
    let (workbench, cx, calls) = workbench_with(vec![kb_item("cmd+k cmd+x", "chordCommand", Some("key1")), kb_item("cmd+k", "simpleCommand", None)], cx);
    assert_eq!(press(&workbench, "cmd-k", cx, &calls), (vec!["simpleCommand".into()], None, None));
    set_context(&workbench, "key1", Some(true), cx);
    assert_eq!(press(&workbench, "cmd-k", cx, &calls), (vec!["simpleCommand".into()], None, None));
    assert_eq!(press(&workbench, "cmd-x", cx, &calls), (vec![], None, None));
}

#[gpui::test]
fn can_have_spying_command(cx: &mut TestAppContext) {
    let (workbench, cx, calls) = workbench_with(vec![kb_item("cmd+k", "^simpleCommand", None)], cx);
    assert_eq!(press(&workbench, "cmd-k", cx, &calls), (vec!["simpleCommand".into()], None, None));
}

/// contextkeys.ts `InputFocusedContext`: a focused code editor's text is an input, so `!inputFocus` keybindings stay
/// off there.
#[gpui::test]
fn a_focused_text_editor_is_an_input(cx: &mut TestAppContext) {
    let (workbench, cx, calls) = workbench_with(vec![kb_item("a", "outside", Some("!inputFocus"))], cx);
    assert_eq!(press(&workbench, "a", cx, &calls).0, ["outside"]);
    let input = workbench.update_in(cx, |workbench, window, cx| {
        window.focus(&workbench.editor_focus, cx);
        workbench.key_context(window).get("inputFocus").cloned()
    });
    assert_eq!(input, Some(Value::Bool(true)));
}
