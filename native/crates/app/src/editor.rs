//! A text file opened in the editor and the editor group holding it: the lines as the text model
//! holds them (textModel.ts), and what the editor derives from them for drawing (indentation,
//! indent guides, word highlights).

use std::cell::{Ref, RefCell, RefMut};
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use config::configuration::Overrides;
use config::indentation::creation_options;
use config::service::Settings;
use files::encoding;

use crate::diff_editor::DiffInput;
use crate::multi_diff_editor::MultiDiffInput;
use crate::languages::Languages;
use crate::syntax::Tokenization;

mod anchor_select;
pub mod auto_indent;
mod bracket_matching;
mod caret_operations;
pub mod clipboard;
pub mod commands;
pub mod cursor_undo;
pub mod decoration_types;
mod comment;
pub(crate) mod cursor;
pub mod drag_and_drop;
mod edit;
pub mod find;
pub mod folding;
pub mod injected_text;
pub(crate) mod in_place_replace;
mod folding_controller;
mod graphemes;
mod indentation;
mod lines_operations;
pub mod marker_decorations;
pub mod save_participants;
pub mod view_zones;
pub mod view_state;
pub mod minimap;
pub mod model_service;
mod multicursor;
pub mod section_headers;
pub mod smart_select;
pub mod smooth_scroll;
pub(crate) mod snippet;
pub mod snippet_session;
pub mod sticky_scroll;
pub mod unicode_highlighter;
pub mod unusual_line_terminators;
mod view;
pub mod whitespace;
pub mod word_operations;
pub mod word_wrap;
pub mod zoom;

pub use comment::LineCommentKind;
pub use lines_operations::CaseTransform;
pub use cursor::{Position, Selection, column_from_visible};
use folding::{FoldingMarkers, FoldingModel};
pub use edit::{ContentChange, ContentChanged, CursorEdit, EditReason, History, Pasted, Replace, char_column, end_of, selections_after, text_between, utf16_column};
pub use multicursor::{ColumnSelectData, Reveal};
pub use snippet::{SnippetText, expand as expand_snippet};
pub use view::{HORIZONTAL_SCROLLBAR_HEIGHT, LINE_HEIGHT, Scrollbar, VERTICAL_SCROLLBAR_WIDTH, VerticalReveal, Viewport};

pub struct TextDocument {
    /// The URI scheme: "file", "untitled", or the scheme of an extension's file system provider.
    pub scheme: String,
    /// The file path, or the URI path for other schemes.
    pub path: PathBuf,
    pub lines: Vec<String>,
    pub eol: &'static str,
    /// The `textFileService` encoding id, such as "utf8", "utf8bom" or "windows1252".
    pub encoding: String,
    pub language_id: String,
    pub indentation: Indentation,
    /// `foldingRules.offSide` of the language, used by the indent guides of blank lines.
    pub off_side: bool,
    /// `foldingRules.markers` of the language.
    pub folding_markers: Option<Arc<FoldingMarkers>>,
    /// `comments.blockComment` start and end tokens of the language.
    pub block_comment: Option<(String, String)>,
    pub tokens: Tokenization,
    /// `TextModel.isTooLargeForTokenization`, decided at creation: no tokens, folding, word highlights or sticky scroll.
    pub too_large_for_tokenization: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Indentation {
    pub tab_size: usize,
    /// `indentSize`; `None` while it follows the tab size (`'tabSize'`).
    pub indent_size: Option<usize>,
    pub insert_spaces: bool,
}

impl Indentation {
    pub fn indent_size(&self) -> usize {
        self.indent_size.unwrap_or(self.tab_size)
    }
}

/// `editor.tabSize` and `editor.insertSpaces` defaults, for documents built without settings.
#[cfg(test)]
const DEFAULT_INDENTATION: Indentation = Indentation { tab_size: 4, indent_size: None, insert_spaces: true };

impl TextDocument {
    pub fn read(path: &Path, languages: &Languages, settings: &Settings) -> std::io::Result<Self> {
        let bytes = std::fs::read(path)?;
        Ok(Self::from_bytes("file", path, &bytes, None, languages, settings))
    }

    /// A document of any scheme from its bytes, decoded as `TextFileService.readStream` does: in the `requested`
    /// encoding, else the one its BOM or zero bytes show, else `files.encoding`.
    // shortcut: binary files open as text; upstream refuses them (FILE_IS_BINARY) and shows the binary editor.
    pub fn from_bytes(scheme: &str, path: &Path, bytes: &[u8], requested: Option<&str>, languages: &Languages, settings: &Settings) -> Self {
        let detected = encoding::detect(bytes).encoding;
        let encoding = encoding::read_encoding(settings, (scheme == "file").then_some(path), requested, detected);
        let text = encoding::decode(bytes, &encoding);
        Self::from_text(scheme, path, &text, encoding, None, languages, settings)
    }

    /// An untitled model (untitledTextEditorModel.ts): `language_id` or the language of its name and first line.
    pub fn untitled(path: &Path, text: &str, language_id: Option<&str>, languages: &Languages, settings: &Settings) -> Self {
        let (encoding, _) = encoding::write_encoding(settings, None, None);
        Self::from_text("untitled", path, text, encoding, language_id, languages, settings)
    }

    pub fn from_text(scheme: &str, path: &Path, text: &str, encoding: String, language_id: Option<&str>, languages: &Languages, settings: &Settings) -> Self {
        let (lines, found_eol) = split_lines(text);
        let associations = settings.get_value(Some("files.associations"), &Overrides::default()).unwrap_or_default();
        let (uri_path, _) = split_query(scheme, path);
        let configured = Languages::configured_language(&associations, uri_path);
        let language_id = language_id.or(configured.as_deref()).unwrap_or_else(|| languages.language_of(uri_path, lines[0].as_str())).to_string();
        let off_side = languages.off_side(&language_id);
        let folding_markers = languages.folding_markers(&language_id);
        let block_comment = languages.block_comment(&language_id).map(|(start, end)| (start.to_string(), end.to_string()));
        let indentation = creation_indentation(&lines, settings, path, &language_id);
        let eol = found_eol.unwrap_or_else(|| default_eol(settings, path, &language_id));
        let too_large_for_tokenization = too_large_for_tokenization(text, lines.len(), settings, path, &language_id);
        let tokens = Tokenization::new(languages.grammars.grammar(&language_id).filter(|_| !too_large_for_tokenization));
        TextDocument { scheme: scheme.to_string(), path: path.to_path_buf(), lines, eol, encoding, language_id, indentation, off_side, folding_markers, block_comment, tokens, too_large_for_tokenization }
    }

    pub fn uri_string(&self) -> String {
        uri_string(&self.scheme, &self.path)
    }

    /// `UriComponents` as the extension host revives them.
    pub fn uri(&self) -> serde_json::Value {
        uri_value(&self.scheme, &self.path)
    }

    pub fn is(&self, scheme: &str, path: &Path) -> bool {
        self.scheme == scheme && self.path == path
    }

    /// uriIdentityService.ts `extUri.isEqual` for the disk, which has no PathCaseSensitive: a path in any case.
    pub fn is_file(&self, path: &Path) -> bool {
        self.scheme == "file" && self.path.to_string_lossy().to_lowercase() == path.to_string_lossy().to_lowercase()
    }

    pub fn name(&self) -> String {
        split_query(&self.scheme, &self.path).0.file_name().unwrap_or_default().to_string_lossy().into_owned()
    }
}

/// The `UriComponents` of a model's scheme and path, as the extension host revives them.
pub fn uri_value(scheme: &str, path: &Path) -> serde_json::Value {
    let (path, fragment) = split_fragment(scheme, path);
    let (path, query) = split_query(scheme, path);
    let path = path.to_string_lossy();
    // host_requests.rs `uri_parts` keys a non-file URI's authority as the `//authority` start of its path.
    let (authority, path) = match path.strip_prefix("//").filter(|_| scheme != "file") {
        Some(rest) => rest.split_at(rest.find('/').unwrap_or(rest.len())),
        None => ("", &*path),
    };
    serde_json::json!({ "$mid": 1, "scheme": scheme, "authority": authority, "path": path, "query": query, "fragment": fragment })
}

/// A non-file URI's query is kept after a `?` in its model's path (host_requests.rs `uri_parts`), so models of
/// one path with different queries (git's `ref`) stay apart: the path without it and the query.
pub fn split_query<'a>(scheme: &str, path: &'a Path) -> (&'a Path, &'a str) {
    let text = path.to_str().unwrap_or_default();
    match text.split_once('?').filter(|_| scheme != "file") {
        Some((path, query)) => (Path::new(path), query),
        None => (path, ""),
    }
}

