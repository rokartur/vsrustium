//! clipboard.ts and clipboardUtils.ts on the editor side: the rich text copy (`getRichTextToCopy`,
//! textToHtmlTokenizer.ts `tokenizeLineToHTML`) and the column block paste (columnSelectionPasteCommand.ts).

use icu_segmenter::GraphemeClusterSegmenter;
use serde_json::Value;

use super::edit::{Command, Replace, overtype_end};
use super::{Position, Selection};

/// `EDITOR_FONT_DEFAULTS.fontFamily` on macOS.
pub const DEFAULT_FONT_FAMILY: &str = "Menlo, Monaco, 'Courier New', monospace";

/// A token of a line for the HTML copy: where it ends (chars), its foreground color id and font style bits
/// (`MetadataConsts` order: italic 1, bold 2, underline 4, strikethrough 8).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HtmlToken {
    pub end: usize,
    pub foreground: usize,
    pub font_style: i32,
}

/// The editor font as `fontInfo` gives it to the copy: family, weight, size and line height in pixels.
pub struct FontInfo {
    pub family: String,
    pub weight: String,
    pub size: f64,
    pub line_height: f64,
}

/// The validated `editor.fontFamily`, `editor.fontWeight`, `editor.fontSize` and `editor.lineHeight` as
/// `BareFontInfo._create` turns them into the font info, the editor zoom applied.
pub fn font_info(family: Option<Value>, weight: Option<Value>, size: Option<Value>, line_height: Option<Value>, zoom: f64) -> FontInfo {
    let family = family.as_ref().and_then(Value::as_str).unwrap_or(DEFAULT_FONT_FAMILY).to_string();
    // `EditorFontWeight.validate`: "normal", "bold" or a number from 1 to 1000.
    let weight = match weight.as_ref().and_then(Value::as_str) {
        Some(weight @ ("normal" | "bold")) => weight.to_string(),
        Some(weight) if weight.parse::<u32>().is_ok_and(|number| (1..=1000).contains(&number)) => weight.to_string(),
        _ => "normal".to_string(),
    };
    // `EditorFontSize.validate`: 0 or no number is the default, else clamped to 6..100.
    let size = match size.as_ref().and_then(Value::as_f64) {
        Some(size) if size != 0. => size.clamp(6., 100.),
        _ => 12.,
    };
    let line_height = line_height.as_ref().and_then(Value::as_f64).unwrap_or(0.).clamp(0., 150.);
    // Values under `MINIMUM_LINE_HEIGHT` 8 are in ems; 0 is `GOLDEN_LINE_HEIGHT_RATIO` 1.5 times the font size.
    let line_height = match line_height {
        0. => 1.5 * size,
        height if height < 8. => height * size,
        height => height,
    };
    let line_height = line_height.round().max(8.);
    FontInfo { family, weight, size: size * zoom, line_height: line_height * zoom }
}

/// `getRichTextToCopy`: the `<div>` with the editor colors and font around each line's `tokenizeLineToHTML`,
/// and the language as `mode`; none for plain text or several selections. `line_tokens` gives a line's tokens.
pub fn rich_text_to_copy(
    lines: &[String],
    language_id: &str,
    selections: &[Selection],
    empty_selection_clipboard: bool,
    font: &FontInfo,
    color_map: &[String],
    tab_size: usize,
    line_tokens: impl Fn(usize) -> Vec<HtmlToken>,
) -> Option<(String, String)> {
    if language_id == "plaintext" || selections.len() != 1 {
        return None;
    }
    let mut start = selections[0].start();
    let mut end = selections[0].end();
    if start == end {
        if !empty_selection_clipboard {
            return None;
        }
        start = Position::new(start.line, 0);
        end = Position::new(start.line, lines[start.line].chars().count());
    }
    let family = html_font_family(&font.family);
    let mut html = format!(
        "<div style=\"color: {};background-color: {};font-family: {family};font-weight: {};font-size: {}px;line-height: {}px;white-space: pre;\">",
        color_map[1], color_map[2], font.weight, font.size, font.line_height
    );
    for line in start.line..=end.line {
        let text = &lines[line];
        let start_offset = if line == start.line { start.column } else { 0 };
        let end_offset = if line == end.line { end.column } else { text.chars().count() };
        if text.is_empty() {
            html.push_str("<br>");
        } else {
            html.push_str(&tokenize_line_to_html(text, &line_tokens(line), color_map, start_offset, end_offset, tab_size, false));
        }
    }
    html.push_str("</div>");
    Some((html, language_id.to_string()))
}

