//! findSectionHeaders.ts: the `#region` folding markers and `MARK:` comments the minimap labels.

use onig::{Regex, RegexOptions, Region, SearchOptions, Syntax};

use super::Position;

/// findSectionHeaders.ts `CHUNK_SIZE` and `MAX_SECTION_LINES`: mark headers are matched over chunks of lines that
/// overlap by the most lines a header may span.
const CHUNK_SIZE: usize = 100;
const MAX_SECTION_LINES: usize = 5;

/// editorOptions.ts `editor.minimap.markSectionHeaderRegex` default.
pub const MARK_SECTION_HEADER_REGEX: &str = r"\bMARK:\s*(?<separator>\-?)\s*(?<label>.*)$";

pub struct FindOptions<'a> {
    /// `foldingRules.markers.start` with its flags; `None` finds no region headers.
    pub region_start: Option<&'a Regex>,
    /// `markSectionHeaderRegex` when `showMarkSectionHeaders` is on.
    pub mark_regex: Option<&'a str>,
    pub block_comment: Option<(&'a str, &'a str)>,
}

/// A header over `start..end` (lines and char columns, 0-based).
#[derive(Clone, Debug, PartialEq)]
pub struct SectionHeader {
    pub start: Position,
    pub end: Position,
    pub text: String,
    pub has_separator_line: bool,
    /// A mark header shows only when its start is in a comment token.
    pub should_be_in_comments: bool,
}

pub fn find_section_headers(lines: &[String], options: &FindOptions) -> Vec<SectionHeader> {
    let mut headers = Vec::new();
    if let Some(start) = options.region_start {
        headers.extend(collect_region_headers(lines, start));
    }
    if let Some(pattern) = options.mark_regex {
        headers.extend(collect_mark_headers(lines, pattern, options.block_comment));
    }
    headers
}

fn collect_region_headers(lines: &[String], start: &Regex) -> Vec<SectionHeader> {
    let label = group_index(start, "label");
    let mut headers = Vec::new();
    for (line, content) in lines.iter().enumerate() {
        let Some(captures) = start.captures(content) else { continue };
        let whole = captures.pos(0).expect("a match has group 0");
        let (label_start, label_end) = label.and_then(|index| captures.pos(index)).unwrap_or((whole.1, content.len()));
        if label_end <= label_start {
            continue;
        }
        let (text, has_separator_line) = header_text(&content[label_start..label_end]);
        if !text.is_empty() || has_separator_line {
            let column = |offset: usize| content[..offset].chars().count();
            headers.push(SectionHeader { start: Position::new(line, column(label_start)), end: Position::new(line, column(label_end)), text, has_separator_line, should_be_in_comments: false });
        }
    }
    headers
}