/// A notebook cell's URI differs from its notebook's only in the fragment, kept after a `#` in its model's path.
pub fn split_fragment<'a>(scheme: &str, path: &'a Path) -> (&'a Path, &'a str) {
    let text = path.to_str().unwrap_or_default();
    match text.rsplit_once('#').filter(|_| scheme == crate::notebook::CELL_SCHEME) {
        Some((path, fragment)) => (Path::new(path), fragment),
        None => (path, ""),
    }
}

/// The indentation a model starts with (`TextModel.resolveOptions` from `getCreationOptions`): the language's
/// `editor.tabSize` and `editor.insertSpaces`, guessed from the text when `editor.detectIndentation` is on.
fn creation_indentation(lines: &[String], settings: &Settings, path: &Path, language_id: &str) -> Indentation {
    let options = creation_options(&settings.configuration, path, language_id);
    let configured = Indentation { tab_size: options.tab_size, indent_size: options.indent_size, insert_spaces: options.insert_spaces };
    match options.detect_indentation {
        true => guess_indentation(lines, Indentation { indent_size: None, ..configured }),
        false => configured,
    }
}

/// modelService.ts `_getEOL` and `_readModelOptions`: the language's `files.eol` when it is `\n` or `\r\n`, else LF
/// as `auto` gives on macOS. textResourcePropertiesService.ts reads it with the resource too.
fn default_eol(settings: &Settings, path: &Path, language_id: &str) -> &'static str {
    let overrides = Overrides { resource: Some(path), override_identifier: Some(language_id) };
    match settings.get_value(Some("files.eol"), &overrides) {
        Some(eol) if eol == "\r\n" => "\r\n",
        _ => "\n",
    }
}

/// Lines split at `\r\n`, `\r` or `\n`; the EOL is CRLF when most line breaks hold a CR, else LF, and none for a
/// text without line breaks (PieceTreeTextBufferBuilder `_getEOL`).
pub(crate) fn split_lines(text: &str) -> (Vec<String>, Option<&'static str>) {
    let (mut lines, mut line) = (Vec::new(), String::new());
    let (mut breaks, mut with_cr) = (0, 0);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                breaks += 1;
                with_cr += 1;
                chars.next_if_eq(&'\n');
                lines.push(std::mem::take(&mut line));
            }
            '\n' => {
                breaks += 1;
                lines.push(std::mem::take(&mut line));
            }
            _ => line.push(c),
        }
    }
    lines.push(line);
    let eol = match breaks {
        0 => None,
        _ if with_cr * 2 > breaks => Some("\r\n"),
        _ => Some("\n"),
    };
    (lines, eol)
}

/// `guessIndentation` (indentationGuesser.ts), over UTF-16 code units as upstream counts them.
/// `URI.toString()`: `file:///a%20b` for a file, `scheme:/path` for a URI without authority.
pub fn uri_string(scheme: &str, path: &Path) -> String {
    let mut uri = match scheme {
        "file" => String::from("file://"),
        scheme => format!("{scheme}:"),
    };
    let (path, fragment) = split_fragment(scheme, path);
    let (path, query) = split_query(scheme, path);
    let encode = |uri: &mut String, text: &str, keep_slash: bool| {
        for byte in text.bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => uri.push(byte as char),
                b'/' if keep_slash => uri.push('/'),
                _ => uri.push_str(&format!("%{byte:02X}")),
            }
        }
    };
    encode(&mut uri, &path.to_string_lossy(), true);
    if !query.is_empty() {
        uri.push('?');
        encode(&mut uri, query, false);
    }
    if !fragment.is_empty() {
        uri.push('#');
        encode(&mut uri, fragment, false);
    }
    uri
}

pub fn guess_indentation(lines: &[String], default: Indentation) -> Indentation {
    const ALLOWED_TAB_SIZE_GUESSES: [usize; 7] = [2, 4, 6, 8, 3, 5, 7];
    const MAX_ALLOWED_TAB_SIZE_GUESS: usize = 8;
    let (mut lines_with_tabs, mut lines_with_spaces) = (0, 0);
    let mut previous_line: Vec<u16> = Vec::new();
    let mut previous_indentation = 0;
    let mut spaces_diff_count = [0usize; 9];
    for line in lines.iter().take(10000) {
        let line: Vec<u16> = line.encode_utf16().collect();
        let Some(indentation) = line.iter().position(|&c| c != u16::from(b' ') && c != u16::from(b'\t')) else { continue };
        let tabs = line[..indentation].iter().filter(|&&c| c == u16::from(b'\t')).count();
        if tabs > 0 {
            lines_with_tabs += 1;
        } else if indentation > 1 {
            lines_with_spaces += 1;
        }
        let diff = spaces_diff(&previous_line, previous_indentation, &line, indentation);
        if diff.looks_like_alignment && !(default.insert_spaces && default.tab_size == diff.spaces) {
            continue;
        }
        if diff.spaces <= MAX_ALLOWED_TAB_SIZE_GUESS {
            spaces_diff_count[diff.spaces] += 1;
        }
        previous_line = line;
        previous_indentation = indentation;
    }

    let mut insert_spaces = default.insert_spaces;
    if lines_with_tabs != lines_with_spaces {
        insert_spaces = lines_with_tabs < lines_with_spaces;
    }
    let mut tab_size = default.tab_size;
    if insert_spaces {
        let mut best_score = 0;
        for size in ALLOWED_TAB_SIZE_GUESSES {
            if spaces_diff_count[size] > best_score {
                best_score = spaces_diff_count[size];
                tab_size = size;
            }
        }
        // Indented 4 most often but 2 nearly as often: the file is probably indented by 2 with nested blocks.
        if tab_size == 4 && spaces_diff_count[4] > 0 && spaces_diff_count[2] > 0 && spaces_diff_count[2] * 3 >= spaces_diff_count[4] * 2 {
            tab_size = 2;
        }
    }
    Indentation { tab_size, indent_size: None, insert_spaces }
}

struct SpacesDiff {
    spaces: usize,
    looks_like_alignment: bool,
}

/// `spacesDiff`: how much the indentation of `b` differs from `a`, in spaces, where it is unambiguous.
fn spaces_diff(a: &[u16], a_length: usize, b: &[u16], b_length: usize) -> SpacesDiff {
    let none = SpacesDiff { spaces: 0, looks_like_alignment: false };
    let space = u16::from(b' ');
    let common = a[..a_length].iter().zip(&b[..b_length]).take_while(|(x, y)| x == y).count();
    let count = |s: &[u16]| (s.iter().filter(|&&c| c == space).count(), s.iter().filter(|&&c| c != space).count());
    let (a_spaces, a_tabs) = count(&a[common..a_length]);
    let (b_spaces, b_tabs) = count(&b[common..b_length]);
    if (a_spaces > 0 && a_tabs > 0) || (b_spaces > 0 && b_tabs > 0) {
        return none;
    }
    let tabs_diff = a_tabs.abs_diff(b_tabs);
    let spaces = a_spaces.abs_diff(b_spaces);
    if tabs_diff == 0 {
        // `b` lines up with a space in `a` after a line ending in a comma: alignment, not indentation.
        let looks_like_alignment = spaces > 0
            && b_spaces >= 1
            && b_spaces - 1 < a.len()
            && b_spaces < b.len()
            && b[b_spaces] != space
            && a[b_spaces - 1] == space
            && a.last() == Some(&u16::from(b','));
        return SpacesDiff { spaces, looks_like_alignment };
    }
    if spaces % tabs_diff == 0 {
        return SpacesDiff { spaces: spaces / tabs_diff, looks_like_alignment: false };
    }
    none
}

