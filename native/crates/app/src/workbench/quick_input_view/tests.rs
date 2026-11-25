use std::path::Path;
use std::sync::{Arc, Mutex};

use exthost::scan::scan_extension;
use gpui::{Modifiers, MouseButton, MouseDownEvent, TestAppContext, point, px};

use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::load_theme;
use crate::workbench::Workbench;
use storage::Storage;

#[gpui::test]
fn dragging_the_header_moves_the_quick_input_and_a_double_click_puts_it_back(cx: &mut TestAppContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-drag-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let storage = Arc::new(Mutex::new(Storage::in_memory()));
    let profile = storage.clone();
    let (workbench, cx) = cx.add_window_view(|_, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), crate::default_settings(), HostSync::new(std::sync::mpsc::channel().0), cx);
        workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), dir.join("keybindings.json"), cx);
        workbench.restore_profile_state(profile);
        workbench
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("shift-cmd-p");
    let bounds = |cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.quick_inputs.bounds.get());
    let start = bounds(cx);

    // The input box, here its top border, is not a drag area.
    let in_input = start.origin + point(start.size.width / 2., px(6.5));
    cx.simulate_mouse_down(in_input, MouseButton::Left, Modifiers::none());
    assert!(workbench.read_with(cx, |workbench, _| workbench.quick_inputs.drag.is_none()));
    cx.simulate_mouse_up(in_input, MouseButton::Left, Modifiers::none());

    // The header's 6px padding is.
    let grab = start.origin + point(px(3.), px(3.));
    cx.simulate_mouse_down(grab, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(grab + point(px(-100.), px(100.)), MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(grab + point(px(-100.), px(100.)), MouseButton::Left, Modifiers::none());
    let moved = bounds(cx);
    assert_eq!(moved.origin - start.origin, point(px(-100.), px(100.)));
    let view_state = workbench.read_with(cx, |workbench, _| workbench.quick_inputs.view_state);
    assert!(view_state.is_some());
    let stored = storage.lock().unwrap().get("workbench.quickInput.viewState").map(str::to_owned).expect("the drag end stores the position");
    assert!(stored.starts_with(r#"{"top":"#) && stored.contains(r#","left":"#), "{stored}");

    // Released: moving the pointer leaves it in place.
    cx.simulate_mouse_move(grab, None, Modifiers::none());
    assert_eq!(bounds(cx).origin, moved.origin);

    let header = moved.origin + point(px(3.), px(3.));
    cx.simulate_event(MouseDownEvent { position: header, modifiers: Modifiers::none(), button: MouseButton::Left, click_count: 2, first_mouse: false });
    cx.simulate_mouse_up(header, MouseButton::Left, Modifiers::none());
    assert_eq!(bounds(cx).origin, start.origin);
    assert_eq!(storage.lock().unwrap().get("workbench.quickInput.viewState"), None, "the reset removes the key");

    // The next window reads the stored position.
    storage.lock().unwrap().set("workbench.quickInput.viewState", stored);
    workbench.update(cx, |workbench, _| workbench.load_quick_input_view_state());
    assert_eq!(workbench.read_with(cx, |workbench, _| workbench.quick_inputs.view_state), view_state);
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn the_quick_input_stays_open_while_the_window_is_inactive(cx: &mut TestAppContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-deactivate-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (workbench, cx) = cx.add_window_view(|_, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), crate::default_settings(), HostSync::new(std::sync::mpsc::channel().0), cx);
        workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), dir.join("keybindings.json"), cx);
        workbench
    });
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    cx.simulate_keystrokes("shift-cmd-p");
    let shown = |cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.quick_inputs.visible.is_some());
    assert!(shown(cx));

    cx.deactivate_window();
    assert!(shown(cx), "a window deactivation is no blur");
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    cx.simulate_keystrokes("a");
    let value = workbench.read_with(cx, |workbench, _| workbench.quick_inputs.current().map(|input| input.input_box.value.clone()));
    assert_eq!(value.as_deref(), Some(">a"));

    cx.update(|window, cx| window.blur(cx));
    cx.run_until_parked();
    assert!(!shown(cx), "a real focus out hides it");
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn the_word_highlight_stops_while_the_quick_input_has_the_focus(cx: &mut TestAppContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-word-blur-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("a.txt");
    std::fs::write(&file, "the cat\nthe dog").unwrap();
    let (workbench, cx) = cx.add_window_view(|window, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), crate::default_settings(), HostSync::new(std::sync::mpsc::channel().0), cx);
        workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), dir.join("keybindings.json"), cx);
        workbench.watch_editor_focus(window, cx);
        workbench.open_file(file.clone(), true, window, cx);
        workbench
    });
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    let highlights = |cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().highlights.len());
    assert_eq!(highlights(cx), 2);

    cx.simulate_keystrokes("cmd-p");
    assert_eq!(highlights(cx), 0, "onDidBlurEditorWidget");
    cx.deactivate_window();
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    assert_eq!(highlights(cx), 0);
    cx.simulate_keystrokes("escape");
    assert_eq!(highlights(cx), 2, "onDidFocusEditorText");

    // A picked command edits before GPUI reports the restored focus; the edit stops upstream's pending query.
    cx.simulate_keystrokes("cmd-p");
    workbench.update_in(cx, |workbench, window, cx| {
        workbench.editors.active_mut().unwrap().type_text("x");
        window.focus(&workbench.editor_focus.clone(), cx);
    });
    cx.run_until_parked();
    assert_eq!(highlights(cx), 0, "onDidChangeModelContent");

    // A window deactivation keeps the DOM focus: the highlights stay, and the reactivation runs no query, so an
    // edit made meanwhile (a dialog's action) leaves them off.
    cx.simulate_keystrokes("cmd-z");
    workbench.update(cx, |workbench, _| workbench.editors.active_mut().unwrap().refresh_highlights());
    assert_eq!(highlights(cx), 2);
    cx.deactivate_window();
    cx.run_until_parked();
    assert_eq!(highlights(cx), 2);
    workbench.update(cx, |workbench, _| _ = workbench.editors.active_mut().unwrap().type_text("x"));
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    assert_eq!(highlights(cx), 0);
    std::fs::remove_dir_all(dir).unwrap();
}

