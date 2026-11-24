//! Running ripgrep (base/node/ripgrep.ts, search/node/ripgrepSearchUtils.ts) and the glob arguments both searches
//! share (search/node/ripgrepTextSearchEngine.ts).

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use files::glob;

/// `rgDiskPath`: the `@vscode/ripgrep-universal` binary for this machine.
pub fn rg_path(app_root: &Path) -> PathBuf {
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        other => other,
    };
    app_root.join(format!("node_modules/@vscode/ripgrep-universal/bin/darwin-{arch}/rg"))
}

/// The cancellation token of one search; cancelling kills the ripgrep it runs.
#[derive(Default)]
pub struct Cancellation {
    cancelled: AtomicBool,
    child: Mutex<Option<Child>>,
}

impl Cancellation {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Some(child) = self.child.lock().unwrap().as_mut() {
            let _ = child.kill();
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

pub struct Finished {
    pub stderr: String,
    /// Exit code 0: something matched (ripgrep exits with 1 when nothing did).
    pub success: bool,
    pub got_data: bool,
}

/// Runs ripgrep in `cwd` and hands each stdout line (without the newline) to `on_line` until it returns false, which
/// kills ripgrep.
pub fn run(rg: &Path, args: &[String], cwd: &Path, cancellation: &Cancellation, on_line: &mut dyn FnMut(&str) -> bool) -> std::io::Result<Finished> {
    let mut child = Command::new(rg).args(args).current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let stdout = child.stdout.take().expect("stdout is piped");
    let mut stderr = child.stderr.take().expect("stderr is piped");
    let stderr = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });
    *cancellation.child.lock().unwrap() = Some(child);
    if cancellation.is_cancelled() {
        cancellation.cancel();
    }
    let mut reader = BufReader::new(stdout);
    let mut line = Vec::new();
    let mut got_data = false;
    while !cancellation.is_cancelled() && reader.read_until(b'\n', &mut line)? > 0 {
        got_data = true;
        let text = String::from_utf8_lossy(&line);
        if !on_line(text.strip_suffix('\n').unwrap_or(&text)) {
            break;
        }
        line.clear();
    }
    let mut child = cancellation.child.lock().unwrap().take().expect("set above");
    let _ = child.kill();
    let status = child.wait()?;
    Ok(Finished { stderr: stderr.join().unwrap_or_default(), success: status.success(), got_data })
}

/// `anchorGlob`: ripgrep globs without a leading `**` or `/` match at any depth, VS Code globs only at the root.
pub fn anchor_glob(glob: &str) -> String {
    if glob.starts_with("**") || glob.starts_with('/') { glob.to_string() } else { format!("/{glob}") }
}

/// `getExcludeExceptionGlobs`: the globs of a run that lists only what `!` exceptions re-include, since ripgrep does
/// not descend into excluded folders.
pub fn exclude_exception_globs(exclude_exceptions: &[String]) -> Vec<String> {
    let exceptions: Vec<String> = exclude_exceptions.iter().flat_map(|exception| perform_brace_expansion_for_ripgrep(exception)).collect();
    let mut globs: Vec<String> = Vec::new();
    let mut add = |glob: String| {
        if !globs.contains(&glob) {
            globs.push(glob);
        }
    };
    // Unless an exception can match at any depth, only the folders that lead to the exceptions are walked.
    if !exceptions.iter().any(|exception| exception.starts_with("**")) {
        add("!*".to_string());
        for exception in &exceptions {
            let segments = glob::split_glob_aware(exception, '/');
            for end in 1..segments.len() {
                add(format!("{}/", anchor_glob(&segments[..end].join("/"))));
            }
        }
    }
    for exception in &exceptions {
        add(anchor_glob(exception));
        add(anchor_glob(&format!("{exception}/**")));
    }
    for folder in glob::get_folders_skipped_by_exclude_exceptions(exclude_exceptions) {
        add(format!("!**/{folder}"));
    }
    globs
}

/// `performBraceExpansionForRipgrep`: ripgrep rejects nested braces, so one level of `{a,b}` becomes separate globs.
pub fn perform_brace_expansion_for_ripgrep(pattern: &str) -> Vec<String> {
    let Split { fixed_start, in_braces, fixed_end } = escape_aware_split(pattern);
    let (Some(fixed_start), Some(fixed_end)) = (fixed_start, fixed_end) else { return vec![in_braces] };
    let mut alternatives = glob::split_glob_aware(&in_braces, ',');
    if alternatives.is_empty() {
        alternatives.push(String::new());
    }
    let ends = perform_brace_expansion_for_ripgrep(&fixed_end);
    let mut expanded = Vec::new();
    for alternative in &alternatives {
        for end in &ends {
            expanded.push(format!("{fixed_start}{alternative}{end}"));
        }
    }
    expanded
}

