//! quickFix.ts, the builtin actions of terminalQuickFixBuiltinActions.ts and the matching of quickFixAddon.ts
//! `getQuickFixesForCommand` and terminalCommand.ts `getOutputMatch`: what a finished command's line and output
//! offer as fixes. Extension providers get their lines from `lines_for_command` and match in the workbench.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
use regex::Regex;

use super::{FinishedCommand, TerminalInstance};

/// `ITerminalOutputMatcher`.
#[derive(Clone, Debug)]
pub struct OutputMatcher {
    pub line_matcher: Regex,
    pub anchor_bottom: bool,
    pub offset: i64,
    pub length: usize,
}

/// `ITerminalOutputMatch`: the match's groups (0 is the whole match), its named groups and the lines read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OutputMatch {
    pub groups: Vec<Option<String>>,
    pub named: Vec<(String, String)>,
    pub lines: Vec<String>,
}

impl OutputMatch {
    pub fn from_captures(regex: &Regex, text: &str, lines: Vec<String>) -> Option<OutputMatch> {
        let captures = regex.captures(text)?;
        let groups = captures.iter().map(|group| group.map(|group| group.as_str().to_owned())).collect();
        let named = regex.capture_names().flatten().filter_map(|name| Some((name.to_owned(), captures.name(name)?.as_str().to_owned()))).collect();
        Some(OutputMatch { groups, named, lines })
    }

    fn group(&self, index: usize) -> Option<&str> {
        self.groups.get(index)?.as_deref()
    }

    fn named(&self, name: &str) -> Option<&str> {
        self.named.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str())
    }
}