/// quickinput.test.ts "keepScrollPosition - works with activeItems" and "keepScrollPosition - works with items".
#[gpui::test]
fn keep_scroll_position_keeps_the_list_where_it_is(cx: &mut TestAppContext) {
    use crate::main_thread_quick_open::Owner;
    use crate::quick_input::{Entry, Item, Kind, QuickInput};

    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), crate::default_settings(), HostSync::new(std::sync::mpsc::channel().0), cx));
    let items: Vec<Entry> = (0..1000).map(|handle| Entry::Item(Item { handle, label: format!("item {handle}"), pickable: true, ..Item::default() })).collect();
    let id = workbench.update_in(cx, |workbench, window, cx| {
        let id = workbench.quick_inputs.add(QuickInput::new(0, Kind::QuickPick), Owner::Session(1));
        workbench.show_quick_input(id, window, cx);
        let input = workbench.quick_inputs.get_mut(id).unwrap();
        input.set_items(items.clone());
        input.set_active_items(&[999]);
        id
    });
    cx.run_until_parked();
    let scroll_top = |cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.quick_inputs.list.logical_scroll_top().item_ix);
    let update = |cx: &mut gpui::VisualTestContext, keep: bool, set: &dyn Fn(&mut QuickInput)| {
        workbench.update(cx, |workbench, cx| {
            let input = workbench.quick_inputs.get_mut(id).unwrap();
            input.keep_scroll_position = keep;
            set(input);
            cx.notify();
        });
        cx.run_until_parked();
    };
    let bottom = scroll_top(cx);
    assert_ne!(bottom, 0);

    update(cx, true, &|input| input.set_active_items(&[0]));
    assert_eq!(scroll_top(cx), bottom);
    update(cx, false, &|input| input.set_active_items(&[0]));
    assert_eq!(scroll_top(cx), 0);

    update(cx, false, &|input| input.set_active_items(&[999]));
    assert_eq!(scroll_top(cx), bottom);
    update(cx, true, &|input| input.set_items(items.clone()));
    assert_eq!(scroll_top(cx), bottom);
    update(cx, false, &|input| input.set_items(items.clone()));
    assert_eq!(scroll_top(cx), 0);
}
