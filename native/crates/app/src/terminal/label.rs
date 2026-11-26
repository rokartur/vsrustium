//! terminalInstance.ts `TerminalLabelComputer` for `terminal.integrated.tabs.description`, with labels.ts
//! `template`.

use std::path::Path;

/// What `computeLabel` knows of one terminal.
pub struct LabelInput<'a> {
    pub process: &'a str,
    pub sequence: Option<&'a str>,
    /// `instance.cwd`; `None` until the cwd is detected.
    pub cwd: Option<&'a str>,
    pub user_home: &'a str,
    /// `instance.workspaceFolder`: the folder holding the launch cwd, else the last active one.
    pub workspace_folder: Option<&'a Path>,
    pub multi_root: bool,
    /// `type === 'Task'`.
    pub task: bool,
    pub shell_type: Option<&'a str>,
}

/// `computeLabel(instance, tabs.description, TerminalLabelType.Description)`. Cwd detection is always on: the port
/// reads the pty's cwd as `NaiveCwdDetection` does.
pub fn description(template: &str, separator: &str, input: &LabelInput) -> String {
    let mut cwd = input.cwd.unwrap_or_default().to_owned();
    if !input.user_home.is_empty() && cwd == input.user_home {
        cwd = "~".into();
    } else if let Some(rest) = cwd.strip_prefix(input.user_home).filter(|rest| !input.user_home.is_empty() && rest.starts_with('/')) {
        cwd = format!("~{rest}");
    }
    let mut cwd_folder = String::new();
    if let Some(detected) = input.cwd.filter(|_| !cwd.is_empty() && !input.task) {
        // Case-insensitive as the macOS disk provider has no PathCaseSensitive.
        let differs = input.workspace_folder.is_some_and(|folder| !folder.to_string_lossy().eq_ignore_ascii_case(detected));
        if input.multi_root || differs {
            cwd_folder = cwd.rsplit('/').next().unwrap_or_default().to_owned();
        }
    }
    let workspace_folder = input.workspace_folder.and_then(Path::file_name).map(|name| name.to_string_lossy().into_owned());
    let values = [
        ("cwd", Some(cwd.as_str())),
        ("cwdFolder", Some(cwd_folder.as_str())),
        ("workspaceFolder", workspace_folder.as_deref()),
        ("workspaceFolderName", workspace_folder.as_deref()),
        ("process", Some(input.process)),
        ("sequence", input.sequence),
        ("task", input.task.then_some("Task")),
        ("shellType", input.shell_type),
    ];
    template_label(template.trim(), separator, &values).replace(['\n', '\r', '\t'], "").trim().to_owned()
}

#[derive(PartialEq)]
enum Segment {
    Text,
    Variable,
    Separator,
}

/// labels.ts `template`: `${name}` takes its value, `${separator}` stays only between two non-empty parts.
pub fn template_label(template: &str, separator: &str, values: &[(&str, Option<&str>)]) -> String {
    let mut segments: Vec<(String, Segment)> = Vec::new();
    let mut in_variable = false;
    let mut current = String::new();
    for char in template.chars() {
        if char == '$' || (in_variable && char == '{') {
            if !current.is_empty() {
                segments.push((std::mem::take(&mut current), Segment::Text));
            }
            current.clear();
            in_variable = true;
        } else if char == '}' && in_variable {
            if current == "separator" {
                if segments.last().is_none_or(|(_, kind)| *kind != Segment::Separator) {
                    segments.push((separator.to_owned(), Segment::Separator));
                }
            } else if let Some(value) = values.iter().find(|(name, _)| *name == current).and_then(|(_, value)| *value).filter(|value| !value.is_empty()) {
                segments.push((value.to_owned(), Segment::Variable));
            }
            current.clear();
            in_variable = false;
        } else {
            current.push(char);
        }
    }
    if !current.is_empty() && !in_variable {
        segments.push((current, Segment::Text));
    }
    let mut label = String::new();
    for (index, (value, kind)) in segments.iter().enumerate() {
        let filled = |segment: Option<&(String, Segment)>| segment.is_some_and(|(value, kind)| *kind != Segment::Separator && !value.is_empty());
        if *kind == Segment::Separator && !(index > 0 && filled(segments.get(index - 1)) && filled(segments.get(index + 1))) {
            continue;
        }
        label.push_str(value);
    }
    label
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{LabelInput, description, template_label};

    const DEFAULT: &str = "${task}${separator}${local}${separator}${host}${separator}${cwdFolder}";

    fn input<'a>(cwd: Option<&'a str>, folder: Option<&'a Path>) -> LabelInput<'a> {
        LabelInput { process: "fish", sequence: None, cwd, user_home: "/Users/a", workspace_folder: folder, multi_root: false, task: false, shell_type: None }
    }

    #[test]
    fn the_cwd_folder_shows_only_away_from_the_workspace_folder() {
        let folder = Path::new("/Users/a/project");
        assert_eq!(description(DEFAULT, " - ", &input(Some("/Users/a/project/docs"), Some(folder))), "docs");
        assert_eq!(description(DEFAULT, " - ", &input(Some("/Users/a/Project"), Some(folder))), "");
        assert_eq!(description(DEFAULT, " - ", &input(Some("/Users/a"), Some(folder))), "~");
        assert_eq!(description(DEFAULT, " - ", &input(Some("/tmp/x"), None)), "", "no workspace folder, single root");
        assert_eq!(description(DEFAULT, " - ", &LabelInput { multi_root: true, ..input(Some("/Users/a/project"), Some(folder)) }), "project");
        assert_eq!(description(DEFAULT, " - ", &LabelInput { task: true, ..input(Some("/tmp/x"), Some(folder)) }), "Task");
    }

    #[test]
    fn separators_need_a_value_on_both_sides() {
        let values = [("a", Some("1")), ("b", Some("")), ("c", Some("3"))];
        assert_eq!(template_label("${a}${separator}${b}${separator}${c}", " - ", &values), "1 - 3");
        assert_eq!(template_label("${separator}${a}${separator}", " - ", &values), "1");
        assert_eq!(template_label("x ${c} y", " - ", &values), "x 3 y");
    }
}