/// The font family of `getRichTextToCopy`: the default when the family holds `:;\/<>` or is the default,
/// else the family quoted when it needs quotes, with the default after it.
fn html_font_family(family: &str) -> String {
    if family.contains([':', ';', '\\', '/', '<', '>']) || family == DEFAULT_FONT_FAMILY {
        return DEFAULT_FONT_FAMILY.to_string();
    }
    let mut family = family.replace('"', "'");
    if !family.contains([',', '\'']) && family.contains(['+', ' ']) {
        family = format!("'{family}'");
    }
    format!("{family}, {DEFAULT_FONT_FAMILY}")
}

/// `tokenizeLineToHTML` over chars `start_offset..end_offset`: one span per token with its inline style, tabs
/// expanded to their stops; `use_nbsp` (Windows) turns every other space into `&#160;`.
pub fn tokenize_line_to_html(text: &str, tokens: &[HtmlToken], color_map: &[String], start_offset: usize, end_offset: usize, tab_size: usize, use_nbsp: bool) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut result = String::from("<div>");
    let mut char_index = 0;
    let mut width = 0;
    let mut prev_is_space = true;
    for token in tokens {
        let mut part = String::new();
        while char_index < token.end && char_index < end_offset {
            let c = chars[char_index];
            width += if is_full_width(c) { 2 } else if c == '\t' { 0 } else { 1 };
            if char_index < start_offset {
                if c == '\t' {
                    width += tab_size - width % tab_size;
                }
                char_index += 1;
                continue;
            }
            let mut space = |part: &mut String| {
                if use_nbsp && prev_is_space {
                    part.push_str("&#160;");
                    prev_is_space = false;
                } else {
                    part.push(' ');
                    prev_is_space = true;
                }
            };
            match c {
                '\t' => {
                    let spaces = tab_size - width % tab_size;
                    width += spaces;
                    for _ in 0..spaces {
                        space(&mut part);
                    }
                }
                ' ' => space(&mut part),
                _ => {
                    match c {
                        '<' => part.push_str("&lt;"),
                        '>' => part.push_str("&gt;"),
                        '&' => part.push_str("&amp;"),
                        '\0' => part.push_str("&#00;"),
                        '\u{feff}' | '\u{2028}' | '\u{2029}' | '\u{85}' => part.push('\u{fffd}'),
                        // A zero width space: a carriage return would break the line.
                        '\r' => part.push_str("&#8203"),
                        c => part.push(c),
                    }
                    prev_is_space = false;
                }
            }
            char_index += 1;
        }
        if token.end <= start_offset {
            continue;
        }
        result.push_str(&format!("<span style=\"{}\">{part}</span>", inline_style(token, color_map)));
        if token.end > end_offset || char_index >= end_offset || start_offset >= end_offset {
            break;
        }
    }
    result.push_str("</div>");
    result
}

/// `TokenMetadata.getInlineStyleFromMetadata`.
fn inline_style(token: &HtmlToken, color_map: &[String]) -> String {
    let mut style = format!("color: {};", color_map[token.foreground]);
    if token.font_style & textmate::ITALIC != 0 {
        style.push_str("font-style: italic;");
    }
    if token.font_style & textmate::BOLD != 0 {
        style.push_str("font-weight: bold;");
    }
    let mut decoration = String::new();
    if token.font_style & textmate::UNDERLINE != 0 {
        decoration.push_str(" underline");
    }
    if token.font_style & textmate::STRIKETHROUGH != 0 {
        decoration.push_str(" line-through");
    }
    if !decoration.is_empty() {
        style.push_str(&format!("text-decoration:{decoration};"));
    }
    style
}

/// strings.ts `isFullWidthCharacter`.
fn is_full_width(c: char) -> bool {
    matches!(c as u32, 0x2E80..=0xD7AF | 0xF900..=0xFAFF | 0xFF01..=0xFF5E)
}

