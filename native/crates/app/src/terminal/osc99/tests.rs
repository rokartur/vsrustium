//! terminalNotification.test.ts.

use super::*;

fn env() -> Env {
    Env { enabled: true, window_focused: false, terminal_visible: true }
}

fn shown(effects: &[Effect]) -> Vec<Shown> {
    effects.iter().filter_map(|effect| if let Effect::Show(shown) = effect { Some(shown.clone()) } else { None }).collect()
}

fn writes(effects: &[Effect]) -> Vec<String> {
    effects.iter().filter_map(|effect| if let Effect::Write(text) = effect { Some(text.clone()) } else { None }).collect()
}

#[test]
fn ignores_notifications_when_disabled() {
    let mut handler = Osc99::default();
    assert!(handler.handle(";Hello", &Env { enabled: false, ..env() }).is_empty());
}

#[test]
fn creates_notification_for_title_and_body_and_defers_until_done() {
    let mut handler = Osc99::default();
    assert!(handler.handle("i=1:d=0:p=title;Hello", &env()).is_empty());
    assert_eq!(shown(&handler.handle("i=1:p=body;World", &env()))[0].message, "Hello: World");
    assert!(handler.handle("i=chunk:d=0:p=title;Hello ", &env()).is_empty());
    assert_eq!(shown(&handler.handle("i=chunk:d=1:p=title;World", &env()))[0].message, "Hello World");
}

#[test]
fn decodes_base64_and_sanitizes_links() {
    let mut handler = Osc99::default();
    assert_eq!(shown(&handler.handle("e=1:p=title;SGVsbG8=", &env()))[0].message, "Hello");
    handler.handle("i=link:d=0:p=title;Click [run](command:workbench.action.reloadWindow)", &env());
    assert_eq!(shown(&handler.handle("i=link:p=body;See [docs](https://example.com)", &env()))[0].message, "Click run: See docs");
    let nested = handler.handle("i=nested:p=title;[[ ](http://z)Update available - click to install](command:workbench.action.terminal.sendSequence?%7B%22text%22%3A%22calc%5Cr%22%7D)", &env());
    assert_eq!(shown(&nested)[0].message, " Update available - click to install");
}

#[test]
fn reports_activation_on_button_click() {
    let mut handler = Osc99::default();
    handler.handle("i=btn:d=0:a=report:p=title;Hi", &env());
    let key = shown(&handler.handle("i=btn:p=buttons;Yes", &env()))[0].key;
    assert_eq!(writes(&handler.button(key, 1)), ["\x1b]99;i=btn;1\x1b\\"]);
    assert!(handler.closed(key).is_empty(), "a button already reported the activation");
}

#[test]
fn supports_buttons_before_title() {
    let mut handler = Osc99::default();
    handler.handle("i=btn:p=buttons;One\u{2028}Two", &env());
    let notification = shown(&handler.handle("i=btn:a=report;Buttons test", &env()))[0].clone();
    assert_eq!(notification.buttons, ["One", "Two"]);
    assert_eq!(writes(&handler.button(notification.key, 2)), ["\x1b]99;i=btn;2\x1b\\"]);
}

#[test]
fn reports_activation_and_close_when_closed() {
    let mut handler = Osc99::default();
    handler.handle("i=btn:p=buttons;One\u{2028}Two", &env());
    let key = shown(&handler.handle("i=btn:a=report;Buttons test", &env()))[0].key;
    assert_eq!(writes(&handler.closed(key)), ["\x1b]99;i=btn;\x1b\\"]);
    let key = shown(&handler.handle("i=close:c=1:p=title;Bye", &env()))[0].key;
    assert_eq!(writes(&handler.closed(key)), ["\x1b]99;i=close:p=close;\x1b\\"]);
}

#[test]
fn responds_to_query_and_alive() {
    let mut handler = Osc99::default();
    handler.handle("i=a:p=title;A", &env());
    handler.handle("i=b:p=title;B", &env());
    assert_eq!(writes(&handler.handle("i=q:p=?;", &env())), ["\x1b]99;i=q:p=?:a=report,focus:c=1:o=always,unfocused,invisible:p=title,body,buttons,close,alive,?:u=0,1,2:w=1;\x1b\\"]);
    assert_eq!(writes(&handler.handle("i=q:p=alive;", &env())), ["\x1b]99;i=q:p=alive;a,b\x1b\\"]);
}

#[test]
fn honors_occasion_for_visibility_and_focus() {
    let mut handler = Osc99::default();
    assert!(handler.handle("o=unfocused:p=title;Hidden", &Env { window_focused: true, ..env() }).is_empty());
    assert!(handler.handle("o=invisible:p=title;Hidden", &env()).is_empty());
    assert_eq!(shown(&handler.handle("o=invisible:p=title;Shown", &Env { terminal_visible: false, ..env() })).len(), 1);
}

#[test]
fn closes_via_close_payload_and_updates_by_id() {
    let mut handler = Osc99::default();
    let key = shown(&handler.handle("i=closeme:p=title;Close", &env()))[0].key;
    assert_eq!(handler.handle("i=closeme:p=title;Again", &env()), [Effect::Update(Shown { key, id: Some("closeme".into()), message: "Again".into(), warning: false, priority: None, buttons: Vec::new(), auto_close_ms: None })]);
    assert_eq!(handler.handle("i=closeme:p=close;", &env()), [Effect::Close(key)]);
}

#[test]
fn maps_urgency_to_severity_and_priority() {
    let mut handler = Osc99::default();
    let notification = shown(&handler.handle("u=2:p=title;Urgent", &env()))[0].clone();
    assert!(notification.warning);
    assert_eq!(notification.priority, Some(Priority::Urgent));
}