/// `TerminalQuickFixType` with what each runs.
#[derive(Clone, Debug, PartialEq)]
pub enum Fix {
    /// Run: the command line, sent to the terminal (executed unless `shouldExecute` is false).
    Command { command: String, execute: bool },
    /// Open: a URI.
    Open(String),
    /// Free port: kill the process listening on the port.
    FreePort(String),
    /// A workbench command with its title.
    VscodeCommand { id: String, title: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct QuickFix {
    pub id: String,
    pub source: String,
    /// `kind: 'explain'`.
    pub explain: bool,
    pub fix: Fix,
}

impl QuickFix {
    /// The action's label: `Run: {0}`, `Open: {0}`, `Free port {0}` or the command's title.
    pub fn label(&self) -> String {
        match &self.fix {
            Fix::Command { command, .. } => format!("Run: {command}"),
            // `labelService.getUriLabel` for a file, the URI itself for a web link.
            Fix::Open(uri) => match crate::editor_history::file_path(uri) {
                Some(path) => format!("Open: {}", path.display()),
                None => format!("Open: {uri}"),
            },
            Fix::FreePort(port) => format!("Free port {port}"),
            Fix::VscodeCommand { title, .. } => title.clone(),
        }
    }

    /// `getQuickFixIcon`.
    pub fn icon(&self) -> &'static str {
        match &self.fix {
            _ if self.explain => "sparkle",
            Fix::Open(uri) if uri.starts_with("http://") || uri.starts_with("https://") => "link-external",
            Fix::Open(_) => "go-to-file",
            Fix::Command { .. } => "run",
            Fix::FreePort(_) => "debug-disconnect",
            Fix::VscodeCommand { .. } => "lightbulb",
        }
    }
}

/// The fixes in order, a command line or URI once (`commandQuickFixSet`, `openQuickFixSet`).
pub fn dedup(fixes: Vec<QuickFix>) -> Vec<QuickFix> {
    let mut kept: Vec<QuickFix> = Vec::new();
    for fix in fixes {
        let seen = kept.iter().any(|other| match (&other.fix, &fix.fix) {
            (Fix::Command { command: a, .. }, Fix::Command { command: b, .. }) => a == b,
            (Fix::Open(a), Fix::Open(b)) => a == b,
            _ => false,
        });
        if !seen {
            kept.push(fix);
        }
    }
    kept
}

/// `commandExitResult`: 'success' wants exit code 0, 'error' any other.
pub fn exit_result_matches(error: bool, exit_code: Option<i32>) -> bool {
    error != (exit_code == Some(0))
}

struct Builtin {
    id: &'static str,
    command_line: &'static str,
    line_matcher: &'static str,
    offset: i64,
    length: usize,
    error: bool,
    fixes: fn(&str, &OutputMatch) -> Vec<Fix>,
}

/// terminal.quickFix.contribution.ts registers these; the two pwsh feedback provider fixes are left out: they read
/// the line after the one matched from a match of one line, which never has it.
const BUILTINS: [Builtin; 6] = [
    Builtin { id: "Git Two Dashes", command_line: "git", line_matcher: r"error: did you mean `--(.+)` \(with two dashes\)\?", offset: 0, length: 2, error: true, fixes: git_two_dashes },
    Builtin { id: "Git Fast Forward Pull", command_line: "git", line_matcher: "and can be fast-forwarded", offset: 0, length: 8, error: false, fixes: |_, _| vec![Fix::Command { command: "git pull".into(), execute: true }] },
    Builtin { id: "Free Port", command_line: ".+", line_matcher: r"(?:address already in use (?:0\.0\.0\.0|127\.0\.0\.1|localhost|::):|Unable to bind [^ ]*:|can't listen on port |listen EADDRINUSE [^ ]*:)(?P<portNumber>\d{4,5})", offset: 0, length: 30, error: true, fixes: free_port },
    Builtin { id: "Git Similar", command_line: "git", line_matcher: "(?:(most similar commands? (is|are)))", offset: 0, length: 10, error: true, fixes: git_similar },
    Builtin { id: "Git Push Set Upstream", command_line: r"git\s+push", line_matcher: r"git push --set-upstream origin (?P<branchName>[^\s]+)", offset: 0, length: 8, error: true, fixes: git_push_set_upstream },
    Builtin { id: "Git Create Pr", command_line: r"git\s+push", line_matcher: r"remote:\s*(?P<link>https://github\.com/.+/.+/pull/new/.+)", offset: 4, length: 12, error: false, fixes: git_create_pr },
];

fn git_two_dashes(command_line: &str, output: &OutputMatch) -> Vec<Fix> {
    let Some(argument) = output.group(1) else { return Vec::new() };
    vec![Fix::Command { command: command_line.replacen(&format!(" -{argument}"), &format!(" --{argument}"), 1), execute: true }]
}

fn free_port(_: &str, output: &OutputMatch) -> Vec<Fix> {
    output.named("portNumber").map(|port| Fix::FreePort(port.to_owned())).into_iter().collect()
}

fn git_similar(command_line: &str, output: &OutputMatch) -> Vec<Fix> {
    let Some(matched) = output.group(0) else { return Vec::new() };
    let start = output.lines.iter().position(|line| line.contains(matched)).map_or(0, |index| index + 1);
    let subcommand = Regex::new(r"git\s+[^\s]+").expect("the pattern compiles");
    output.lines[start..]
        .iter()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .map(|fixed| Fix::Command { command: subcommand.replacen(command_line, 1, regex::NoExpand(&format!("git {fixed}"))).into_owned(), execute: true })
        .collect()
}

fn git_push_set_upstream(_: &str, output: &OutputMatch) -> Vec<Fix> {
    let Some(branch) = output.named("branchName") else { return Vec::new() };
    vec![Fix::Command { command: format!("git push --set-upstream origin {branch}"), execute: true }]
}

fn git_create_pr(_: &str, output: &OutputMatch) -> Vec<Fix> {
    output.named("link").map(|link| Fix::Open(link.trim_end().to_owned())).into_iter().collect()
}

impl TerminalInstance {
    /// The builtin fixes for a finished command, in registration order.
    pub fn builtin_quick_fixes(&self, command: &FinishedCommand) -> Vec<QuickFix> {
        let mut fixes = Vec::new();
        for builtin in &BUILTINS {
            if !exit_result_matches(builtin.error, command.exit_code) {
                continue;
            }
            let command_line = Regex::new(builtin.command_line).expect("builtin patterns compile");
            if !command_line.is_match(&command.command) {
                continue;
            }
            let matcher = OutputMatcher { line_matcher: Regex::new(builtin.line_matcher).expect("builtin patterns compile"), anchor_bottom: true, offset: builtin.offset, length: builtin.length };
            let Some(output) = self.output_match(command, &matcher) else { continue };
            for fix in (builtin.fixes)(&command.command, &output) {
                fixes.push(QuickFix { id: builtin.id.into(), source: "builtin".into(), explain: false, fix });
            }
        }
        fixes
    }