fn collect_mark_headers(lines: &[String], pattern: &str, block_comment: Option<(&str, &str)>) -> Vec<SectionHeader> {
    let mut headers: Vec<SectionHeader> = Vec::new();
    if pattern.trim().is_empty() {
        return headers;
    }
    let mut options = RegexOptions::REGEX_OPTION_CAPTURE_GROUP;
    if is_multiline_regex_source(pattern) {
        // JavaScript's `s` flag: `.` matches line breaks.
        options |= RegexOptions::REGEX_OPTION_MULTILINE;
    }
    let Ok(regex) = Regex::with_options(pattern, options, Syntax::default()) else { return headers };
    if leads_to_endless_loop(&regex, pattern) {
        return headers;
    }
    let (separator_group, label_group) = (group_index(&regex, "separator"), group_index(&regex, "label"));
    let mut start_line = 0;
    while start_line < lines.len() {
        let end_line = (start_line + CHUNK_SIZE).min(lines.len());
        let text = lines[start_line..end_line].join("\n");
        let mut last_index = 0;
        let mut region = Region::new();
        while last_index <= text.len() && regex.search_with_options(&text, last_index, text.len(), SearchOptions::SEARCH_OPTION_NONE, Some(&mut region)).is_some() {
            let (match_start, match_end) = region.pos(0).expect("a match has group 0");
            let preceding = &text[..match_start];
            let line = start_line + preceding.matches('\n').count();
            let matched = &text[match_start..match_end];
            let match_lines: Vec<&str> = matched.split('\n').collect();
            let line_start = preceding.rfind('\n').map_or(0, |index| index + 1);
            let start_column = text[line_start..match_start].chars().count();
            let end = match match_lines.len() {
                1 => Position::new(line, start_column + matched.chars().count()),
                height => Position::new(line + height - 1, match_lines[height - 1].chars().count()),
            };
            let group = |index: Option<usize>| index.and_then(|index| region.pos(index));
            let (separator_range, label_range) = (group(separator_group), group(label_group));
            let mut label = label_range.map_or(String::new(), |(start, end)| text[start..end].to_string());
            let mut separator = separator_range.map_or(String::new(), |(start, end)| text[start..end].to_string());
            if let Some((comment_start, comment_end)) = block_comment {
                let captures_start = separator_range.map_or(usize::MAX, |range| range.0).min(label_range.map_or(usize::MAX, |range| range.0)).min(match_end);
                let captures_line_start = text[..captures_start].rfind('\n').map_or(0, |index| index + 1);
                if is_inside_block_comment(&text[captures_line_start..captures_start], comment_start, comment_end) {
                    let contiguous = matches!((separator_range, label_range), (Some(separator), Some(label)) if separator.1 == label.0);
                    (label, separator) = strip_block_comment_end(&label, &separator, comment_end, contiguous);
                }
            }
            let header = SectionHeader { start: Position::new(line, start_column), end, has_separator_line: !separator.is_empty(), text: label, should_be_in_comments: true };
            if (!header.text.is_empty() || header.has_separator_line) && headers.last().is_none_or(|last| last.end.line < header.start.line) {
                headers.push(header);
            }
            // An empty match would stay put; upstream rules most out with `regExpLeadsToEndlessLoop`.
            last_index = if match_end > match_start { match_end } else { match_end + text[match_end..].chars().next().map_or(1, char::len_utf8) };
        }
        start_line += CHUNK_SIZE - MAX_SECTION_LINES;
    }
    headers
}

fn group_index(regex: &Regex, name: &str) -> Option<usize> {
    let mut found = None;
    regex.foreach_name(|group, indices| {
        if group == name {
            found = indices.first().map(|&index| index as usize);
        }
        found.is_none()
    });
    found
}

/// strings.ts `regExpLeadsToEndlessLoop`: a pattern that matches the empty string, but for the anchors meant to.
fn leads_to_endless_loop(regex: &Regex, pattern: &str) -> bool {
    if matches!(pattern, "^" | "^$" | "$" | r"^\s*$") {
        return false;
    }
    regex.find("").is_some()
}

/// textModelSearch.ts `isMultilineRegexSource`: a line break, or `\n`, `\r` or `\W`.
fn is_multiline_regex_source(pattern: &str) -> bool {
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        match c {
            '\n' => return true,
            '\\' => match chars.next() {
                Some('n' | 'r' | 'W') => return true,
                Some(_) => {}
                None => break,
            },
            _ => {}
        }
    }
    false
}

fn is_inside_block_comment(line_prefix: &str, start_token: &str, end_token: &str) -> bool {
    let mut open = false;
    let mut offset = 0;
    while offset < line_prefix.len() {
        let token = if open { end_token } else { start_token };
        let Some(index) = line_prefix[offset..].find(token) else { break };
        open = !open;
        offset += index + token.len();
    }
    open
}

fn strip_block_comment_end(label: &str, separator: &str, end_token: &str, contiguous: bool) -> (String, String) {
    let trimmed = label.trim_end();
    if let Some(stripped) = trimmed.strip_suffix(end_token) {
        return (stripped.trim_end().to_string(), separator.to_string());
    }
    let combined = format!("{separator}{trimmed}");
    if contiguous && trimmed.chars().count() < end_token.chars().count() {
        if let Some(remaining) = combined.strip_suffix(end_token) {
            let separator = if remaining.trim().is_empty() { "" } else { remaining };
            return (String::new(), separator.to_string());
        }
    }
    (label.to_string(), separator.to_string())
}