/// `ColumnSelectionPasteCommand`: the lines of a copied column block pasted from the selection down, at the
/// selection's visible column, each padded with spaces to the block's width; lines past the end are appended
/// indented to that column. The cursor ends after the last line's text.
pub fn column_selection_paste(lines: &[String], selection: Selection, text: &[String], tab_size: usize, overtype: bool) -> Command {
    let start = selection.start();
    let column = visible_column_from_column(&lines[start.line], start.column, tab_size);
    let text = pad_to_block_width(text, column, tab_size);
    let last_line = lines.len() - 1;
    let mut edits = Vec::new();
    for (index, line_text) in text.iter().enumerate() {
        let line = selection.end().line + index;
        if line > last_line {
            let end = Position::new(last_line, lines[last_line].chars().count());
            let padding = " ".repeat(column);
            edits.push(Replace::new(end, end, format!("\n{padding}{}", text[index..].join(&format!("\n{padding}")))));
            break;
        }
        let (range, line_text) = match index {
            0 => (selection, line_text.clone()),
            _ => {
                let at = column_from_visible_column(&lines[line], column, tab_size);
                let mut padded = line_text.clone();
                if at == lines[line].chars().count() {
                    let end_column = visible_column_from_column(&lines[line], at, tab_size);
                    padded = " ".repeat(column.saturating_sub(end_column)) + line_text;
                }
                (Selection::caret(Position::new(line, at)), padded)
            }
        };
        let end = if overtype { overtype_end(lines, range, &line_text) } else { range.end() };
        edits.push(Replace::new(range.start(), end, line_text));
    }
    Command { edits, tracked: None, cursor_state: Box::new(|_, ranges| Selection::caret(ranges.last().expect("a block paste edits").1)) }
}

/// `_padTextToBlockWidth`.
fn pad_to_block_width(text: &[String], column: usize, tab_size: usize) -> Vec<String> {
    let prefix = " ".repeat(column);
    let ends: Vec<usize> = text.iter().map(|line| {
        let with_prefix = format!("{prefix}{line}");
        visible_column_from_column(&with_prefix, with_prefix.chars().count(), tab_size)
    }).collect();
    let block_end = ends.iter().copied().fold(column, usize::max);
    text.iter().zip(ends).map(|(line, end)| format!("{line}{}", " ".repeat(block_end - end))).collect()
}

/// The graphemes of `line` as (first char column, char column after it, first code point).
fn graphemes(line: &str) -> Vec<(usize, usize, char)> {
    let breaks: Vec<usize> = GraphemeClusterSegmenter::new().segment_str(line).collect();
    let column = |byte: usize| line[..byte].chars().count();
    breaks.windows(2).map(|pair| (column(pair[0]), column(pair[1]), line[pair[0]..].chars().next().expect("a grapheme has a char"))).collect()
}

/// `CursorColumns._nextVisibleColumn`.
fn next_visible_column(c: char, visible: usize, tab_size: usize) -> usize {
    match c {
        '\t' => visible + tab_size - visible % tab_size,
        c if is_full_width(c) || is_emoji_imprecise(c) => visible + 2,
        _ => visible + 1,
    }
}

/// `CursorColumns.visibleColumnFromColumn` over graphemes. shortcut: the rest of the port counts one visible
/// column per char (editor.rs `visible_column`); move it here once wide chars are drawn two columns wide.
fn visible_column_from_column(line: &str, column: usize, tab_size: usize) -> usize {
    let text: String = line.chars().take(column).collect();
    graphemes(&text).into_iter().fold(0, |visible, (_, _, c)| next_visible_column(c, visible, tab_size))
}

/// `CursorColumns.columnFromVisibleColumn`: the grapheme edge nearest `visible`, the earlier one on a tie.
fn column_from_visible_column(line: &str, visible: usize, tab_size: usize) -> usize {
    if visible == 0 {
        return 0;
    }
    let (mut before_visible, mut before_column) = (0, 0);
    for (_, after_column, c) in graphemes(line) {
        let after_visible = next_visible_column(c, before_visible, tab_size);
        if after_visible >= visible {
            return if after_visible - visible < visible - before_visible { after_column } else { before_column };
        }
        (before_visible, before_column) = (after_visible, after_column);
    }
    line.chars().count()
}

/// strings.ts `isEmojiImprecise`.
fn is_emoji_imprecise(c: char) -> bool {
    matches!(c as u32, 0x1F1E6..=0x1F1FF | 8986 | 8987 | 9200 | 9203 | 9728..=10175 | 11088 | 11093 | 127744..=128591 | 128640..=128764 | 128992..=129008 | 129280..=129535 | 129648..=129782)
}

#[cfg(test)]
mod tests;
