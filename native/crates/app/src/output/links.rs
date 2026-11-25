#![allow(dead_code, reason = "wired into the output view next")]

use std::path::{Path, PathBuf};

use regex::{Regex, RegexBuilder};

/// One link found in an output line: outputLinkComputer.ts `ILink` with its `url` fragment `#line,column` split out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputLink {
    /// Start column (1-based) and end column (exclusive), counted in chars.
    pub range: (usize, usize),
    pub path: PathBuf,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

/// outputLinkComputer.ts `OutputLinkComputer.createPatterns`, without the Windows backslash variant.
pub fn create_patterns(workspace_folder: &Path) -> Vec<Regex> {
    let folder = regex::escape(&workspace_folder.to_string_lossy());
    let valid_path_character_pattern = r#"[^\s\(\):<>'"]"#;
    let valid_path_character_or_space_pattern =
        format!("(?:{valid_path_character_pattern}| {valid_path_character_pattern})");
    let path_pattern =
        format!(r"{valid_path_character_or_space_pattern}+\.{valid_path_character_pattern}+");
    let strict_path_pattern = format!("{valid_path_character_pattern}+");

    [
        format!(r"{folder}({path_pattern}) on line ((\d+)(, column (\d+))?)"),
        format!(r"{folder}({path_pattern}):line ((\d+)(, column (\d+))?)"),
        format!(r"{folder}({path_pattern})(\s?\((\d+)(,(\d+))?)\)"),
        format!(r"{folder}({strict_path_pattern})(:(\d+))?(:(\d+))?"),
    ]
    .iter()
    .map(|pattern| {
        RegexBuilder::new(pattern)
            .case_insensitive(true)
            .build()
            .expect("output link pattern compiles")
    })
    .collect()
}

/// outputLinkComputer.ts `OutputLinkComputer.detectLinks`, with `$computeLinks`'s `resources.joinPath(folderUri, ...)` as the resource creator.
pub fn detect_links(line: &str, patterns: &[Regex], workspace_folder: &Path) -> Vec<OutputLink> {
    let mut links: Vec<OutputLink> = Vec::new();
    for pattern in patterns {
        let mut offset = 0;
        for captures in pattern.captures_iter(line) {
            let folder_relative_path = captures[1].trim_end_matches('.').replace('\\', "/");
            let path = workspace_folder.join(folder_relative_path.trim_start_matches('/'));
            let line_number = captures.get(3).map(|m| m.as_str().parse().unwrap_or(u32::MAX));
            let column = line_number
                .and(captures.get(5))
                .map(|m| m.as_str().parse().unwrap_or(u32::MAX));

            let full_match = captures[0].trim_end_matches('.');
            let index = offset + line[offset..].find(full_match).expect("match lies past offset");
            offset = index + full_match.len();

            let start = line[..index].chars().count() + 1;
            let range = (start, start + full_match.chars().count());
            if links.iter().any(|link| intersecting_or_touching(link.range, range)) {
                break;
            }
            links.push(OutputLink { range, path, line: line_number, column });
        }
    }
    links
}

