use super::{ICONS, IconSelectBox, IconTarget, KeyAction, profile_icon_file};
use gpui::Keystroke;

fn key(picker: &IconSelectBox, key: &str) -> Option<KeyAction> {
    picker.key_action(&Keystroke::parse(key).unwrap())
}

#[test]
fn the_search_keeps_icons_whose_id_contains_it_and_the_focused_icon() {
    let mut picker = IconSelectBox::new(IconTarget::NewProfile);
    assert_eq!(picker.rendered.len(), ICONS.len());
    picker.focus_icon(16);
    assert_eq!(picker.rendered[16], "terminal-debian");
    picker.search = crate::input_box::InputBox::new("TERMINAL".into());
    picker.filter();
    assert_eq!(picker.rendered, ["terminal", "terminal-debian", "terminal-linux", "terminal-ubuntu"]);
    assert_eq!(picker.focused, Some(1));
    picker.search = crate::input_box::InputBox::new("nothing".into());
    picker.filter();
    assert!(picker.rendered.is_empty());
    assert_eq!(picker.focused, None);
}

#[test]
fn rows_move_twelve_icons_and_leave_the_grid_for_the_input_from_the_first_row() {
    let mut picker = IconSelectBox::new(IconTarget::NewProfile);
    picker.focus_next_row();
    assert!(picker.grid_focused);
    assert_eq!(picker.focused, Some(0));
    picker.focus_next_row();
    assert_eq!(picker.focused, Some(12));
    picker.focus_last();
    picker.focus_next_row();
    assert_eq!(picker.focused, Some(82));
    picker.focus_previous_row();
    assert_eq!(picker.focused, Some(70));
    picker.focus_first();
    picker.focus_previous();
    assert_eq!(picker.focused, Some(0));
    picker.focus_previous_row();
    assert!(!picker.grid_focused);
}

#[test]
fn the_input_keeps_its_caret_keys_and_the_grid_takes_the_arrows() {
    let mut picker = IconSelectBox::new(IconTarget::NewProfile);
    assert_eq!(key(&picker, "left"), None);
    assert_eq!(key(&picker, "a"), None);
    assert_eq!(key(&picker, "down"), Some(KeyAction::NextRow));
    assert_eq!(key(&picker, "enter"), Some(KeyAction::Select));
    assert_eq!(key(&picker, "space"), None);
    assert_eq!(key(&picker, "escape"), Some(KeyAction::Close));
    picker.grid_focused = true;
    assert_eq!(key(&picker, "left"), Some(KeyAction::Previous));
    assert_eq!(key(&picker, "space"), Some(KeyAction::Select));
    assert_eq!(key(&picker, "shift-down"), None);
}

#[test]
fn the_default_icon_draws_the_settings_gear() {
    assert_eq!(profile_icon_file(None), "settings-gear");
    assert_eq!(profile_icon_file(Some("settings-view-bar-icon")), "settings-gear");
    assert_eq!(profile_icon_file(Some("zap")), "symbol-event");
    assert_eq!(profile_icon_file(Some("rocket")), "rocket");
}
