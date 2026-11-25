use std::collections::HashMap;
use std::time::{Duration, Instant};

use alacritty_terminal::index::{Column, Line, Point};

use super::pty::PtyEvent;
use super::*;

fn dark_colors() -> colors::TerminalColors {
    let app_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let themes = exthost::scan::scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    colors::TerminalColors::new(&crate::theme::load_theme(&[themes], "Dark Modern").unwrap())
}

fn screen_line(instance: &TerminalInstance, line: i32) -> String {
    let columns = instance.term.columns();
    (0..columns).map(|column| instance.term.grid()[Point::new(Line(line), Column(column))].c).collect::<String>().trim_end().to_owned()
}

#[test]
fn a_shell_runs_in_a_pty_and_its_output_reaches_the_screen() {
    let launch = ShellLaunchConfig {
        executable: "/bin/sh".into(),
        args: vec!["-c".into(), "printf 'cols=%s\\n' \"$(tput cols)\"; printf '\\033[31mred\\033[0m'; exit 3".into()],
        cwd: std::env::temp_dir(),
        env: HashMap::from([("TERM".into(), "xterm-256color".into()), ("PATH".into(), "/usr/bin:/bin".into())]),
        ..Default::default()
    };
    let colors = dark_colors();
    let (mut instance, receiver) = TerminalInstance::new(1, launch, 40, 5, Config { scrolling_history: 1000, ..Config::default() });
    let mut receiver = receiver.expect("the shell spawned");
    let deadline = Instant::now() + Duration::from_secs(10);
    while instance.exit_code.is_none() && Instant::now() < deadline {
        match receiver.try_recv() {
            Ok(PtyEvent::Data(bytes)) => _ = instance.process(&bytes, &colors),
            Ok(PtyEvent::Exit(code)) => instance.exited(code),
            Err(_) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
    assert_eq!(instance.exit_code, Some(Some(3)));
    assert_eq!(screen_line(&instance, 0), "cols=40");
    assert_eq!(screen_line(&instance, 1), "red");
    let red = &instance.term.grid()[Point::new(Line(1), Column(0))];
    assert_eq!(red.fg, alacritty_terminal::vte::ansi::Color::Named(alacritty_terminal::vte::ansi::NamedColor::Red));
}

#[test]
fn the_environment_drops_vscode_variables_and_adds_terminal_keys() {
    let base = [("PATH", "/bin"), ("VSCODE_PID", "1"), ("VSCODE_SHELL_LOGIN", "1"), ("ELECTRON_RUN_AS_NODE", "1"), ("VSCODE_NODE_OPTIONS", "--x"), ("HOME", "/h")];
    let base = base.into_iter().map(|(key, value)| (key.to_owned(), value.to_owned()));
    let overrides = [HashMap::from([("HOME".to_owned(), None), ("FOO".to_owned(), Some("bar".to_owned()))])];
    let env = create_environment(base, &overrides, "1.147.0", "en", "auto");
    let mut keys: Vec<_> = env.iter().map(|(key, value)| format!("{key}={value}")).collect();
    keys.sort();
    assert_eq!(
        keys,
        ["COLORTERM=truecolor", "FOO=bar", "LANG=en_US.UTF-8", "NODE_OPTIONS=--x", "PATH=/bin", "TERM=xterm-256color", "TERM_PROGRAM=vscode", "TERM_PROGRAM_VERSION=1.147.0", "VSCODE_SHELL_LOGIN=1"]
    );
    assert_eq!(lang_variable("pt-br"), "pt_BR.UTF-8");
    assert_eq!(lang_variable("xx"), "xx.UTF-8");
    let kept = create_environment([("LANG".to_owned(), "pl_PL.UTF-8".to_owned())].into_iter(), &[], "1", "en", "auto");
    assert_eq!(kept["LANG"], "pl_PL.UTF-8");
}

#[test]
fn paste_uses_carriage_returns_and_bracketed_paste_markers() {
    assert_eq!(paste_data("a\r\nb\nc", false), "a\rb\rc");
    assert_eq!(paste_data("a\n", true), "\x1b[200~a\r\x1b[201~");
    assert_eq!(paste_data("a\x1b[201~rm -rf ~\n", true), "\x1b[200~a\u{241b}[201~rm -rf ~\r\x1b[201~");
    assert_eq!(paste_data("\x1b", false), "\x1b");
    let launch = ShellLaunchConfig { executable: "/bin/cat".into(), cwd: std::env::temp_dir(), ..Default::default() };
    let colors = dark_colors();
    let (mut instance, _receiver) = TerminalInstance::new(1, launch, 20, 3, Config { scrolling_history: 100, ..Config::default() });
    instance.process(b"\x1b[?2004h", &colors);
    assert!(instance.term.mode().contains(TermMode::BRACKETED_PASTE));
    instance.process(b"abc\r\ndef", &colors);
    instance.select_all();
    assert_eq!(instance.selection_text().as_deref(), Some("abc\ndef\n"));
}

/// Record Terminal Session: output, typed input and sent text are separate events, a resize records the new size.
#[test]
fn a_recording_keeps_each_kind_of_event() {
    let launch = ShellLaunchConfig { extension_pty: true, cwd: std::env::temp_dir(), ..Default::default() };
    let (mut instance, _receiver) = TerminalInstance::new(1, launch, 20, 3, Config::default());
    instance.process_input = Some(Box::new(|_: &str| {}));
    instance.input("not recorded");
    instance.recording = Some(Recording { events: Vec::new(), last_event: Instant::now() });
    let colors = dark_colors();
    instance.process(b"out", &colors);
    instance.input("a");
    instance.send_text("ls", true);
    instance.resize(30, 4);
    let events = instance.recording.unwrap().events;
    assert_eq!(serde_json::Value::Array(events), json!([
        { "type": "output", "data": "out" },
        { "type": "input", "data": "a" },
        { "type": "sendText", "data": "ls\r" },
        { "type": "resize", "cols": 30, "rows": 4 },
    ]));
}

#[test]
fn osc_52_stores_and_reads_the_clipboard() {
    let launch = ShellLaunchConfig { extension_pty: true, cwd: std::env::temp_dir(), ..Default::default() };
    let (mut instance, _receiver) = TerminalInstance::new(1, launch, 20, 3, Config { osc52: alacritty_terminal::term::Osc52::CopyPaste, ..Config::default() });
    let written = Rc::new(RefCell::new(String::new()));
    let sink = written.clone();
    instance.process_input = Some(Box::new(move |data: &str| sink.borrow_mut().push_str(data)));
    let colors = dark_colors();
    instance.process(b"\x1b]52;c;aGk=\x07\x1b]52;c;?\x07", &colors);
    assert_eq!(instance.clipboard_requests, ["hi"]);
    instance.answer_clipboard_loads("hey");
    assert_eq!(*written.borrow(), "\x1b]52;c;aGV5\x07");
    assert!(instance.clipboard_loads.is_empty());
}

#[test]
fn clear_keeps_the_cursor_line_as_the_first_line() {
    let launch = ShellLaunchConfig { executable: "/bin/cat".into(), cwd: std::env::temp_dir(), ..Default::default() };
    let colors = dark_colors();
    let (mut instance, _receiver) = TerminalInstance::new(1, launch, 20, 3, Config { scrolling_history: 100, ..Config::default() });
    instance.process(b"1\r\n2\r\n3\r\n4\r\n$ ab", &colors);
    assert!(instance.term.history_size() > 0);
    instance.clear();
    assert_eq!(instance.term.history_size(), 0);
    assert_eq!(screen_line(&instance, 0), "$ ab");
    assert_eq!(screen_line(&instance, 1), "");
    assert_eq!(instance.term.grid().cursor.point, Point::new(Line(0), Column(4)));
}

#[test]
fn exit_messages_follow_parse_exit_result() {
    let launch = ShellLaunchConfig { executable: "/bin/zsh".into(), args: vec!["-l".into(), "-i".into()], cwd: std::env::temp_dir(), ..Default::default() };
    let (mut instance, _receiver) = TerminalInstance::new(1, launch, 20, 3, Config { scrolling_history: 100, ..Config::default() });
    assert_eq!(instance.exit_message(0), None);
    assert_eq!(instance.exit_message(1).as_deref(), Some("The terminal process \"/bin/zsh '-l', '-i'\" failed to launch (exit code: 1)."));
    instance.kill();
    assert_eq!(instance.exit_message(2).as_deref(), Some("The terminal process \"/bin/zsh '-l', '-i'\" terminated with exit code: 2."));
}