/// The indent level of each line, as `getLinesIndentGuides` (guidesTextModelPart.ts) computes it:
/// a blank line takes its level from the lines with content around it.
pub fn indent_guides(lines: &[String], tab_size: usize, indent_size: usize, off_side: bool) -> Vec<usize> {
    let indents: Vec<Option<usize>> = lines.iter().map(|line| indent_level(line, tab_size)).collect();
    let mut levels = Vec::with_capacity(lines.len());
    for (index, indent) in indents.iter().enumerate() {
        if let Some(indent) = indent {
            levels.push(indent.div_ceil(indent_size));
            continue;
        }
        let above = indents[..index].iter().rev().find_map(|indent| *indent);
        let below = indents[index + 1..].iter().find_map(|indent| *indent);
        let level = match (above, below) {
            (Some(above), Some(below)) if above < below => 1 + above / indent_size,
            (Some(above), Some(below)) if above == below => below.div_ceil(indent_size),
            (Some(_), Some(below)) if off_side => below.div_ceil(indent_size),
            (Some(_), Some(below)) => 1 + below / indent_size,
            // At the top or bottom of the file.
            _ => 0,
        };
        levels.push(level);
    }
    levels
}

/// `computeIndentLevel`: the indent width in columns, or `None` for a line of only whitespace.
fn indent_level(line: &str, tab_size: usize) -> Option<usize> {
    let mut indent = 0;
    for c in line.chars() {
        match c {
            ' ' => indent += 1,
            '\t' => indent = indent - indent % tab_size + tab_size,
            _ => return Some(indent),
        }
    }
    None
}

/// `USUAL_WORD_SEPARATORS` (wordHelper.ts); whitespace separates words too.
const WORD_SEPARATORS: &str = "`~!@#$%^&*()-=+[{]}\\|;:'\",.<>/?";

fn is_word_char(c: char) -> bool {
    !c.is_whitespace() && !WORD_SEPARATORS.contains(c)
}

/// The word at or touching a cursor before char `column`, as `getWordAtPosition` finds it with the default word pattern.
/// shortcut: the pattern's number branch (`-?\d*\.\d\w*`) is not ported, so "3.14" is two words; add it when a language needs it.
pub fn word_at(line: &str, column: usize) -> Option<Range<usize>> {
    let chars: Vec<char> = line.chars().collect();
    let column = column.min(chars.len());
    let start = column - chars[..column].iter().rev().take_while(|&&c| is_word_char(c)).count();
    let end = column + chars[column..].iter().take_while(|&&c| is_word_char(c)).count();
    (start < end).then_some(start..end)
}

/// Whole-word, case-sensitive occurrences of `word` in char ranges, as `TextualDocumentHighlightProvider` finds them.
pub fn word_occurrences(lines: &[String], word: &str) -> Vec<(usize, Range<usize>)> {
    let word: Vec<char> = word.chars().collect();
    let mut found = Vec::new();
    for (line_index, line) in lines.iter().enumerate() {
        let chars: Vec<char> = line.chars().collect();
        let mut start = 0;
        while start + word.len() <= chars.len() {
            let end = start + word.len();
            let bounded = (start == 0 || !is_word_char(chars[start - 1])) && (end == chars.len() || !is_word_char(chars[end]));
            if bounded && chars[start..end] == word[..] {
                found.push((line_index, start..end));
                start = end;
            } else {
                start += 1;
            }
        }
    }
    found
}

/// viewLineRenderer.ts: a line longer than `stopRenderingLineAfter` chars draws that many, then a "Show more"
/// span. The char limit, its byte offset and the span's label.
pub fn line_overflow(text: &str, limit: Option<usize>) -> Option<(usize, usize, String)> {
    let limit = limit.filter(|limit| text.len() > *limit)?;
    let (byte, _) = text.char_indices().nth(limit)?;
    Some((limit, byte, format!("Show more ({})", overflowing_char_count(text[byte..].chars().count()))))
}

/// viewLines.css .mtkoverflow: the label in the editor font, 4px padding and a 1px border on both sides.
pub fn show_more_width(label: &str, char_width: f32) -> f32 {
    label.chars().count() as f32 * char_width + 10.
}

/// viewLineRenderer.ts `renderOverflowingCharCount`.
fn overflowing_char_count(count: usize) -> String {
    match count {
        0..1024 => format!("{count} chars"),
        1024..1_048_576 => format!("{:.1} KB", count as f64 / 1024.),
        _ => format!("{:.1} MB", count as f64 / 1024. / 1024.),
    }
}

/// The drawn column of char `column`: tabs expanded to the next tab stop, a bidi format character 8 wide.
pub fn visible_column(line: &str, column: usize, tab_size: usize) -> usize {
    line.chars().take(column).fold(0, |visible, c| visible + char_columns(c, visible, tab_size))
}

/// `CursorColumns.visibleColumnFromColumn`: tabs expanded, every other char one column, for cursor moves and Col.
pub fn logical_visible_column(line: &str, column: usize, tab_size: usize) -> usize {
    line.chars().take(column).fold(0, |visible, c| if c == '\t' { visible + tab_size - visible % tab_size } else { visible + 1 })
}

/// viewLineRenderer.ts `isControlCharacter` past C0 and DEL: the bidi format characters, drawn as `[U+XXXX]`.
pub fn is_bidi_format_character(c: char) -> bool {
    matches!(c as u32, 0x202A..=0x202E | 0x2066..=0x2069 | 0x200E..=0x200F | 0x061C)
}

/// The columns char `c` takes when drawn at visible column `visible` (`producedCharacters` of `_renderLine`).
pub fn char_columns(c: char, visible: usize, tab_size: usize) -> usize {
    match c {
        '\t' => tab_size - visible % tab_size,
        c if is_bidi_format_character(c) => 8,
        _ => 1,
    }
}

/// The char index nearest to visible column `target`, for a click on the text.
pub fn column_at_visible(line: &str, target: f32, tab_size: usize) -> usize {
    let mut visible = 0;
    for (index, c) in line.chars().enumerate() {
        let width = char_columns(c, visible, tab_size);
        if target < visible as f32 + width as f32 / 2. {
            return index;
        }
        visible += width;
    }
    line.chars().count()
}

/// textModel.ts `LARGE_FILE_SIZE_THRESHOLD` (UTF-16 units) and `LARGE_FILE_LINE_COUNT_THRESHOLD`, with
/// `editor.largeFileOptimizations` on.
fn too_large_for_tokenization(text: &str, line_count: usize, settings: &Settings, path: &Path, language_id: &str) -> bool {
    const LARGE_FILE_SIZE_THRESHOLD: usize = 20 * 1024 * 1024;
    const LARGE_FILE_LINE_COUNT_THRESHOLD: usize = 300 * 1000;
    let overrides = Overrides { resource: Some(path), override_identifier: Some(language_id) };
    if settings.get_value(Some("editor.largeFileOptimizations"), &overrides) == Some(serde_json::Value::Bool(false)) {
        return false;
    }
    // A UTF-16 length is at most the UTF-8 one, so only a long text needs counting.
    line_count > LARGE_FILE_LINE_COUNT_THRESHOLD || (text.len() > LARGE_FILE_SIZE_THRESHOLD && text.encode_utf16().count() > LARGE_FILE_SIZE_THRESHOLD)
}

/// The line as drawn: tabs become spaces up to the next tab stop, control characters a space under their picture;
/// the whitespace marks and control pictures are drawn apart (`editor::whitespace`, `workbench::control_characters`).
pub fn expand_tabs(line: &str, tab_size: usize) -> String {
    let mut out = String::with_capacity(line.len());
    let mut visible = 0;
    for c in line.chars() {
        if c == '\t' {
            let width = tab_size - visible % tab_size;
            out.extend(std::iter::repeat_n(' ', width));
            visible += width;
        } else if c.is_ascii_control() {
            out.push(' ');
            visible += 1;
        } else if is_bidi_format_character(c) {
            out.push_str("        ");
            visible += 8;
        } else {
            out.push(c);
            visible += 1;
        }
    }
    out
}

