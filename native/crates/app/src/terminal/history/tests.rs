use super::*;

const EXPECTED_COMMANDS: [&str; 4] = [
    "single line command",
    "git commit -m \"A wrapped line in pwsh history\n\nSome commit description\n\nFixes #xyz\"",
    "git status",
    "two \"\nline\"",
];

fn history() -> TerminalPersistedHistory<i32> {
    TerminalPersistedHistory::load(None, 0, 5)
}

fn keys(history: &TerminalPersistedHistory<i32>) -> Vec<(&str, i32)> {
    history.entries().iter().map(|(key, value)| (key.as_str(), *value)).collect()
}

struct TempHome(PathBuf);

impl TempHome {
    fn new(name: &str) -> Self {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("terminal-history-{name}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        TempHome(path)
    }

    fn write(&self, file: &str, content: &str) -> PathBuf {
        let path = self.0.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
        path
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

fn no_env(_: &str) -> Option<String> {
    None
}

#[test]
fn should_support_adding_items_to_the_cache_and_respect_lru() {
    let mut history = history();
    history.add("foo", 1);
    assert_eq!(keys(&history), [("foo", 1)]);
    history.add("bar", 2);
    assert_eq!(keys(&history), [("foo", 1), ("bar", 2)]);
    history.add("foo", 1);
    assert_eq!(keys(&history), [("bar", 2), ("foo", 1)]);
}

#[test]
fn should_support_removing_specific_items() {
    let mut history = history();
    for (key, value) in [("1", 1), ("2", 2), ("3", 3), ("4", 4), ("5", 5)] {
        history.add(key, value);
    }
    assert_eq!(history.entries().len(), 5);
    history.add("6", 6);
    assert_eq!(history.entries().len(), 5);
}

#[test]
fn should_limit_the_number_of_entries_based_on_config() {
    let mut history = history();
    for (key, value) in [("1", 1), ("2", 2), ("3", 3), ("4", 4), ("5", 5)] {
        history.add(key, value);
    }
    assert_eq!(history.entries().len(), 5);
    history.add("6", 6);
    assert_eq!(history.entries().len(), 5);
    history.set_limit(2);
    assert_eq!(history.entries().len(), 2);
    history.add("7", 7);
    assert_eq!(history.entries().len(), 2);
    history.set_limit(3);
    assert_eq!(history.entries().len(), 2);
    history.add("8", 8);
    assert_eq!(history.entries().len(), 3);
    history.add("9", 9);
    assert_eq!(history.entries().len(), 3);
}

#[test]
fn should_reload_from_storage_service_after_recreation() {
    let mut history = history();
    history.add("1", 1);
    history.add("2", 2);
    history.add("3", 3);
    assert_eq!(history.entries().len(), 3);
    let json = history.to_json();
    assert_eq!(json, r#"{"entries":[{"key":"1","value":1},{"key":"2","value":2},{"key":"3","value":3}]}"#);
    let history2 = TerminalPersistedHistory::<i32>::load(Some(&json), history.timestamp(), 5);
    assert_eq!(keys(&history2), [("1", 1), ("2", 2), ("3", 3)]);
    assert!(!history2.is_stale(history.timestamp()));
    assert!(history2.is_stale(history.timestamp() + 1));
}

#[test]
fn fetch_bash_history_current_os() {
    let home = TempHome::new("bash");
    let content = ["single line command", "git commit -m \"A wrapped line in pwsh history", "", "Some commit description", "", "Fixes #xyz\"", "git status", "two \"", "line\""].join("\n");
    let path = home.write(".bash_history", &content);
    let history = fetch_bash_history(&home.0).unwrap().unwrap();
    assert_eq!(history.commands, EXPECTED_COMMANDS);
    assert_eq!(history.source_resource, path);
    assert_eq!(history.source_label, "~/.bash_history");
}

#[test]
fn fetch_bash_history_missing_file_is_none() {
    let home = TempHome::new("bash-missing");
    assert_eq!(fetch_bash_history(&home.0).unwrap(), None);
}

// Not ported: the bash, zsh, pwsh and fish 'remote' suites (Windows, macOS and Linux remotes), remote is not ported.

#[test]
fn fetch_zsh_history_simple_current_os() {
    let home = TempHome::new("zsh-simple");
    let content = ["single line command", "git commit -m \"A wrapped line in pwsh history\\", "\\", "Some commit description\\", "\\", "Fixes #xyz\"", "git status", "two \"\\", "line\""].join("\n");
    home.write(".zsh_history", &content);
    assert_eq!(fetch_zsh_history(&home.0).unwrap().unwrap().commands, EXPECTED_COMMANDS);
}

#[test]
fn fetch_zsh_history_extended_current_os() {
    let home = TempHome::new("zsh-extended");
    let content = [
        ": 1655252330:0;single line command",
        ": 1655252330:0;git commit -m \"A wrapped line in pwsh history\\",
        "\\",
        "Some commit description\\",
        "\\",
        "Fixes #xyz\"",
        ": 1655252330:0;git status",
        ": 1655252330:0;two \"\\",
        "line\"",
    ]
    .join("\n");
    home.write(".zsh_history", &content);
    assert_eq!(fetch_zsh_history(&home.0).unwrap().unwrap().commands, EXPECTED_COMMANDS);
}

#[test]
fn fetch_pwsh_history_current_os() {
    let home = TempHome::new("pwsh");
    let content = ["single line command", "git commit -m \"A wrapped line in pwsh history`", "`", "Some commit description`", "`", "Fixes #xyz\"", "git status", "two \"`", "line\""].join("\n");
    home.write(".local/share/powershell/PSReadline/ConsoleHost_history.txt", &content);
    assert_eq!(fetch_pwsh_history(&home.0).unwrap().unwrap().commands, EXPECTED_COMMANDS);
}

const FISH_CONTENT: [&str; 8] = [
    "- cmd: single line command",
    "  when: 1650000000",
    "- cmd: git commit -m \"A wrapped line in pwsh history\\n\\nSome commit description\\n\\nFixes #xyz\"",
    "  when: 1650000010",
    "- cmd: git status",
    "  when: 1650000020",
    "- cmd: two \"\\nline\"",
    "  when: 1650000030",
];

#[test]
fn fetch_fish_history_current_os() {
    let home = TempHome::new("fish");
    home.write(".local/share/fish/fish_history", &FISH_CONTENT.join("\n"));
    let history = fetch_fish_history(&home.0, &no_env).unwrap().unwrap();
    assert_eq!(history.commands, EXPECTED_COMMANDS);
    assert_eq!(history.source_label, "~/.local/share/fish/fish_history");
}

#[test]
fn fetch_fish_history_overriden_path_current_os() {
    let home = TempHome::new("fish-xdg");
    let path = home.write("data-home/fish/fish_history", &FISH_CONTENT.join("\n"));
    let data_home = home.0.join("data-home").to_string_lossy().into_owned();
    let env = move |name: &str| (name == "XDG_DATA_HOME").then(|| data_home.clone());
    let history = fetch_fish_history(Path::new("/nonexistent-home"), &env).unwrap().unwrap();
    assert_eq!(history.commands, EXPECTED_COMMANDS);
    assert_eq!(history.source_resource, path);
    assert_eq!(history.source_label, "$XDG_DATA_HOME/fish/fish_history");
}

#[test]
fn sanitize_fish_history_cmd_valid_new_lines() {
    let cases = [
        "\\n",
        "\\n at start",
        "some \\n in the middle",
        "at the end \\n",
        "\\\\\\n",
        "\\\\\\n valid at start",
        "valid \\\\\\n in the middle",
        "valid in the end \\\\\\n",
        "\\\\\\\\\\n",
        "\\\\\\\\\\n valid at start",
        "valid \\\\\\\\\\n in the middle",
        "valid in the end \\\\\\\\\\n",
        "mixed valid \\r\\n",
        "mixed valid \\\\\\r\\n",
        "mixed valid \\r\\\\\\n",
    ];
    for case in cases {
        assert!(sanitize_fish_history_cmd(case).contains('\n'), "{case}");
    }
}

#[test]
fn sanitize_fish_history_cmd_invalid_new_lines() {
    let cases = [
        "\\\\n",
        "\\\\n invalid at start",
        "invalid \\\\n in the middle",
        "invalid in the end \\\\n",
        "\\\\\\\\n",
        "\\\\\\\\n invalid at start",
        "invalid \\\\\\\\n in the middle",
        "invalid in the end \\\\\\\\n",
        "mixed invalid \\r\\\\n",
        "mixed invalid \\r\\\\\\\\n",
        "echo \"\\\\n\"",
    ];
    for case in cases {
        assert!(!sanitize_fish_history_cmd(case).contains('\n'), "{case}");
    }
}