/// `getHeaderText`: trimmed, a leading dash draws the separator line, dashes at both ends dropped.
fn header_text(text: &str) -> (String, bool) {
    let text = text.trim();
    (text.trim_matches('-').to_string(), text.starts_with('-'))
}

/// minimap.ts `_fitSectionHeader`: a label wider than `max_width` keeps its start and end around an ellipsis.
pub fn fit_section_header(text: &str, max_width: f64, measure: impl Fn(&str) -> f64) -> String {
    let ellipsis = "…";
    let width = measure(text);
    let ellipsis_width = measure(ellipsis);
    if text.is_empty() || width <= max_width || width <= ellipsis_width {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let average_char_width = width / chars.len() as f64;
    let max_char_count = (((max_width - ellipsis_width) / average_char_width).floor() as isize - 1).max(0) as usize;
    let mut half = max_char_count.div_ceil(2);
    while half > 0 && chars[half - 1].is_whitespace() {
        half -= 1;
    }
    let tail = max_char_count - half;
    let start: String = chars[..half].iter().collect();
    let end: String = chars[chars.len() - tail.min(chars.len())..].iter().collect();
    format!("{start}{ellipsis}{end}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|line| line.to_string()).collect()
    }

    fn marks(model: &[String], pattern: &str) -> Vec<SectionHeader> {
        find_section_headers(model, &FindOptions { region_start: None, mark_regex: Some(pattern), block_comment: None })
    }

    /// (text, 1-based start line, 1-based end line), as the upstream tests assert them.
    fn summary(headers: &[SectionHeader]) -> Vec<(&str, usize, usize)> {
        headers.iter().map(|header| (header.text.as_str(), header.start.line + 1, header.end.line + 1)).collect()
    }

    const BANNER: &str = r"^// =+\n^// (?<label>[^\n]+?)\n^// =+$";
    const DASHED_BANNER: &str = r"^// [-=]+\n^// (?<label>[^\n]+?)\n^// [-=]+$";

    #[test]
    fn a_wide_label_keeps_its_ends_around_an_ellipsis_split_after_no_whitespace() {
        let measure = |text: &str| text.chars().count() as f64;
        assert_eq!(fit_section_header("abcdefghij", 10., measure), "abcdefghij");
        assert_eq!(fit_section_header("abcdefghij", 6., measure), "ab…ij");
        assert_eq!(fit_section_header("a cdefghij", 6., measure), "a…hij");
    }

    #[test]
    fn finds_simple_section_headers() {
        let model = lines(&["regular line", "MARK: My Section", "another line", "MARK: Another Section", "last line"]);
        assert_eq!(summary(&marks(&model, r"MARK:\s*(?<label>.*)$")), [("My Section", 2, 2), ("Another Section", 4, 4)]);
    }

    #[test]
    fn finds_section_headers_with_separators() {
        let model = lines(&["regular line", "MARK: -My Section", "another line", "MARK: - Another Section", "last line"]);
        let headers = marks(&model, r"MARK:\s*(?<separator>-?)\s*(?<label>.*)$");
        assert_eq!(headers.iter().map(|header| (header.text.as_str(), header.has_separator_line)).collect::<Vec<_>>(), [("My Section", true), ("Another Section", true)]);
    }

    #[test]
    fn finds_multi_line_section_headers_with_separators() {
        let model = lines(&["regular line", "// ==========", "// My Section", "// ==========", "code...", "// ==========", "// Another Section", "// ==========", "more code..."]);
        assert_eq!(summary(&marks(&model, BANNER)), [("My Section", 2, 4), ("Another Section", 6, 8)]);
    }

    #[test]
    fn handles_overlapping_and_consecutive_multi_line_section_headers() {
        let model = lines(&["// ==========", "// Section 1", "// ==========", "// ==========", "// Section 2", "// =========="]);
        assert_eq!(summary(&marks(&model, BANNER)), [("Section 1", 1, 3), ("Section 2", 4, 6)]);
    }

    #[test]
    fn section_headers_must_be_in_comments_when_specified() {
        let model = lines(&["// ==========", "// Section 1", "// ==========", "==========", "Section 2", "=========="]);
        assert!(marks(&model, r"^(?:// )?=+\n^(?:// )?(?<label>[^\n]+?)\n^(?:// )?=+$")[0].should_be_in_comments);
    }

    #[test]
    fn handles_section_headers_at_chunk_boundaries() {
        let mut model: Vec<String> = (0..150).map(|i| format!("line {i}")).collect();
        for (index, line) in ["// ==========", "// Section 1", "// ==========", "// ==========", "// Section 2", "// =========="].iter().enumerate() {
            model[97 + index] = line.to_string();
        }
        assert_eq!(summary(&marks(&model, BANNER)), [("Section 1", 98, 100), ("Section 2", 101, 103)]);
    }

    #[test]
    fn handles_empty_and_whitespace_only_regex_without_infinite_loop() {
        let model = lines(&["line 1", "line 2", "line 3"]);
        assert_eq!(marks(&model, ""), []);
        assert_eq!(marks(&model, "   "), []);
    }

    #[test]
    fn correctly_advances_past_matches() {
        let model = lines(&["// ==========", "// Section 1", "// ==========", "some code", "// ==========", "// Section 2", "// ==========", "more code", "// ==========", "// Section 3", "// =========="]);
        assert_eq!(summary(&marks(&model, BANNER)).iter().map(|header| header.0).collect::<Vec<_>>(), ["Section 1", "Section 2", "Section 3"]);
    }

    #[test]
    fn handles_nested_separators() {
        let model = lines(&["// ==============", "// Major Section", "// ==============", "", "// ----------", "// Subsection", "// ----------"]);
        assert_eq!(summary(&marks(&model, DASHED_BANNER)).iter().map(|header| header.0).collect::<Vec<_>>(), ["Major Section", "Subsection"]);
    }

    #[test]
    fn handles_overlapping_section_headers_without_duplicates() {
        let model = lines(&["// ==========", "// Section 1", "// ==========", "// Section 2", "// ==========", "// ==========", "// Section 3", "// ==========="]);
        assert_eq!(summary(&marks(&model, BANNER)), [("Section 1", 1, 3), ("Section 3", 6, 8)]);
    }

    #[test]
    fn handles_partially_overlapping_multiline_section_headers() {
        let model = lines(&["// ================", "// Major Section 1", "// ================", "// --------", "// Subsection 1.1", "// --------", "// ================", "// Major Section 2", "// ================"]);
        assert_eq!(summary(&marks(&model, DASHED_BANNER)), [("Major Section 1", 1, 3), ("Subsection 1.1", 4, 6), ("Major Section 2", 7, 9)]);
    }

    fn with_comments(model: &[String], pattern: &str, comment: (&str, &str)) -> Vec<(usize, String, bool)> {
        find_section_headers(model, &FindOptions { region_start: None, mark_regex: Some(pattern), block_comment: Some(comment) })
            .into_iter()
            .map(|header| (header.start.line + 1, header.text, header.has_separator_line))
            .collect()
    }

    fn expected(headers: &[(usize, &str, bool)]) -> Vec<(usize, String, bool)> {
        headers.iter().map(|&(line, text, separator)| (line, text.to_string(), separator)).collect()
    }

    #[test]
    fn strips_the_block_comment_end_token_from_mark_section_header_labels() {
        let model = lines(&[
            "<!-- MARK: Html Section -->",
            "/* MARK: - C Section */",
            "<!-- MARK: Spaced Section   -->   ",
            "<!-- MARK: -->",
            "<!-- MARK: - -->",
            "// MARK: Line Comment Section */ trailing",
            "// MARK: Regex /foo.*/",
            "/* closed */ // MARK: After Closed Comment */",
        ]);
        assert_eq!(
            with_comments(&model, MARK_SECTION_HEADER_REGEX, ("<!--", "-->")),
            expected(&[
                (1, "Html Section", false),
                (2, "C Section */", true),
                (3, "Spaced Section", false),
                (5, "", true),
                (6, "Line Comment Section */ trailing", false),
                (7, "Regex /foo.*/", false),
                (8, "After Closed Comment */", false),
            ])
        );
        assert_eq!(
            with_comments(&model, MARK_SECTION_HEADER_REGEX, ("/*", "*/")),
            expected(&[
                (1, "Html Section -->", false),
                (2, "C Section", true),
                (3, "Spaced Section   -->   ", false),
                (4, "->", true),
                (5, "-->", true),
                (6, "Line Comment Section */ trailing", false),
                (7, "Regex /foo.*/", false),
                (8, "After Closed Comment */", false),
            ])
        );
    }

    #[test]
    fn keeps_the_separator_when_the_block_comment_end_token_is_split_across_the_separator_and_the_label() {
        let model = lines(&["<!-- MARK: - -->", "<!-- MARK: -->"]);
        assert_eq!(with_comments(&model, r"\bMARK:\s*(?<separator>[- ]*)\s*(?<label>.*)$", ("<!--", "-->")), expected(&[(1, "", true)]));
    }

    #[test]
    fn determines_the_block_comment_context_from_the_captured_text() {
        let model = lines(&["<!-- MARK: Section -->", "<!-- MARK: - -> -->"]);
        let comment = ("<!--", "-->");
        let texts = |pattern: &str| with_comments(&model, pattern, comment).into_iter().map(|(_, text, separator)| (text, separator)).collect::<Vec<_>>();
        assert_eq!(texts(r"^<!--\s*MARK:\s*(?<label>.*)$"), [("Section".to_string(), false), ("- ->".to_string(), false)]);
        assert_eq!(texts(r"\bMARK:\s*(?<separator>-?)\s*(?<label>.*?)\s*-->$"), [("Section".to_string(), false), ("->".to_string(), true)]);
    }

    #[test]
    fn supports_identical_block_comment_start_and_end_tokens() {
        let model = lines(&[r#"""" MARK: Doc Section """"#, r#""""closed""" # MARK: Quote """"#]);
        let texts: Vec<String> = with_comments(&model, MARK_SECTION_HEADER_REGEX, (r#"""""#, r#"""""#)).into_iter().map(|header| header.1).collect();
        assert_eq!(texts, ["Doc Section", r#"Quote """"#]);
    }

    #[test]
    fn keeps_mark_section_header_labels_without_block_comment_tokens_unchanged() {
        let model = lines(&["<!-- MARK: Html Section -->"]);
        assert_eq!(summary(&marks(&model, MARK_SECTION_HEADER_REGEX))[0].0, "Html Section -->");
        assert_eq!(with_comments(&model, r"\bMARK:\s*(?<label>\w+ \w+)", ("<!--", "-->"))[0].1, "Html Section");
    }

    #[test]
    fn takes_region_section_header_labels_from_the_label_group_of_the_folding_start_marker() {
        let regions = |model: &[&str], start: &str| {
            let regex = Regex::with_options(start, RegexOptions::REGEX_OPTION_CAPTURE_GROUP, Syntax::default()).expect("valid marker");
            find_section_headers(&lines(model), &FindOptions { region_start: Some(&regex), mark_regex: None, block_comment: None })
                .into_iter()
                .map(|header| (header.text, header.start.line + 1, header.start.column + 1, header.end.column + 1))
                .collect::<Vec<_>>()
        };
        let html = regions(&["<!-- #region Html Section -->", "<!-- #region -->", "  <!--#region Indented-->"], r"^\s*<!--\s*#?region\b\s*(?<label>.*?)\s*-->");
        assert_eq!(html, [("Html Section".to_string(), 1, 14, 26), ("Indented".to_string(), 3, 15, 23)]);
        let css = regions(&["/* #region Css Section */", "/* #region */"], r"^\s*\/\*\s*#region\b\s*(?<label>.*?)\s*\*\/");
        assert_eq!(css, [("Css Section".to_string(), 1, 12, 23)]);
        let typescript = regions(&["// #region Ts Section", "// #region"], r"^\s*//\s*#?region\b");
        assert_eq!(typescript, [("Ts Section".to_string(), 1, 11, 22)]);
    }
}