/// The text model (textModel.ts) with what lives on it rather than on an editor: its version, its undo stack,
/// its indent guides, its marker decorations. Every editor of the resource shares it.
pub struct TextModel {
    pub document: TextDocument,
    /// `getVersionId`: 1 when opened, one more per content change.
    pub version_id: u32,
    pub(crate) history: History,
    /// The indent guide level of each line.
    pub guides: Vec<usize>,
    /// When the extension host last opened this model (`$tryOpenDocument`): it stays open without an editor
    /// for `BoundModelReferenceCollection`'s 3 minutes after that.
    pub host_reference: Option<Instant>,
    /// An untitled model the host opened for a path (`hasAssociatedFilePath`): dirty from the start, saved to that file.
    pub associated_file: bool,
    /// The decorations of the model's problem markers.
    pub markers: Vec<marker_decorations::MarkerDecoration>,
    /// More markers than `MAX_MARKER_DECORATIONS` (`_isMarkerLimitExceeded`): only that many are decorated.
    pub markers_truncated: bool,
    /// Each replace as (version it applied to, start, end, new end), for the model's other editors to follow.
    pub(crate) edits: Vec<(u32, Position, Position, Position)>,
}

/// One model shared by the editors of its resource in every group.
pub type SharedModel = Rc<RefCell<TextModel>>;

/// An editor of a text model: the model handle and the editor's own view state.
pub struct OpenEditor {
    pub model: SharedModel,
    /// A preview editor shows in italics and is replaced by the next preview open (`workbench.editor.enablePreview`).
    pub preview: bool,
    /// The `$model<N>` id the text model gets, so the extension host sees a new editor per opened model.
    pub model_id: u32,
    /// The cursors' selections, the primary one first (`CursorCollection`).
    pub selections: Vec<Selection>,
    /// The cursor added last, which wins a merge of its direction (`lastAddedCursorIndex`).
    last_added: usize,
    /// Per cursor, visible columns a vertical move could not place at the anchor and the head, kept while
    /// moving up and down (`selectionStartLeftoverVisibleColumns`, `leftoverVisibleColumns`).
    leftovers: Vec<(isize, isize)>,
    /// The last column selection and the selections it made: it goes on while they are unchanged (`_columnSelectData`).
    column_select: Option<(ColumnSelectData, Vec<Selection>)>,
    /// The ranges of the last Expand or Shrink Selection and the selections it made: kept while they are unchanged.
    smart_select: Option<(Vec<smart_select::SelectionRanges>, Vec<Selection>)>,
    cursor_states: cursor_undo::CursorStates,
    /// Occurrences of the word at the cursor, by line and char range.
    pub highlights: Vec<(usize, Range<usize>)>,
    /// inPlaceReplace.ts `valueSetReplacement`: the replaced value's line and char range, under the version that made it.
    pub value_set_replacement: Option<(u32, usize, Range<usize>)>,
    /// anchorSelect.ts: the selection anchor, while the group's code editor shows this model.
    pub selection_anchor: Option<Position>,
    /// The folding regions, their collapse state and the lines it hides.
    pub folding: FoldingModel,
    /// Extension `FoldingRangeProvider`s fold the model (`SyntaxRangeProvider`), not its indentation.
    pub syntax_folding: bool,
    /// folding.ts `_currentModelHasFoldedImports`: `editor.foldingImportsByDefault` already collapsed the imports.
    pub folded_imports: bool,
    /// `editor.foldingMaximumRegions` of the indentation provider, and the limit its last regions were cut to.
    pub folding_limit: usize,
    pub folding_limited: Option<usize>,
    /// The sticky scroll scopes of the indentation and marker regions, without the folding model's manual ones.
    pub sticky_model: sticky_scroll::StickyElement,
    /// The scopes of the document symbols and their provider id, used before `sticky_model` (`StickyModelProvider`
    /// with `editor.stickyScroll.defaultModel` `outlineModel`); `None` without symbols.
    pub sticky_outline: Option<(sticky_scroll::StickyElement, Option<String>)>,
    /// The editor's scroll position in pixels, kept with the editor like its view state.
    pub scroll: Scroll,
    /// The widest line drawn so far, in pixels: the content width grows with it and never shrinks.
    max_line_width: f32,
    pub view_zones: Vec<view_zones::ViewZone>,
    /// The extension decorations `$trySetDecorations` set on the editor showing this model.
    pub text_decorations: Vec<decoration_types::TextDecoration>,
    /// The model version this editor's cursors and folding regions were last placed in (`sync_with_model`).
    seen_version: u32,
    /// The snippet session of snippetController2.ts, its placeholders tracked through edits.
    pub snippet_session: Option<snippet_session::SnippetSession>,
    pub word_wrap: word_wrap::WordWrap,
    /// The text this editor injects between the model's chars (inlay hints, color boxes), by position.
    pub injected_text: Vec<injected_text::InjectedText>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Scroll {
    pub top: f32,
    pub left: f32,
}

/// `BoundModelReferenceCollection` maxAge.
pub const HOST_REFERENCE_AGE: Duration = Duration::from_secs(3 * 60);

impl OpenEditor {
    fn new(document: TextDocument, preview: bool, model_id: u32) -> Self {
        let indentation = document.indentation;
        let guides = indent_guides(&document.lines, indentation.tab_size, indentation.indent_size(), document.off_side);
        let model = TextModel { document, version_id: 1, history: History::default(), guides, host_reference: None, associated_file: false, markers: Vec::new(), markers_truncated: false, edits: Vec::new() };
        Self::of_model(Rc::new(RefCell::new(model)), preview, model_id)
    }

    /// An editor of a model that is open already.
    pub fn of_model(model: SharedModel, preview: bool, model_id: u32) -> Self {
        let (seen_version, word_wrap) = {
            let model = model.borrow();
            (model.version_id, word_wrap::WordWrap::new(&model.document.lines, model.document.too_large_for_tokenization))
        };
        let mut editor = OpenEditor { seen_version, model, preview, model_id, selections: vec![Selection::default()], last_added: 0, leftovers: Vec::new(), column_select: None, smart_select: None, cursor_states: Default::default(), highlights: Vec::new(), value_set_replacement: None, selection_anchor: None, folding: FoldingModel::default(), syntax_folding: false, folded_imports: false, sticky_model: Default::default(), folding_limit: folding::MAX_FOLDING_REGIONS_FOR_INDENT, folding_limited: None, sticky_outline: None, scroll: Scroll::default(), max_line_width: 0., view_zones: Vec::new(), snippet_session: None, word_wrap, text_decorations: Vec::new(), injected_text: Vec::new() };
        editor.refresh_highlights();
        editor.refresh_folding_ranges();
        editor
    }

    pub fn document(&self) -> Ref<'_, TextDocument> {
        Ref::map(self.model.borrow(), |model| &model.document)
    }