/// range.ts `Range.areIntersectingOrTouching` for two ranges on one line.
fn intersecting_or_touching(a: (usize, usize), b: (usize, usize)) -> bool {
    !(a.1 < b.0 || b.1 < a.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = "C:/Users/someone/AppData/Local/Temp/_monacodata_9888/workspaces/mankala";

    /// outputLinkProvider.test.ts `toOSPath` on macOS and Linux.
    fn to_os_path(line: &str) -> String {
        line.replace('\\', "/")
    }

    fn detect(line: &str) -> Vec<OutputLink> {
        detect_links(&to_os_path(line), &create_patterns(Path::new(ROOT)), Path::new(ROOT))
    }

    #[track_caller]
    fn assert_link(
        line: &str,
        relative: &str,
        line_number: Option<u32>,
        column: Option<u32>,
        range: (usize, usize),
    ) {
        let result = detect(line);
        assert_eq!(result.len(), 1, "{line}");
        assert_eq!(result[0].path, Path::new(ROOT).join(relative));
        assert_eq!((result[0].line, result[0].column), (line_number, column));
        assert_eq!(result[0].range, range);
    }

    #[test]
    fn link_detection() {
        assert_eq!(detect("Foo bar").len(), 0);

        assert_link(r" at C:\Users\someone\AppData\Local\Temp\_monacodata_9888\workspaces\mankala\Game.ts in", "Game.ts", None, None, (5, 84));
        assert_link(r" at C:\Users\someone\AppData\Local\Temp\_monacodata_9888\workspaces\mankala\Game.ts:336 in", "Game.ts", Some(336), None, (5, 88));
        assert_link(r" at C:\Users\someone\AppData\Local\Temp\_monacodata_9888\workspaces\mankala\Game.ts:336:9 in", "Game.ts", Some(336), Some(9), (5, 90));
        assert_link(r" at C:\Users\someone\AppData\Local\Temp\_monacodata_9888\workspaces\mankala\Game.ts:336:9 in", "Game.ts", Some(336), Some(9), (5, 90));
        assert_link(r" at C:\Users\someone\AppData\Local\Temp\_monacodata_9888\workspaces\mankala\Game.ts>dir in", "Game.ts", None, None, (5, 84));
        assert_link(r" at C:\Users\someone\AppData\Local\Temp\_monacodata_9888\workspaces\mankala\Game.ts:336:9] in", "Game.ts", Some(336), Some(9), (5, 90));

        let result = detect(r" at C:\Users\someone\AppData\Local\Temp\_monacodata_9888\workspaces\mankala\Game.ts] in");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].path, Path::new(ROOT).join("Game.ts]"));
        assert_eq!((result[0].line, result[0].column), (None, None));

        assert_link(r"C:\Users\someone\AppData\Local\Temp\_monacodata_9888\workspaces\mankala\Game.ts on line 8", "Game.ts", Some(8), None, (1, 90));
        assert_link(r"C:\Users\someone\AppData\Local\Temp\_monacodata_9888\workspaces\mankala\Game.ts on line 8, column 13", "Game.ts", Some(8), Some(13), (1, 101));
        assert_link(r"C:\Users\someone\AppData\Local\Temp\_monacodata_9888\workspaces\mankala\Game.ts on LINE 8, COLUMN 13", "Game.ts", Some(8), Some(13), (1, 101));
        assert_link(r"C:\Users\someone\AppData\Local\Temp\_monacodata_9888\workspaces\mankala\Game.ts:line 8", "Game.ts", Some(8), None, (1, 87));

        assert_link(" at File.put (C:/Users/someone/AppData/Local/Temp/_monacodata_9888/workspaces/mankala/Game.ts)", "Game.ts", None, None, (15, 94));
        assert_link(" at File.put (C:/Users/someone/AppData/Local/Temp/_monacodata_9888/workspaces/mankala/Game.ts:278)", "Game.ts", Some(278), None, (15, 98));
        assert_link(" at File.put (C:/Users/someone/AppData/Local/Temp/_monacodata_9888/workspaces/mankala/Game.ts:278:34)", "Game.ts", Some(278), Some(34), (15, 101));
        assert_link(" at File.put (C:/Users/someone/AppData/Local/Temp/_monacodata_9888/workspaces/mankala/Game.ts:278:34)", "Game.ts", Some(278), Some(34), (15, 101));

        let features = "lib/something/Features.ts";
        assert_link("C:/Users/someone/AppData/Local/Temp/_monacodata_9888/workspaces/mankala/lib/something/Features.ts(45): error", features, Some(45), None, (1, 102));
        assert_link("C:/Users/someone/AppData/Local/Temp/_monacodata_9888/workspaces/mankala/lib/something/Features.ts (45): error", features, Some(45), None, (1, 103));
        assert_link("C:/Users/someone/AppData/Local/Temp/_monacodata_9888/workspaces/mankala/lib/something/Features.ts(45,18): error", features, Some(45), Some(18), (1, 105));
        assert_link("C:/Users/someone/AppData/Local/Temp/_monacodata_9888/workspaces/mankala/lib/something/Features.ts(45,18): error", features, Some(45), Some(18), (1, 105));
        assert_link("C:/Users/someone/AppData/Local/Temp/_monacodata_9888/workspaces/mankala/lib/something/Features.ts (45,18): error", features, Some(45), Some(18), (1, 106));
        assert_link("C:/Users/someone/AppData/Local/Temp/_monacodata_9888/workspaces/mankala/lib/something/Features.ts (45,18): error", features, Some(45), Some(18), (1, 106));

        assert_link(r"C:\Users\someone\AppData\Local\Temp\_monacodata_9888\workspaces\mankala\lib\something\Features.ts(45): error", features, Some(45), None, (1, 102));
        assert_link(r"C:\Users\someone\AppData\Local\Temp\_monacodata_9888\workspaces\mankala\lib\something\Features.ts (45): error", features, Some(45), None, (1, 103));
        assert_link(r"C:\Users\someone\AppData\Local\Temp\_monacodata_9888\workspaces\mankala\lib\something\Features.ts(45,18): error", features, Some(45), Some(18), (1, 105));
        assert_link(r"C:\Users\someone\AppData\Local\Temp\_monacodata_9888\workspaces\mankala\lib\something\Features.ts(45,18): error", features, Some(45), Some(18), (1, 105));
        assert_link(r"C:\Users\someone\AppData\Local\Temp\_monacodata_9888\workspaces\mankala\lib\something\Features.ts (45,18): error", features, Some(45), Some(18), (1, 106));
        assert_link(r"C:\Users\someone\AppData\Local\Temp\_monacodata_9888\workspaces\mankala\lib\something\Features.ts (45,18): error", features, Some(45), Some(18), (1, 106));

        assert_link(r"C:\Users\someone\AppData\Local\Temp\_monacodata_9888\workspaces\mankala\lib\something\Features Special.ts (45,18): error", "lib/something/Features Special.ts", Some(45), Some(18), (1, 114));

        assert_link(r" at C:\Users\someone\AppData\Local\Temp\_monacodata_9888\workspaces\mankala\Game.ts. in", "Game.ts", None, None, (5, 84));

        assert_eq!(detect(r" at C:\Users\someone\AppData\Local\Temp\_monacodata_9888\workspaces\mankala\Game in").len(), 1);
        assert_eq!(detect(r" at C:\Users\someone\AppData\Local\Temp\_monacodata_9888\workspaces\mankala\Game\ in").len(), 1);

        assert_link(r#" at "C:\Users\someone\AppData\Local\Temp\_monacodata_9888\workspaces\mankala\Game.ts" in"#, "Game.ts", None, None, (6, 85));
        assert_link(r" at 'C:\Users\someone\AppData\Local\Temp\_monacodata_9888\workspaces\mankala\Game.ts' in", "Game.ts", None, None, (6, 85));
    }

    #[test]
    fn issue_106847() {
        let root = Path::new("C:/Users/username/Desktop");
        let line = to_os_path(r"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa C:\Users\username\Desktop\test-ts\prj.conf C:\Users\username\Desktop\test-ts\prj.conf C:\Users\username\Desktop\test-ts\prj.conf");
        let result = detect_links(&line, &create_patterns(root), root);
        assert_eq!(result.len(), 3);
        for link in result {
            assert!(link.range.0 > 0 && link.range.1 > 0);
        }
    }
}
