use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::term::Config;

use super::super::{ShellLaunchConfig, TerminalInstance, colors};

fn terminal(rows: u16) -> (TerminalInstance, colors::TerminalColors) {
    let app_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let themes = exthost::scan::scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let colors = colors::TerminalColors::new(&crate::theme::load_theme(&[themes], "Dark Modern").unwrap());
    let launch = ShellLaunchConfig { extension_pty: true, cwd: std::env::temp_dir(), ..Default::default() };
    let (instance, _) = TerminalInstance::new(1, launch, 40, rows, Config { scrolling_history: 1000, ..Config::default() });
    (instance, colors)
}

fn output(lines: usize) -> String {
    (0..lines).map(|line| format!("line {line}\r\n")).collect()
}

fn ignored() -> Vec<String> {
    vec!["clear".into()]
}

#[test]
fn a_running_command_sticks_while_its_output_scrolls() {
    let (mut instance, colors) = terminal(10);
    instance.process(format!("\x1b]633;A\x07$ \x1b]633;B\x07ls\r\n\x1b]633;C\x07{}", output(30)).as_bytes(), &colors);
    assert_eq!(instance.sticky_scroll(5, &ignored()), Some(super::StickyScroll { start: 0, count: 1, truncated: false, shift: 0, reveal: (0, 0) }));
    assert_eq!(instance.row_text(0), "$ ls");

    instance.term.scroll_display(Scroll::Top);
    assert_eq!(instance.sticky_scroll(5, &ignored()), None, "the viewport is at the prompt");
}

#[test]
fn ignored_commands_and_the_alternate_buffer_hide_it() {
    let (mut instance, colors) = terminal(10);
    instance.process(format!("\x1b]633;A\x07$ \x1b]633;B\x07Clear \r\n\x1b]633;E;Clear \x07\x1b]633;C\x07{}", output(30)).as_bytes(), &colors);
    assert_eq!(instance.sticky_scroll(5, &ignored()), None);

    let (mut instance, colors) = terminal(10);
    instance.process(format!("\x1b]633;A\x07$ \x1b]633;B\x07top\r\n\x1b]633;C\x07{}\x1b[?1049h", output(30)).as_bytes(), &colors);
    assert_eq!(instance.sticky_scroll(5, &ignored()), None);
}

#[test]
fn a_finished_command_sticks_until_its_end_scrolls_past_and_long_prompts_are_capped() {
    let (mut instance, colors) = terminal(10);
    let prompt = "\x1b]633;A\x07\r\nuser@host\r\nmore\r\n$ \x1b]633;B\x07";
    instance.process(format!("{prompt}ls\r\n\x1b]633;C\x07{}\x1b]633;D;0\x07{prompt}x\r\n\x1b]633;C\x07{}", output(30), output(10)).as_bytes(), &colors);
    // A on line 0 (blank, trimmed), prompt rows 1 to 3, command on 3, output 4 to 33, D and the next A on 34;
    // 49 lines in all, 39 of them scrollback.
    instance.term.scroll_display(Scroll::Top);
    instance.term.scroll_display(Scroll::Delta(-10));
    let sticky = instance.sticky_scroll(5, &ignored()).unwrap();
    assert_eq!((sticky.start, sticky.count, sticky.truncated, sticky.reveal), (1, 3, false, (1, 3)));

    let capped = instance.sticky_scroll(2, &ignored()).unwrap();
    assert_eq!((capped.count, capped.truncated), (2, true));

    // Viewport top at 33: the end on 34 pushes the overlay's last two rows out of the top.
    instance.term.scroll_display(Scroll::Bottom);
    instance.term.scroll_display(Scroll::Delta(6));
    let sticky = instance.sticky_scroll(5, &ignored()).unwrap();
    assert_eq!((sticky.start, sticky.count, sticky.shift), (1, 3, 2));
}

#[test]
fn reveal_scrolls_a_quarter_of_the_rows_above_the_command() {
    let (mut instance, colors) = terminal(8);
    instance.process(output(40).as_bytes(), &colors);
    instance.reveal_rows(20, 20);
    let viewport_y = instance.term.history_size() as i64 - instance.term.grid().display_offset() as i64;
    assert_eq!(viewport_y, 18);
}