    pub fn document_mut(&mut self) -> RefMut<'_, TextDocument> {
        RefMut::map(self.model.borrow_mut(), |model| &mut model.document)
    }

    pub fn text_model(&self) -> Ref<'_, TextModel> {
        self.model.borrow()
    }

    pub fn text_model_mut(&mut self) -> RefMut<'_, TextModel> {
        self.model.borrow_mut()
    }

    /// `TextModel.setLanguage` and `ModelService._onDidChangeLanguage`: the language's grammar and rules, and its
    /// indentation options when they differ from the old language's (`_setModelOptionsForModel`).
    pub fn set_language(&mut self, language_id: &str, languages: &Languages, settings: &Settings) {
        let indentation = {
            let mut document = self.document_mut();
            let old = creation_options(&settings.configuration, &document.path, &document.language_id);
            let new = creation_options(&settings.configuration, &document.path, language_id);
            document.language_id = language_id.to_string();
            document.off_side = languages.off_side(language_id);
            document.folding_markers = languages.folding_markers(language_id);
            document.block_comment = languages.block_comment(language_id).map(|(start, end)| (start.to_string(), end.to_string()));
            document.tokens = Tokenization::new(languages.grammars.grammar(language_id).filter(|_| !document.too_large_for_tokenization));
            match (old == new, new.detect_indentation) {
                (true, _) => document.indentation,
                (false, true) => guess_indentation(&document.lines, Indentation { tab_size: new.tab_size, indent_size: None, insert_spaces: new.insert_spaces }),
                (false, false) => Indentation { tab_size: new.tab_size, indent_size: new.indent_size, insert_spaces: new.insert_spaces },
            }
        };
        self.set_indentation(indentation);
        self.refresh_folding_ranges();
    }
}

/// An editor group other than the active one (editorGroupView.ts): its editors, their MRU and the size of its
/// code editor. `EditorGroup::activate_group` swaps it with the active group's fields.
pub struct GroupState {
    pub id: u32,
    pub editors: Vec<OpenEditor>,
    recent: Vec<u32>,
    pub viewport: Viewport,
    unicode_banner_closed: bool,
}

impl GroupState {
    pub fn active_index(&self) -> Option<usize> {
        let id = *self.recent.first()?;
        self.editors.iter().position(|editor| editor.model_id == id)
    }
}

/// The group of the panel's code editor (the Output view's): kept with the editor groups so it edits through
/// the same code, but never in the grid nor a view column.
pub const PANEL_GROUP: u32 = u32::MAX;
/// The modal editor part's one group (modalEditorPart.ts), outside the grid like the panel's.
pub const MODAL_GROUP: u32 = u32::MAX - 1;

/// The editor part's groups. The active group's editors and state are the fields below; the other groups wait
/// in `other_groups`. Text models are shared by the editors of their resource in every group, so `models`,
/// `diffs`, the disposed models and the view states are the part's.
#[derive(Default)]
pub struct EditorGroup {
    /// The active group's id (`GroupIdentifier`); the first group is 0.
    pub id: u32,
    pub other_groups: Vec<GroupState>,
    /// The other groups' ids, most recently active first (`mostRecentActiveGroups` without the active one).
    group_mru: Vec<u32>,
    next_group_id: u32,
    /// The groups in grid order (`GroupsOrder.GRID_APPEARANCE`), which the host numbers as view columns; the
    /// workbench sets it, empty while the part has one group.
    pub columns: Vec<u32>,
    /// The panel's code editor shows (the Output view is visible): the host sees it as a visible text editor.
    pub panel_editor_shown: bool,
    /// Closed file editors as (group, path, tab index), for the workbench's Reopen Closed Editor.
    pub closed: Vec<(u32, PathBuf, usize)>,
    /// The active group's last editor closed (`closeEmptyGroups` closes the group then).
    pub closed_last_editor: bool,
    pub editors: Vec<OpenEditor>,
    /// Text models open without an editor: documents the extension host opened, and closed editors it still holds.
    pub models: Vec<OpenEditor>,
    /// The active group's model ids, most recently active first: closing the active editor activates the next one
    /// (`focusRecentEditorAfterClose`).
    recent: Vec<u32>,
    next_model_id: u32,
    /// The size of the group's one code editor, as last laid out; every editor of the group shows in it.
    pub viewport: Viewport,
    /// A line to center once the code editor has its first size: a diff opened before any editor showed (`revealFirstDiff`).
    pub reveal_after_layout: Option<Position>,
    /// unicodeHighlighter.ts `_bannerClosed`: the banner was closed for the model the code editor shows.
    pub unicode_banner_closed: bool,
    /// The diff editors among `editors`, by their entry's model id: each shows two models found with `model`.
    pub diffs: HashMap<u32, DiffInput>,
    /// The multi diff editors among `editors`, by their entry's model id.
    pub multi_diffs: HashMap<u32, MultiDiffInput>,
    /// The merge editors among `editors`, by their entry's model id: each edits its result model.
    pub merges: HashMap<u32, crate::merge_editor::MergeInput>,
    /// `$registerTextEditorDecorationType` render options by decoration type key.
    pub decoration_types: HashMap<String, serde_json::Value>,
    pub disposed: model_service::DisposedModels,
    pub view_states: view_state::ViewStates,
}

impl EditorGroup {
    /// The model the code editor edits: the active editor's, or the modified side of the active diff editor.
    pub fn active(&self) -> Option<&OpenEditor> {
        let tab = self.active_tab()?;
        match self.edited_side(tab.model_id) {
            Some(side) => self.model(&side.scheme, &side.path),
            None => Some(tab),
        }
    }

    /// The side a diff editor (its modified one) or a merge editor (its result) edits in the code editor.
    fn edited_side(&self, id: u32) -> Option<crate::diff_editor::Side> {
        self.diffs.get(&id).map(|diff| diff.modified.clone()).or_else(|| self.merges.get(&id).map(|merge| merge.result.clone()))
    }

    pub fn active_mut(&mut self) -> Option<&mut OpenEditor> {
        let id = *self.recent.first()?;
        match self.edited_side(id) {
            Some(modified) => self.model_mut(&modified.scheme, &modified.path),
            None => self.editors.iter_mut().find(|editor| editor.model_id == id),
        }
    }

    /// The active tab's entry, a diff editor's too.
    pub fn active_tab(&self) -> Option<&OpenEditor> {
        let id = *self.recent.first()?;
        self.editors.iter().find(|editor| editor.model_id == id)
    }

    /// The diff editor of the active tab, if it is one.
    pub fn active_diff(&self) -> Option<&DiffInput> {
        self.diffs.get(self.recent.first()?)
    }

    /// The merge editor of the active tab, if it is one.
    pub fn active_merge(&self) -> Option<&crate::merge_editor::MergeInput> {
        self.merges.get(self.recent.first()?)
    }

    pub fn active_merge_mut(&mut self) -> Option<&mut crate::merge_editor::MergeInput> {
        self.merges.get_mut(self.recent.first()?)
    }

    /// Opens a merge editor as `open_diff` opens a diff editor.
    pub fn open_merge(&mut self, entry: TextDocument, input: crate::merge_editor::MergeInput, preview: bool) {
        self.next_model_id += 1;
        self.merges.insert(self.next_model_id, input);
        let editor = OpenEditor::new(entry, preview, self.next_model_id);
        self.insert(editor, preview);
    }

    /// The multi diff editor of the active tab, if it is one.
    pub fn active_multi_diff(&self) -> Option<&MultiDiffInput> {
        self.multi_diffs.get(self.recent.first()?)
    }

    /// The model a tab edits: its own, or a diff editor's modified side.
    pub fn tab_model(&self, index: usize) -> &OpenEditor {
        let tab = &self.editors[index];
        match self.edited_side(tab.model_id) {
            Some(side) => self.model(&side.scheme, &side.path).expect("a diff or merge editor's models stay open"),
            None => tab,
        }
    }

    /// Opens a diff editor of two open models as `open` opens an editor; `entry` is its text-less tab document.
    pub fn open_diff(&mut self, entry: TextDocument, input: DiffInput, preview: bool) {
        self.next_model_id += 1;
        self.diffs.insert(self.next_model_id, input);
        let editor = OpenEditor::new(entry, preview, self.next_model_id);
        self.insert(editor, preview);
    }

    /// Opens a multi diff editor as `open_diff` opens a diff editor.
    pub fn open_multi_diff(&mut self, entry: TextDocument, input: MultiDiffInput, preview: bool) {
        self.next_model_id += 1;
        self.multi_diffs.insert(self.next_model_id, input);
        let editor = OpenEditor::new(entry, preview, self.next_model_id);
        self.insert(editor, preview);
    }

    pub fn active_index(&self) -> Option<usize> {
        let id = *self.recent.first()?;
        self.editors.iter().position(|editor| editor.model_id == id)
    }

