//! A text file opened in the editor and the editor group holding it: the lines as the text model
//! holds them (textModel.ts), and what the editor derives from them for drawing (indentation,
//! indent guides, word highlights).

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use config::indentation::creation_options;
use config::service::Settings;

use crate::languages::Languages;
use crate::syntax::Tokenization;

pub mod commands;
mod cursor;
mod edit;
pub mod folding;
mod folding_controller;
pub mod minimap;
mod multicursor;
pub mod section_headers;
mod snippet;
mod view;

pub use cursor::{Position, Selection};
use folding::{FoldingMarkers, FoldingModel};
pub use edit::{ContentChanged, Copied, CursorEdit, EditReason, History, Replace, char_column, end_of, selections_after, text_between, utf16_column};
pub use multicursor::{ColumnSelectData, Reveal};
pub use snippet::expand as expand_snippet;
pub use view::{HORIZONTAL_SCROLLBAR_HEIGHT, LINE_HEIGHT, Scrollbar, VERTICAL_SCROLLBAR_WIDTH, VerticalReveal, Viewport};

pub struct TextDocument {
    /// The URI scheme: "file", "untitled", or the scheme of an extension's file system provider.
    pub scheme: String,
    /// The file path, or the URI path for other schemes.
    pub path: PathBuf,
    pub lines: Vec<String>,
    pub eol: &'static str,
    /// The `textFileService` encoding id: "utf8" or "utf8bom".
    pub encoding: &'static str,
    pub language_id: String,
    pub indentation: Indentation,
    /// `foldingRules.offSide` of the language, used by the indent guides of blank lines.
    pub off_side: bool,
    /// `foldingRules.markers` of the language.
    pub folding_markers: Option<Arc<FoldingMarkers>>,
    /// `comments.blockComment` start and end tokens of the language.
    pub block_comment: Option<(String, String)>,
    pub tokens: Tokenization,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Indentation {
    pub tab_size: usize,
    pub insert_spaces: bool,
}

/// `editor.tabSize` and `editor.insertSpaces` defaults, for documents built without settings.
#[cfg(test)]
const DEFAULT_INDENTATION: Indentation = Indentation { tab_size: 4, insert_spaces: true };

impl TextDocument {
    pub fn read(path: &Path, languages: &Languages, settings: &Settings) -> std::io::Result<Self> {
        let bytes = std::fs::read(path)?;
        Ok(Self::from_bytes("file", path, &bytes, languages, settings))
    }

    /// A document of any scheme from its bytes, as the text file model decodes them.
    pub fn from_bytes(scheme: &str, path: &Path, bytes: &[u8], languages: &Languages, settings: &Settings) -> Self {
        let (encoding, content) = match bytes.strip_prefix(b"\xEF\xBB\xBF") {
            Some(rest) => ("utf8bom", rest),
            None => ("utf8", &bytes[..]),
        };
        // shortcut: no encoding guessing or binary detection; non-UTF-8 bytes decode as U+FFFD. Port encoding.ts when a file needs it.
        let text = String::from_utf8_lossy(content);
        let (lines, eol) = split_lines(&text);
        let language_id = languages.language_of(path, lines[0].as_str()).to_string();
        let off_side = languages.off_side(&language_id);
        let folding_markers = languages.folding_markers(&language_id);
        let block_comment = languages.block_comment(&language_id).map(|(start, end)| (start.to_string(), end.to_string()));
        let indentation = creation_indentation(&lines, settings, path, &language_id);
        let tokens = Tokenization::new(languages.grammars.grammar(&language_id));
        TextDocument { scheme: scheme.to_string(), path: path.to_path_buf(), lines, eol, encoding, language_id, indentation, off_side, folding_markers, block_comment, tokens }
    }

    pub fn uri_string(&self) -> String {
        uri_string(&self.scheme, &self.path)
    }

    /// `UriComponents` as the extension host revives them.
    pub fn uri(&self) -> serde_json::Value {
        serde_json::json!({ "$mid": 1, "scheme": self.scheme, "authority": "", "path": self.path.to_string_lossy(), "query": "", "fragment": "" })
    }

    pub fn is(&self, scheme: &str, path: &Path) -> bool {
        self.scheme == scheme && self.path == path
    }