    /// terminalCommand.ts `getOutputMatch`: the output's lines from the anchor, each tested alone, until `length`
    /// lines are read; the first that matches gives the match.
    pub fn output_match(&self, command: &FinishedCommand, matcher: &OutputMatcher) -> Option<OutputMatch> {
        let (start, end) = (command.executed_line?.max(0), command.end_line?);
        let mut lines: Vec<String> = Vec::new();
        let mut found: Option<(String, usize)> = None;
        let note = |lines: &mut Vec<String>, index: usize, found: &mut Option<(String, usize)>| {
            if found.is_none() && matcher.line_matcher.is_match(&lines[index]) {
                *found = Some((lines[index].clone(), index));
            }
        };
        if matcher.anchor_bottom {
            let mut line = end - matcher.offset;
            while line >= start {
                let mut wrapped_start = line;
                while wrapped_start >= start && self.row_is_continuation(wrapped_start) {
                    wrapped_start -= 1;
                }
                lines.insert(0, self.joined_rows(wrapped_start, line));
                note(&mut lines, 0, &mut found);
                if lines.len() >= matcher.length {
                    break;
                }
                line = wrapped_start - 1;
            }
        } else {
            let mut line = start + matcher.offset;
            while line < end {
                let mut wrapped_end = line;
                while wrapped_end + 1 < end && self.row_is_continuation(wrapped_end + 1) {
                    wrapped_end += 1;
                }
                lines.push(self.joined_rows(line, wrapped_end));
                let last = lines.len() - 1;
                note(&mut lines, last, &mut found);
                if lines.len() >= matcher.length {
                    break;
                }
                line = wrapped_end + 1;
            }
        }
        let (text, _) = found?;
        OutputMatch::from_captures(&matcher.line_matcher, &text, lines)
    }

    /// commandDetectionCapability.ts `getLinesForCommand`, for an extension's matcher.
    pub fn lines_for_command(&self, command: &FinishedCommand, matcher: &OutputMatcher) -> Option<Vec<String>> {
        let (start, end) = (command.executed_line?, command.end_line?);
        let mut lines: Vec<String> = Vec::new();
        if matcher.anchor_bottom {
            let mut line = end - matcher.offset;
            while line >= start {
                let mut wrapped_start = line;
                while wrapped_start >= start && self.row_is_continuation(wrapped_start) {
                    wrapped_start -= 1;
                }
                lines.insert(0, self.joined_rows(wrapped_start, line));
                if lines.len() > matcher.length {
                    lines.pop();
                }
                line = wrapped_start - 1;
            }
        } else {
            let mut line = start + matcher.offset;
            while line < end {
                let mut wrapped_end = line;
                while wrapped_end + 1 < end && self.row_is_continuation(wrapped_end + 1) {
                    wrapped_end += 1;
                }
                lines.push(self.joined_rows(line, wrapped_end));
                // Upstream drops the first line once `length` lines are read, so at most `length - 1` stay.
                if lines.len() == matcher.length {
                    lines.remove(0);
                }
                line = wrapped_end + 1;
            }
        }
        Some(lines)
    }