    /// The open editor of `path`, if any.
    pub fn find(&self, path: &Path) -> Option<usize> {
        self.editors.iter().position(|editor| editor.document().is_file(path))
    }

    /// Opens a document right of the active editor, or in place of the preview editor when this open is a preview too.
    pub fn open(&mut self, document: TextDocument, preview: bool) {
        let editor = self.create_model(document, preview);
        self.insert(editor, preview);
    }

    /// Shows `document` in the editor at `index`, in place of the model it showed, and activates it.
    pub fn replace(&mut self, index: usize, document: TextDocument) {
        let mut editor = self.create_model(document, false);
        self.restore_view_state(&mut editor);
        let replaced = std::mem::replace(&mut self.editors[index], editor);
        self.recent.retain(|id| *id != replaced.model_id);
        self.remove_input(&replaced.model_id);
        self.keep_or_dispose(replaced);
        self.activate(index);
    }

    /// Creates a text model with no editor, as `$tryOpenDocument` does; returns its index in `models`.
    pub fn open_model(&mut self, document: TextDocument) -> usize {
        let model = self.create_model(document, false);
        self.models.push(model);
        self.models.len() - 1
    }

    /// `ModelService.createModel`, with the undo stack a closed model of the same text left; an editor of the
    /// model another group shows when there is one.
    fn create_model(&mut self, document: TextDocument, preview: bool) -> OpenEditor {
        self.next_model_id += 1;
        if let Some(open) = self.all_editors().find(|editor| editor.document().is(&document.scheme, &document.path)) {
            return OpenEditor::of_model(open.model.clone(), preview, self.next_model_id);
        }
        let mut model = OpenEditor::new(document, preview, self.next_model_id);
        self.disposed.restore(&mut model);
        model
    }

    /// A model whose editor went: its file's view state is kept; a model something else references stays, else it
    /// is disposed.
    fn keep_or_dispose(&mut self, model: OpenEditor) {
        self.save_view_state(&model);
        if Rc::strong_count(&model.model) > 1 {
            return;
        }
        if self.is_referenced(&model, Instant::now()) {
            self.models.push(model);
        } else {
            self.disposed.insert(model);
        }
    }

    /// The model of `scheme`/`path`, in an editor of any group or not; the active group's editor first.
    pub fn model(&self, scheme: &str, path: &Path) -> Option<&OpenEditor> {
        let others = self.other_groups.iter().flat_map(|group| &group.editors);
        self.editors.iter().chain(&self.models).chain(others).find(|editor| editor.document().is(scheme, path))
    }

    pub fn model_mut(&mut self, scheme: &str, path: &Path) -> Option<&mut OpenEditor> {
        let others = self.other_groups.iter_mut().flat_map(|group| &mut group.editors);
        self.editors.iter_mut().chain(&mut self.models).chain(others).find(|editor| editor.document().is(scheme, path))
    }

    /// Every editor of every group and every model without an editor.
    pub fn all_editors(&self) -> impl Iterator<Item = &OpenEditor> {
        self.editors.iter().chain(&self.models).chain(self.other_groups.iter().flat_map(|group| &group.editors))
    }

    /// The editor groups, most recently active first; the panel's group is not one.
    pub fn group_ids_mru(&self) -> Vec<u32> {
        std::iter::once(self.id).chain(self.group_mru.iter().copied()).filter(|id| *id != PANEL_GROUP).collect()
    }

    pub fn group(&self, id: u32) -> Option<&GroupState> {
        self.other_groups.iter().find(|group| group.id == id)
    }

    /// A group's editors, the most recently active first.
    pub fn group_editors_by_recency(&self, id: u32) -> Vec<&OpenEditor> {
        let (editors, recent) = match self.group(id) {
            Some(group) => (&group.editors, &group.recent),
            None => (&self.editors, &self.recent),
        };
        recent.iter().filter_map(|model| editors.iter().find(|editor| editor.model_id == *model)).collect()
    }

    pub fn column_ids(&self) -> Vec<u32> {
        if self.columns.is_empty() { vec![self.id] } else { self.columns.clone() }
    }

    /// A group's editors and the index of its active one.
    pub fn group_tabs(&self, id: u32) -> (&[OpenEditor], Option<usize>) {
        match self.group(id) {
            Some(group) => (&group.editors, group.active_index()),
            None => (&self.editors, self.active_index()),
        }
    }

    /// The model a group's code editor edits, as `active` answers for the active group, and its viewport.
    pub fn group_active(&self, id: u32) -> Option<(&OpenEditor, Viewport)> {
        let Some(group) = self.group(id) else { return self.active().map(|editor| (editor, self.viewport)) };
        let tab = &group.editors[group.active_index()?];
        let editor = match self.edited_side(tab.model_id) {
            Some(side) => self.model(&side.scheme, &side.path)?,
            None => tab,
        };
        Some((editor, group.viewport))
    }

    /// Makes group `id` the active one (`activateGroup`); its editor follows what other groups did to its model.
    pub fn activate_group(&mut self, id: u32) {
        if id == self.id {
            return;
        }
        let index = self.other_groups.iter().position(|group| group.id == id).expect("activating an open group");
        let mut group = self.other_groups.remove(index);
        let previous = self.id;
        std::mem::swap(&mut self.id, &mut group.id);
        std::mem::swap(&mut self.editors, &mut group.editors);
        std::mem::swap(&mut self.recent, &mut group.recent);
        std::mem::swap(&mut self.viewport, &mut group.viewport);
        std::mem::swap(&mut self.unicode_banner_closed, &mut group.unicode_banner_closed);
        self.other_groups.push(group);
        self.group_mru.retain(|group| *group != id);
        self.group_mru.insert(0, previous);
        if let Some(editor) = self.active_mut() {
            editor.sync_with_model();
        }
    }

    /// Whether the part has more than one editor group.
    pub fn has_several_groups(&self) -> bool {
        std::iter::once(self.id).chain(self.other_groups.iter().map(|group| group.id)).filter(|id| *id != PANEL_GROUP && *id != MODAL_GROUP).count() > 1
    }

    /// The panel's group, made on first use.
    pub fn ensure_panel_group(&mut self) {
        self.ensure_detached_group(PANEL_GROUP);
    }

    /// A group outside the grid (the panel's or the modal part's), made on first use.
    pub fn ensure_detached_group(&mut self, id: u32) {
        if self.id != id && self.group(id).is_none() {
            self.other_groups.push(GroupState { id, editors: Vec::new(), recent: Vec::new(), viewport: self.viewport, unicode_banner_closed: false });
        }
    }

    /// `addGroup`; with `copy_active` the new group shows the active editor as `splitEditor` copies it: an editor
    /// of the same model with the same view state, pinned. The new group is not activated; answers its id.
    pub fn add_group(&mut self, copy_active: bool) -> u32 {
        self.next_group_id += 1;
        let id = self.next_group_id;
        let mut group = GroupState { id, editors: Vec::new(), recent: Vec::new(), viewport: self.viewport, unicode_banner_closed: false };
        let copied = self.next_model_id + 1;
        if let Some(tab) = self.active_tab().filter(|_| copy_active) {
            let mut copy = OpenEditor::of_model(tab.model.clone(), false, copied);
            copy.restore_view_state(&tab.view_state(self.viewport), self.viewport);
            if let Some(diff) = self.diffs.get(&tab.model_id) {
                let input = DiffInput { inline_now: diff.inline_now, ..DiffInput::new(diff.original.clone(), diff.modified.clone(), diff.label.clone()) };
                self.diffs.insert(copy.model_id, input);
            }
            group.recent.push(copy.model_id);
            group.editors.push(copy);
            self.next_model_id = copied;
        }
        self.other_groups.push(group);
        id
    }

    /// `removeGroup` of a group other than the active one: its editors close.
    pub fn remove_group(&mut self, id: u32) {
        let index = self.other_groups.iter().position(|group| group.id == id).expect("removing an open group");
        let group = self.other_groups.remove(index);
        self.group_mru.retain(|group| *group != id);
        for editor in group.editors {
            self.remove_input(&editor.model_id);
            self.keep_or_dispose(editor);
        }
    }

