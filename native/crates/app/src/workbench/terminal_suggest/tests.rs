use super::*;

#[test]
fn inline_completion_is_supported_for_bash_zsh_fish_pwsh_and_git_bash() {
    for shell in ["bash", "zsh", "fish", "pwsh", "gitbash"] {
        assert!(is_inline_completion_supported(Some(shell)), "{shell}");
    }
}

#[test]
fn inline_completion_is_not_supported_for_other_shells() {
    for shell in ["nu", "julia", "node", "python", "xonsh", "sh", "csh", "ksh", "cmd", "wsl"] {
        assert!(!is_inline_completion_supported(Some(shell)), "{shell}");
    }
    assert!(!is_inline_completion_supported(None));
}

#[test]
fn ctrl_space_toggles_details_while_a_suggestion_is_focused() {
    use crate::keybindings::{Resolution, Resolver, default_keybindings, parse_keybinding};
    let resolver = Resolver::new(default_keybindings(&[]), Vec::new());
    let mut context: std::collections::HashMap<String, Value> = ["terminalFocus", "terminalHasBeenCreated", "terminalIsOpen", "terminalProcessSupported"].iter().map(|key| (key.to_string(), json!(true))).collect();
    context.insert("config.terminal.integrated.suggest.enabled".into(), json!(true));
    let ctrl_space = parse_keybinding("ctrl+space")[0];
    let command = |context: &std::collections::HashMap<String, Value>| match resolver.resolve(context, &[], ctrl_space) {
        Resolution::KbFound { command, .. } => command,
        other => panic!("{other:?}"),
    };
    assert_eq!(command(&context), "workbench.action.terminal.triggerSuggest");
    context.insert("terminalSuggestWidgetVisible".into(), json!(true));
    context.insert("simpleSuggestWidgetHasFocusedSuggestion".into(), json!(true));
    assert_eq!(command(&context), "workbench.action.terminal.suggestToggleDetails");
}

fn prompt(value: &str, cursor: usize) -> PromptState {
    PromptState { value: value.into(), prefix: value.chars().take(cursor).collect(), cursor, ghost: None }
}

fn completion(label: &str, start: usize, end: usize) -> TerminalCompletion {
    TerminalCompletion { replacement_range: Some((start, end)), ..TerminalCompletion::new(label, "ext:test") }
}

#[test]
fn accepting_types_the_rest_of_the_word() {
    let sequence = accept_sequence(&completion("checkout", 4, 6), &prompt("git ch", 6), None, false);
    assert_eq!(sequence, "eckout");
}

#[test]
fn accepting_replaces_what_does_not_match_and_deletes_the_rest_of_the_word() {
    // `git cx|yz`: one backspace over `x`, two deletes over `yz`.
    let sequence = accept_sequence(&completion("checkout", 4, 6), &prompt("git cxyz", 6), None, false);
    assert_eq!(sequence, "\x7f\x1b[3~\x1b[3~heckout");
}

#[test]
fn accepting_escapes_spaces_in_folders() {
    let folder = TerminalCompletion { kind: Some(TerminalCompletionItemKind::Folder), ..completion("my dir/", 3, 5) };
    assert_eq!(accept_sequence(&folder, &prompt("cd my", 5), None, false), "\\ dir/");
}

#[test]
fn run_on_enter_follows_the_setting() {
    let item = completion("ls", 0, 2);
    assert_eq!(accept_sequence(&item, &prompt("ls", 2), Some("exactMatch"), false), "\r");
    assert_eq!(accept_sequence(&item, &prompt("l", 1), Some("exactMatch"), false), "s");
    assert_eq!(accept_sequence(&item, &prompt("l", 1), Some("always"), false), "s\r");
    assert_eq!(accept_sequence(&item, &prompt("l", 1), Some("never"), false), "s");
}

#[test]
fn a_trailing_space_follows_everything_but_folders() {
    assert_eq!(accept_sequence(&completion("ls", 0, 1), &prompt("l", 1), None, true), "s ");
    let folder = TerminalCompletion { kind: Some(TerminalCompletionItemKind::Folder), ..completion("src/", 0, 1) };
    assert_eq!(accept_sequence(&folder, &prompt("s", 1), None, true), "rc/");
}

#[test]
fn the_inline_suggestion_takes_the_ghost_text_and_a_matching_completion() {
    let state = PromptState { value: "git checkout".into(), prefix: "git ch".into(), cursor: 6, ghost: Some(6) };
    let mut completions = vec![TerminalCompletion { detail: Some("Switch branches".into()), ..completion("checkout", 4, 6) }];
    let item = inline_completion_item(&state, 0, &json!("alwaysOnTop"), None, &mut completions);
    assert!(!item.is_invalid);
    assert_eq!(item.completion.label, "checkout");
    assert_eq!(item.completion.replacement_range, Some((4, 6)));
    assert_eq!(item.completion.detail.as_deref(), Some("Switch branches"));
    assert_eq!(item.completion.kind, Some(TerminalCompletionItemKind::InlineSuggestionAlwaysOnTop));
    assert!(completions.is_empty());
    assert!(inline_completion_item(&prompt("git ch", 6), 0, &json!("alwaysOnTop"), None, &mut Vec::new()).is_invalid);
    assert!(inline_completion_item(&state, 0, &json!("off"), None, &mut Vec::new()).is_invalid);
}