    pub fn name(&self) -> String {
        self.path.file_name().unwrap_or_default().to_string_lossy().into_owned()
    }
}

/// The indentation a model starts with (`TextModel.resolveOptions` from `getCreationOptions`): the language's
/// `editor.tabSize` and `editor.insertSpaces`, guessed from the text when `editor.detectIndentation` is on.
fn creation_indentation(lines: &[String], settings: &Settings, path: &Path, language_id: &str) -> Indentation {
    let options = creation_options(&settings.configuration, path, language_id);
    let configured = Indentation { tab_size: options.tab_size, insert_spaces: options.insert_spaces };
    match options.detect_indentation {
        true => guess_indentation(lines, configured),
        false => configured,
    }
}

/// Lines split at `\r\n`, `\r` or `\n`; the EOL is CRLF when most line breaks hold a CR, else LF,
/// and LF for a file with none (PieceTreeTextBufferBuilder `_getEOL`, `files.eol: auto` on macOS).
fn split_lines(text: &str) -> (Vec<String>, &'static str) {
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
    let eol = if with_cr * 2 > breaks { "\r\n" } else { "\n" };
    (lines, eol)
}

/// `guessIndentation` (indentationGuesser.ts), over UTF-16 code units as upstream counts them.
/// `URI.toString()`: `file:///a%20b` for a file, `scheme:/path` for a URI without authority.
pub fn uri_string(scheme: &str, path: &Path) -> String {
    let mut uri = match scheme {
        "file" => String::from("file://"),
        scheme => format!("{scheme}:"),
    };
    for byte in path.to_string_lossy().bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => uri.push(byte as char),
            _ => uri.push_str(&format!("%{byte:02X}")),
        }
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
    Indentation { tab_size, insert_spaces }
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

/// The visible column of char `column`, with tabs expanded to the next tab stop.
pub fn visible_column(line: &str, column: usize, tab_size: usize) -> usize {
    line.chars().take(column).fold(0, |visible, c| if c == '\t' { visible + tab_size - visible % tab_size } else { visible + 1 })
}

/// The char index nearest to visible column `target`, for a click on the text.
pub fn column_at_visible(line: &str, target: f32, tab_size: usize) -> usize {
    let mut visible = 0;
    for (index, c) in line.chars().enumerate() {
        let width = if c == '\t' { tab_size - visible % tab_size } else { 1 };
        if target < visible as f32 + width as f32 / 2. {
            return index;
        }
        visible += width;
    }
    line.chars().count()
}

/// The line as drawn: tabs become spaces up to the next tab stop (`renderWhitespace: selection` draws no glyph for them).
pub fn expand_tabs(line: &str, tab_size: usize) -> String {
    let mut out = String::with_capacity(line.len());
    let mut visible = 0;
    for c in line.chars() {
        if c == '\t' {
            let width = tab_size - visible % tab_size;
            out.extend(std::iter::repeat_n(' ', width));
            visible += width;
        } else {
            out.push(c);
            visible += 1;
        }
    }
    out
}

pub struct OpenEditor {
    pub document: TextDocument,
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
    /// `getVersionId`: 1 when opened, one more per content change.
    pub version_id: u32,
    history: History,
    /// The indent guide level of each line.
    pub guides: Vec<usize>,
    /// Occurrences of the word at the cursor, by line and char range.
    pub highlights: Vec<(usize, Range<usize>)>,
    /// The folding regions, their collapse state and the lines it hides.
    pub folding: FoldingModel,
    /// The editor's scroll position in pixels, kept with the editor like its view state.
    pub scroll: Scroll,
    /// The widest line drawn so far, in pixels: the content width grows with it and never shrinks.
    max_line_width: f32,
    /// When the extension host last opened this model (`$tryOpenDocument`): it stays open without an editor
    /// for `BoundModelReferenceCollection`'s 3 minutes after that.
    pub host_reference: Option<Instant>,
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
        let guides = indent_guides(&document.lines, indentation.tab_size, indentation.tab_size, document.off_side);
        let mut editor = OpenEditor { document, preview, model_id, selections: vec![Selection::default()], last_added: 0, leftovers: Vec::new(), column_select: None, version_id: 1, history: History::default(), guides, highlights: Vec::new(), folding: FoldingModel::default(), scroll: Scroll::default(), max_line_width: 0., host_reference: None };
        editor.refresh_highlights();
        editor.refresh_folding_ranges();
        editor
    }
}

#[derive(Default)]
pub struct EditorGroup {
    pub editors: Vec<OpenEditor>,
    /// Text models open without an editor: documents the extension host opened, and closed editors it still holds.
    pub models: Vec<OpenEditor>,
    /// Model ids, most recently active first: closing the active editor activates the next one (`focusRecentEditorAfterClose`).
    recent: Vec<u32>,
    next_model_id: u32,
    /// The size of the group's one code editor, as last laid out; every editor of the group shows in it.
    pub viewport: Viewport,
}

impl EditorGroup {
    pub fn active(&self) -> Option<&OpenEditor> {
        let id = *self.recent.first()?;
        self.editors.iter().find(|editor| editor.model_id == id)
    }

    pub fn active_mut(&mut self) -> Option<&mut OpenEditor> {
        let id = *self.recent.first()?;
        self.editors.iter_mut().find(|editor| editor.model_id == id)
    }

    pub fn active_index(&self) -> Option<usize> {
        let id = *self.recent.first()?;
        self.editors.iter().position(|editor| editor.model_id == id)
    }

    /// The open editor of `path`, if any.
    pub fn find(&self, path: &Path) -> Option<usize> {
        self.editors.iter().position(|editor| editor.document.is("file", path))
    }