    /// Takes the active editor at `index` out of the active group to move it to another one (`moveEditor`).
    pub fn take_editor(&mut self, index: usize) -> OpenEditor {
        let editor = self.editors.remove(index);
        let shown = self.recent.first().copied();
        self.recent.retain(|id| *id != editor.model_id);
        self.shown_model_changed(shown);
        editor
    }

    /// Opens a moved editor in the active group after its active editor, or activates the group's editor of the
    /// same resource in its place (`doMoveEditorAcrossGroups`).
    pub fn insert_moved(&mut self, editor: OpenEditor) {
        let existing = self.editors.iter().position(|open| Rc::ptr_eq(&open.model, &editor.model) && !self.diffs.contains_key(&open.model_id));
        match existing.filter(|_| !self.diffs.contains_key(&editor.model_id)) {
            Some(index) => {
                self.activate(index);
                self.remove_input(&editor.model_id);
                self.keep_or_dispose(editor);
            }
            None => self.insert(editor, false),
        }
    }

    /// Every editor's cursors follow the edits other editors of its model made (render calls it each frame).
    pub fn sync_editors(&mut self) {
        let others = self.other_groups.iter_mut().flat_map(|group| &mut group.editors);
        for editor in self.editors.iter_mut().chain(others) {
            editor.sync_with_model();
        }
    }

    /// Shows the model at `index` of `models` in a new editor.
    pub fn show_model(&mut self, index: usize, preview: bool) {
        let mut editor = self.models.remove(index);
        editor.preview = preview;
        self.insert(editor, preview);
    }

    /// Drops the models without an editor that nothing references any more.
    pub fn release_models(&mut self, now: Instant) {
        self.release_models_except(now, |_| false);
    }

    /// `release_models`, keeping the models the workbench `hold`s too (the SCM input boxes').
    pub fn release_models_except(&mut self, now: Instant, hold: impl Fn(&OpenEditor) -> bool) {
        let (kept, released) = std::mem::take(&mut self.models).into_iter().partition(|model| hold(model) || self.is_referenced(model, now));
        self.models = kept;
        for model in released.into_iter().filter(|model| Rc::strong_count(&model.model) == 1) {
            self.disposed.insert(model);
        }
    }

    /// textModelResolverService.ts `destroyReferencedObject`: a diff, multi diff or merge editor's side or an unexpired
    /// host reference holds a model, and a dirty one waits for `canDispose`.
    fn is_referenced(&self, model: &OpenEditor, now: Instant) -> bool {
        let shows = |side: &crate::diff_editor::Side| model.document().is(&side.scheme, &side.path);
        let shown_in_diff = self.diffs.values().any(|diff| shows(&diff.original) || shows(&diff.modified))
            || self.multi_diffs.values().any(|input| input.shows(model))
            || self.merges.values().any(|merge| [&merge.base, &merge.input1.side, &merge.input2.side, &merge.result].into_iter().any(shows));
        model.is_dirty() || shown_in_diff || model.text_model().host_reference.is_some_and(|opened| now.duration_since(opened) < HOST_REFERENCE_AGE)
    }

    /// Forgets the diff, multi diff or merge editor whose entry was `model_id`.
    fn remove_input(&mut self, model_id: &u32) {
        self.diffs.remove(model_id);
        self.multi_diffs.remove(model_id);
        self.merges.remove(model_id);
    }

    fn insert(&mut self, mut editor: OpenEditor, preview: bool) {
        self.restore_view_state(&mut editor);
        let preview_index = self.editors.iter().position(|editor| editor.preview).filter(|_| preview);
        let index = match preview_index {
            Some(index) => {
                let replaced_id = self.editors[index].model_id;
                self.recent.retain(|id| *id != replaced_id);
                let replaced = std::mem::replace(&mut self.editors[index], editor);
                self.remove_input(&replaced.model_id);
                self.keep_or_dispose(replaced);
                index
            }
            None => {
                let index = self.active_index().map_or(self.editors.len(), |active| active + 1);
                self.editors.insert(index, editor);
                index
            }
        };
        self.activate(index);
    }

    pub fn activate(&mut self, index: usize) {
        let id = self.editors[index].model_id;
        let shown = self.recent.first().copied();
        self.recent.retain(|recent| *recent != id);
        self.recent.insert(0, id);
        self.shown_model_changed(shown);
    }

    /// The group's code editor shows another model (`onDidChangeModel`).
    fn shown_model_changed(&mut self, previous: Option<u32>) {
        if self.recent.first().copied() == previous {
            return;
        }
        self.unicode_banner_closed = false;
        // codeEditorWidget.ts `_detachModel` removes the editor's decorations, the anchor too.
        for editor in &mut self.editors {
            editor.selection_anchor = None;
        }
        if let Some(editor) = self.active_mut() {
            editor.clear_cursor_states();
        }
    }

    /// `updateEditorViewState` of a file editor (`tracksEditorViewState`: `FileEditorInput` only).
    pub fn save_view_state(&mut self, editor: &OpenEditor) {
        if editor.document().scheme == "file" {
            self.view_states.save(&editor.document().uri_string(), editor.view_state(self.viewport));
        }
    }

    /// `loadEditorViewState` for an editor new in the group: not with `workbench.editor.restoreViewState` off.
    fn restore_view_state(&mut self, editor: &mut OpenEditor) {
        if editor.document().scheme != "file" || !self.disposed.setting_enabled("workbench.editor.restoreViewState", &editor.document().path) {
            return;
        }
        let uri = editor.document().uri_string();
        if let Some(state) = self.view_states.load(&uri) {
            editor.restore_view_state(&state, self.viewport);
        }
    }

    pub fn pin(&mut self, index: usize) {
        self.editors[index].preview = false;
    }

    /// editorGroupModel.ts `moveEditor` then `pin`, as a move inside the group does; `to` is clamped to the tabs.
    pub fn move_editor(&mut self, from: usize, to: usize) {
        let to = to.min(self.editors.len() - 1);
        if from == to {
            return;
        }
        let editor = self.editors.remove(from);
        self.editors.insert(to, editor);
        self.pin(to);
    }

    pub fn close(&mut self, index: usize) {
        let closed = self.editors.remove(index);
        if closed.document().scheme == "file" && !self.diffs.contains_key(&closed.model_id) {
            self.closed.push((self.id, closed.document().path.clone(), index));
        }
        self.remove_input(&closed.model_id);
        let shown = self.recent.first().copied();
        self.recent.retain(|id| *id != closed.model_id);
        self.shown_model_changed(shown);
        self.keep_or_dispose(closed);
        self.closed_last_editor = self.editors.is_empty();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        split_lines(text).0
    }

    #[test]
    fn long_lines_stop_rendering_after_the_limit() {
        // viewLineRenderer.test.ts 'overflow': "Hello " then "Show more (6 chars)".
        assert_eq!(line_overflow("Hello world!", Some(6)), Some((6, 6, "Show more (6 chars)".to_string())));
        assert_eq!(line_overflow("\u{e9}\u{e9}\u{e9}", Some(2)).map(|(limit, byte, _)| (limit, byte)), Some((2, 4)));
        assert_eq!(line_overflow("abcd", Some(4)), None);
        assert_eq!(line_overflow("\u{e9}\u{e9}", Some(3)), None);
        assert_eq!(line_overflow("abcdef", None), None);
        assert_eq!(overflowing_char_count(1023), "1023 chars");
        assert_eq!(overflowing_char_count(1024), "1.0 KB");
        assert_eq!(overflowing_char_count(1_572_864), "1.5 MB");
    }