struct Split {
    fixed_start: Option<String>,
    in_braces: String,
    fixed_end: Option<String>,
}

/// `getEscapeAwareSplitStringForRipgrep`: splits at the first unescaped `{...}`; a second `{` before the `}` is a
/// nesting ripgrep rejects, so the pattern stays whole.
fn escape_aware_split(pattern: &str) -> Split {
    let mut in_braces = false;
    let mut escaped = false;
    let mut fixed_start = String::new();
    let mut braces = String::new();
    for (index, char) in pattern.char_indices() {
        let target = if in_braces { &mut braces } else { &mut fixed_start };
        match char {
            '\\' if escaped => {
                target.push_str("\\\\");
                escaped = false;
            }
            '\\' => escaped = true,
            '{' | '}' if escaped => {
                target.push(char);
                escaped = false;
            }
            '{' if in_braces => return Split { fixed_start: None, in_braces: format!("{fixed_start}{{{braces}{{{}", &pattern[index + 1..]), fixed_end: None },
            '{' => in_braces = true,
            '}' if in_braces => return Split { fixed_start: Some(fixed_start), in_braces: braces, fixed_end: Some(pattern[index + 1..].to_string()) },
            _ => {
                if escaped {
                    target.push('\\');
                }
                target.push(char);
                escaped = false;
            }
        }
    }
    let rest = if in_braces { format!("{{{braces}") } else { String::new() };
    Split { fixed_start: None, in_braces: format!("{fixed_start}{rest}"), fixed_end: None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brace_expansion() {
        let cases: [(&str, &[&str]); 18] = [
            ("eep/{a,b}/test", &["eep/a/test", "eep/b/test"]),
            ("eep/{a,b}/{c,d,e}", &["eep/a/c", "eep/a/d", "eep/a/e", "eep/b/c", "eep/b/d", "eep/b/e"]),
            ("eep/{a,b}/\\{c,d,e}", &["eep/a/{c,d,e}", "eep/b/{c,d,e}"]),
            ("eep/{a,b\\}/test", &["eep/{a,b}/test"]),
            ("eep/{a,b\\\\}/test", &["eep/a/test", "eep/b\\\\/test"]),
            ("eep/{a,b\\\\\\}/test", &["eep/{a,b\\\\}/test"]),
            ("e\\{ep/{a,b}/test", &["e{ep/a/test", "e{ep/b/test"]),
            ("eep/{a,\\b}/test", &["eep/a/test", "eep/\\b/test"]),
            ("{a/*.*,b/*.*}", &["a/*.*", "b/*.*"]),
            ("{{}", &["{{}"]),
            ("aa{{}", &["aa{{}"]),
            ("{b{}", &["{b{}"]),
            ("{{}c", &["{{}c"]),
            ("{{}}", &["{{}}"]),
            ("\\{{}}", &["{}"]),
            ("{}foo", &["foo"]),
            ("bar{ }foo", &["bar foo"]),
            ("{}", &[""]),
        ];
        for (pattern, expected) in cases {
            assert_eq!(perform_brace_expansion_for_ripgrep(pattern), expected, "{pattern}");
        }
    }

    #[test]
    fn exclude_exception_globs_walk_only_the_folders_leading_to_anchored_exceptions() {
        let globs = |exceptions: &[&str]| exclude_exception_globs(&exceptions.iter().map(|exception| exception.to_string()).collect::<Vec<_>>());
        assert_eq!(
            globs(&["node_modules/{@types,typescript}", "src/**/keep.js", "gulpfile.js"]),
            [
                "!*", "/node_modules/", "/src/", "/src/**/", "/node_modules/@types", "/node_modules/@types/**", "/node_modules/typescript",
                "/node_modules/typescript/**", "/src/**/keep.js", "/src/**/keep.js/**", "/gulpfile.js", "/gulpfile.js/**", "!**/.git", "!**/.svn",
                "!**/.hg", "!**/.jj"
            ]
        );
        assert_eq!(
            globs(&["node_modules/@types", "**/Scripts"]),
            ["/node_modules/@types", "/node_modules/@types/**", "**/Scripts", "**/Scripts/**", "!**/.git", "!**/.svn", "!**/.hg", "!**/.jj"]
        );
        assert_eq!(
            globs(&["{.git,.hg}/hooks"]),
            ["!*", "/.git/", "/.hg/", "/.git/hooks", "/.git/hooks/**", "/.hg/hooks", "/.hg/hooks/**", "!**/.svn", "!**/.jj"]
        );
    }
}