    /// Opens a document right of the active editor, or in place of the preview editor when this open is a preview too.
    pub fn open(&mut self, document: TextDocument, preview: bool) {
        self.next_model_id += 1;
        let editor = OpenEditor::new(document, preview, self.next_model_id);
        self.insert(editor, preview);
    }

    /// Creates a text model with no editor, as `$tryOpenDocument` does; returns its index in `models`.
    pub fn open_model(&mut self, document: TextDocument) -> usize {
        self.next_model_id += 1;
        self.models.push(OpenEditor::new(document, false, self.next_model_id));
        self.models.len() - 1
    }

    /// The model of `scheme`/`path`, in an editor or not.
    pub fn model(&self, scheme: &str, path: &Path) -> Option<&OpenEditor> {
        self.editors.iter().chain(&self.models).find(|editor| editor.document.is(scheme, path))
    }

    pub fn model_mut(&mut self, scheme: &str, path: &Path) -> Option<&mut OpenEditor> {
        self.editors.iter_mut().chain(&mut self.models).find(|editor| editor.document.is(scheme, path))
    }

    /// Shows the model at `index` of `models` in a new editor.
    pub fn show_model(&mut self, index: usize, preview: bool) {
        let mut editor = self.models.remove(index);
        editor.preview = preview;
        self.insert(editor, preview);
    }

    /// Drops the models without an editor whose host reference ran out.
    pub fn release_models(&mut self, now: Instant) {
        self.models.retain(|model| model.host_reference.is_some_and(|opened| now.duration_since(opened) < HOST_REFERENCE_AGE));
    }

    fn insert(&mut self, editor: OpenEditor, preview: bool) {
        let preview_index = self.editors.iter().position(|editor| editor.preview).filter(|_| preview);
        let index = match preview_index {
            Some(index) => {
                let replaced_id = self.editors[index].model_id;
                self.recent.retain(|id| *id != replaced_id);
                let replaced = std::mem::replace(&mut self.editors[index], editor);
                if replaced.host_reference.is_some() {
                    self.models.push(replaced);
                }
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
        self.recent.retain(|recent| *recent != id);
        self.recent.insert(0, id);
    }

    pub fn pin(&mut self, index: usize) {
        self.editors[index].preview = false;
    }

    pub fn close(&mut self, index: usize) {
        let closed = self.editors.remove(index);
        self.recent.retain(|id| *id != closed.model_id);
        if closed.host_reference.is_some() {
            self.models.push(closed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        split_lines(text).0
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
        assert_eq!(read("{}"), Indentation { tab_size: 4, insert_spaces: true });
        assert_eq!(read(r#"{ "editor.tabSize": 2, "editor.detectIndentation": false }"#), Indentation { tab_size: 2, insert_spaces: true });
        assert_eq!(read(r#"{ "editor.insertSpaces": false, "editor.detectIndentation": false }"#), Indentation { tab_size: 4, insert_spaces: false });
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn splits_lines_and_picks_the_eol_of_most_breaks() {
        assert_eq!(split_lines("a\nb\n"), (vec!["a".into(), "b".into(), "".into()], "\n"));
        assert_eq!(split_lines("a\r\nb\r\nc\n"), (vec!["a".into(), "b".into(), "c".into(), "".into()], "\r\n"));
        assert_eq!(split_lines("a\rb"), (vec!["a".into(), "b".into()], "\r\n"));
        assert_eq!(split_lines("one line"), (vec!["one line".into()], "\n"));
    }

    #[test]
    fn guesses_indentation_like_upstream() {
        let guess = |text: &str| guess_indentation(&lines(text), DEFAULT_INDENTATION);
        let spaces = |tab_size| Indentation { tab_size, insert_spaces: true };
        let tabs = Indentation { tab_size: 4, insert_spaces: false };
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
            encoding: "utf8",
            language_id: "plaintext".into(),
            indentation: DEFAULT_INDENTATION,
            off_side: false,
            folding_markers: None,
            block_comment: None,
            tokens: Tokenization::default(),
        };
        let names = |group: &EditorGroup| group.editors.iter().map(|editor| editor.document.name()).collect::<Vec<_>>();
        let mut group = EditorGroup::default();
        group.open(document("a"), false);
        group.open(document("b"), true);
        group.open(document("c"), true);
        assert_eq!(names(&group), ["a", "c"]);
        group.activate(0);
        group.open(document("d"), false);
        assert_eq!(names(&group), ["a", "d", "c"]);
        group.close(1);
        assert_eq!(group.active().unwrap().document.name(), "a");
    }

    #[test]
    fn the_cursor_highlights_its_word_and_clamps_to_the_text() {
        let mut group = EditorGroup::default();
        let document = TextDocument {
            scheme: "file".into(),
            path: PathBuf::from("/w/a.txt"),
            lines: lines("Port notes\nport Port"),
            eol: "\n",
            encoding: "utf8",
            language_id: "plaintext".into(),
            indentation: DEFAULT_INDENTATION,
            off_side: false,
            folding_markers: None,
            block_comment: None,
            tokens: Tokenization::default(),
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
    }
}