    #[test]
    fn opened_files_take_the_configured_indentation() {
        let dir = std::env::temp_dir().join(format!("code-gpui-indentation-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("User")).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, "a\n    b\n        c\n").unwrap();
        let read = |user_settings: &str| {
            std::fs::write(dir.join("User/settings.json"), user_settings).unwrap();
            let settings = Settings::load(config::registry::Registry::core(), dir.join("User"), None);
            TextDocument::read(&file, &Languages::new(&[]), &settings).unwrap().indentation
        };
        assert_eq!(read("{}"), Indentation { tab_size: 4, indent_size: None, insert_spaces: true });
        assert_eq!(read(r#"{ "editor.tabSize": 2, "editor.detectIndentation": false }"#), Indentation { tab_size: 2, indent_size: None, insert_spaces: true });
        assert_eq!(read(r#"{ "editor.insertSpaces": false, "editor.detectIndentation": false }"#), Indentation { tab_size: 4, indent_size: None, insert_spaces: false });
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn splits_lines_and_picks_the_eol_of_most_breaks() {
        assert_eq!(split_lines("a\nb\n"), (vec!["a".into(), "b".into(), "".into()], Some("\n")));
        assert_eq!(split_lines("a\r\nb\r\nc\n"), (vec!["a".into(), "b".into(), "c".into(), "".into()], Some("\r\n")));
        assert_eq!(split_lines("a\rb"), (vec!["a".into(), "b".into()], Some("\r\n")));
        assert_eq!(split_lines("one line"), (vec!["one line".into()], None));
    }

    #[test]
    fn a_text_without_line_breaks_takes_the_languages_files_eol() {
        let dir = std::env::temp_dir().join(format!("code-gpui-eol-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("User")).unwrap();
        std::fs::write(dir.join("User/settings.json"), r#"{ "files.eol": "\r\n", "[shellscript]": { "files.eol": "\n" } }"#).unwrap();
        let settings = Settings::load(config::registry::Registry::core(), dir.join("User"), None);
        let eol = |text: &str, language: &str| TextDocument::untitled(Path::new("Untitled-1"), text, Some(language), &Languages::new(&[]), &settings).eol;
        assert_eq!(eol("", "plaintext"), "\r\n");
        assert_eq!(eol("", "shellscript"), "\n");
        assert_eq!(eol("a\nb", "plaintext"), "\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_non_file_uri_keeps_its_authority() {
        let settings = Settings::load(config::registry::Registry::core(), std::env::temp_dir().join("code-gpui-no-user"), None);
        let uri = |scheme: &str, path: &str| {
            let document = TextDocument::from_bytes(scheme, Path::new(path), b"", None, &Languages::new(&[]), &settings);
            (document.uri()["authority"].clone(), document.uri()["path"].clone(), document.uri_string())
        };
        assert_eq!(uri("foo", "//testing/virtual.js"), (serde_json::json!("testing"), serde_json::json!("/virtual.js"), "foo://testing/virtual.js".to_string()));
        assert_eq!(uri("sc", "//auth"), (serde_json::json!("auth"), serde_json::json!(""), "sc://auth".to_string()));
        assert_eq!(uri("file", "//w/a.ts"), (serde_json::json!(""), serde_json::json!("//w/a.ts"), "file:////w/a.ts".to_string()));
    }

    #[test]
    fn guesses_indentation_like_upstream() {
        let guess = |text: &str| guess_indentation(&lines(text), DEFAULT_INDENTATION);
        let spaces = |tab_size| Indentation { tab_size, indent_size: None, insert_spaces: true };
        let tabs = Indentation { tab_size: 4, indent_size: None, insert_spaces: false };
        assert_eq!(guess("x\n  x\n    x\n  x\n"), spaces(2));
        assert_eq!(guess("x\n    x\n        x\n"), spaces(4));
        assert_eq!(guess("x\n\tx\n\t\tx\n"), tabs);
        assert_eq!(guess("x\n\tx\n    x\n"), spaces(4));
        assert_eq!(guess("x\n   x\n      x\n"), spaces(3));
        assert_eq!(guess("x\n\nno indentation\n"), spaces(4));
        // A line after a comma that lines up with the text above it is alignment, not a 6-space indent.
        assert_eq!(guess("const a = 1,\n      b = 2;\n"), spaces(4));
    }

    #[test]
    fn blank_lines_take_the_indent_guides_of_their_block() {
        let text = lines("a\n\tb\n\n\tc\nd\n\n");
        assert_eq!(indent_guides(&text, 4, 4, false), [0, 1, 1, 1, 0, 0, 0]);
        let python = lines("def f():\n    x\n\ny\n");
        assert_eq!(indent_guides(&python, 4, 4, false), [0, 1, 1, 0, 0]);
        assert_eq!(indent_guides(&python, 4, 4, true), [0, 1, 0, 0, 0]);
    }

    #[test]
    fn finds_words_and_their_whole_word_occurrences() {
        assert_eq!(word_at("Port notes", 0), Some(0..4));
        assert_eq!(word_at("Port notes", 4), Some(0..4));
        assert_eq!(word_at("a, b", 2), None);
        assert_eq!(word_at("foo.bar", 5), Some(4..7));
        let text = lines("Port port Port.\nexport Port");
        assert_eq!(word_occurrences(&text, "Port"), [(0, 0..4), (0, 10..14), (1, 7..11)]);
    }

    #[test]
    fn maps_columns_through_tabs() {
        assert_eq!(expand_tabs("\ta\tb", 4), "    a   b");
        assert_eq!(visible_column("\ta\tb", 3, 4), 8);
        assert_eq!(column_at_visible("\ta", 1.9, 4), 0);
        assert_eq!(column_at_visible("\ta", 2.1, 4), 1);
        assert_eq!(column_at_visible("\ta", 9., 4), 2);
    }

    #[test]
    fn previews_replace_each_other_and_closing_activates_the_recent_editor() {
        let document = |name: &str| TextDocument {
            scheme: "file".into(),
            path: PathBuf::from(name),
            lines: vec![String::new()],
            eol: "\n",
            encoding: "utf8".into(),
            language_id: "plaintext".into(),
            indentation: DEFAULT_INDENTATION,
            off_side: false,
            folding_markers: None,
            block_comment: None,
            tokens: Tokenization::default(),
            too_large_for_tokenization: false,
        };
        let names = |group: &EditorGroup| group.editors.iter().map(|editor| editor.document().name()).collect::<Vec<_>>();
        let mut group = EditorGroup::default();
        group.open(document("a"), false);
        group.open(document("b"), true);
        group.open(document("c"), true);
        assert_eq!(names(&group), ["a", "c"]);
        group.activate(0);
        group.open(document("d"), false);
        assert_eq!(names(&group), ["a", "d", "c"]);
        group.close(1);
        assert_eq!(group.active().unwrap().document().name(), "a");
    }

    #[test]
    fn the_cursor_highlights_its_word_and_clamps_to_the_text() {
        let mut group = EditorGroup::default();
        let document = TextDocument {
            scheme: "file".into(),
            path: PathBuf::from("/w/a.txt"),
            lines: lines("Port notes\nport Port"),
            eol: "\n",
            encoding: "utf8".into(),
            language_id: "plaintext".into(),
            indentation: DEFAULT_INDENTATION,
            off_side: false,
            folding_markers: None,
            block_comment: None,
            tokens: Tokenization::default(),
            too_large_for_tokenization: false,
        };
        group.open(document, false);
        let editor = group.active_mut().unwrap();
        assert_eq!(editor.highlights, [(0, 0..4), (1, 5..9)]);
        editor.move_to(Position::new(0, 4), false);
        assert_eq!(editor.highlights.len(), 2);
        editor.move_to(Position::new(1, 4), false);
        assert_eq!(editor.highlights, [(1, 0..4)]);
        editor.move_to(Position::new(1, 4), true);
        editor.selections[0].anchor = Position::new(0, 0);
        editor.refresh_highlights();
        assert!(editor.highlights.is_empty());
        editor.selections[0] = Selection { anchor: Position::new(1, 0), head: Position::new(1, 4) };
        editor.refresh_highlights();
        assert_eq!(editor.highlights, [(1, 0..4)]);
        editor.selections[0].head = Position::new(1, 5);
        editor.refresh_highlights();
        assert!(editor.highlights.is_empty());
    }
}