    /// xterm's `isWrapped`: the buffer line continues the one before it.
    fn row_is_continuation(&self, line: i64) -> bool {
        let history = self.term.history_size() as i64;
        let previous = line - 1 - history;
        if previous < -history || previous >= self.term.screen_lines() as i64 {
            return false;
        }
        self.term.grid()[Line(previous as i32)][Column(self.term.columns() - 1)].flags.contains(Flags::WRAPLINE)
    }

    /// `getXtermLineContent`: the rows' text, each trimmed at its end.
    fn joined_rows(&self, start: i64, end: i64) -> String {
        (start..=end).map(|line| self.row_text(line)).collect()
    }
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::term::Config;

    use super::*;
    use crate::terminal::{ShellLaunchConfig, colors};

    fn run(output: &str, exit_code: i32, command: &str) -> TerminalInstance {
        let app_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let themes = exthost::scan::scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let colors = colors::TerminalColors::new(&crate::theme::load_theme(&[themes], "Dark Modern").unwrap());
        let launch = ShellLaunchConfig { extension_pty: true, cwd: std::env::temp_dir(), ..Default::default() };
        let (mut instance, _) = TerminalInstance::new(1, launch, 80, 10, Config { scrolling_history: 100, ..Config::default() });
        let output = output.replace('\n', "\r\n");
        instance.process(format!("\x1b]633;A\x07$ \x1b]633;B\x07{command}\r\n\x1b]633;E;{command}\x07\x1b]633;C\x07{output}\x1b]633;D;{exit_code}\x07").as_bytes(), &colors);
        instance
    }

    fn fixes(instance: &TerminalInstance) -> Vec<(String, String)> {
        instance.builtin_quick_fixes(instance.commands.last().unwrap()).into_iter().map(|fix| (fix.id.clone(), fix.label())).collect()
    }

    #[test]
    fn git_push_without_upstream_offers_set_upstream() {
        let output = "fatal: The current branch feat has no upstream branch.\nTo push the current branch and set the remote as upstream, use\n\n    git push --set-upstream origin feat\n\n";
        let instance = run(output, 128, "git push");
        assert_eq!(fixes(&instance), [("Git Push Set Upstream".to_owned(), "Run: git push --set-upstream origin feat".to_owned())]);
    }

    #[test]
    fn git_similar_offers_each_suggestion() {
        let output = "git: 'sttaus' is not a git command. See 'git --help'.\n\nThe most similar commands are\n\tstatus\n\tstash\n";
        let instance = run(output, 1, "git sttaus -s");
        assert_eq!(fixes(&instance), [("Git Similar".to_owned(), "Run: git status -s".to_owned()), ("Git Similar".to_owned(), "Run: git stash -s".to_owned())]);
    }

    #[test]
    fn a_port_in_use_offers_to_free_it_and_success_skips_error_fixes() {
        let instance = run("Error: listen EADDRINUSE: address already in use :::3000\n", 1, "node server.js");
        assert_eq!(fixes(&instance), [("Free Port".to_owned(), "Free port 3000".to_owned())]);
        let instance = run("Error: listen EADDRINUSE: address already in use :::3000\n", 0, "node server.js");
        assert!(fixes(&instance).is_empty());
    }

    #[test]
    fn lines_for_an_extension_matcher_keep_the_first_ones_read_upward() {
        // `getLinesForCommand` pops the bottom line once over `length`, so the top lines of the range stay.
        let instance = run("one\ntwo\nthree\n", 1, "npm run x");
        let matcher = OutputMatcher { line_matcher: Regex::new("x").unwrap(), anchor_bottom: true, offset: 1, length: 2 };
        assert_eq!(instance.lines_for_command(instance.commands.last().unwrap(), &matcher).unwrap(), ["one", "two"]);
    }
}
