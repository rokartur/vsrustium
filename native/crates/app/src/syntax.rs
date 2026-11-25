//! Token colors in the editor: the theme's `tokenColors` as `ColorThemeData.tokenColors` hands them
//! to vscode-textmate, and the text model's TextMate tokens (textModelTokens.ts), kept per line.

use std::cell::{OnceCell, RefCell};
use std::ops::Range;

use gpui::{FontStyle, FontWeight, HighlightStyle, Hsla, Rgba, StrikethroughStyle, UnderlineStyle, px};
use serde_json::{Map, Value, json};
use textmate::{Grammar, State, Token};

use crate::brackets::{BracketRange, DocumentBrackets, Span};
use crate::languages::Languages;
use crate::theme::{Theme, ThemeKind};

/// `editor.maxTokenizationLineLength`: a longer line gets no tokens and passes the state through.
const MAX_TOKENIZATION_LINE_LENGTH: usize = 20_000;

/// `ColorThemeData.tokenColors`: the editor colors as the default rule, then the theme's rules, then
/// the `token.*-token` defaults unless the theme sets `token.info-token`.
pub fn token_theme(theme: &Theme, rules: &[Value]) -> textmate::Theme {
    let editor_hex = |key: &str| css_hex(theme.color(key).expect("editor colors have registry defaults"));
    let mut result = vec![json!({ "settings": { "foreground": editor_hex("editor.foreground"), "background": editor_hex("editor.background") } })];
    let has_default_tokens = rules.iter().any(|rule| rule["scope"] == "token.info-token" && rule["settings"].is_object());
    let defaults = if has_default_tokens { Vec::new() } else { default_token_colors(theme.kind) };
    for rule in rules.iter().chain(&defaults) {
        // `rule.scope && rule.settings`: an empty string scope is falsy in JS, an empty array is not.
        if rule["scope"].is_null() || rule["scope"] == "" || !rule["settings"].is_object() {
            continue;
        }
        let settings = &rule["settings"];
        let mut normalized = Map::new();
        for key in ["foreground", "background"] {
            if let Some(color) = settings[key].as_str().and_then(normalize_color) {
                normalized.insert(key.into(), color.into());
            }
        }
        if !settings["fontStyle"].is_null() {
            normalized.insert("fontStyle".into(), settings["fontStyle"].clone());
        }
        result.push(json!({ "scope": rule["scope"], "settings": normalized }));
    }
    textmate::Theme::new(&result)
}

fn default_token_colors(kind: ThemeKind) -> Vec<Value> {
    let (info, warn, error, debug) = match kind {
        ThemeKind::Light | ThemeKind::HighContrastLight => ("#316bcd", "#cd9731", "#cd3131", "#800080"),
        ThemeKind::Dark => ("#6796e6", "#cd9731", "#f44747", "#b267e6"),
        ThemeKind::HighContrastDark => ("#6796e6", "#008000", "#FF0000", "#b267e6"),
    };
    [("token.info-token", info), ("token.warn-token", warn), ("token.error-token", error), ("token.debug-token", debug)]
        .into_iter()
        .map(|(scope, foreground)| json!({ "scope": scope, "settings": { "foreground": foreground } }))
        .collect()
}

/// `Color.Format.CSS.formatHexA(color, true)` in upper case, as `normalizeColor` leaves it.
fn css_hex(color: Rgba) -> String {
    let byte = |v: f32| (v * 255.).round() as u8;
    let hex = format!("#{:02X}{:02X}{:02X}", byte(color.r), byte(color.g), byte(color.b));
    if color.a < 1. { format!("{hex}{:02X}", byte(color.a)) } else { hex }
}

/// `normalizeColor` (colorThemeData.ts): `#RGB`, `#RGBA`, `#RRGGBB` or `#RRGGBBAA` in upper case,
/// short forms doubled, an opaque alpha dropped; anything else is no color.
fn normalize_color(color: &str) -> Option<String> {
    let digits = color.strip_prefix('#')?;
    if ![3, 4, 6, 8].contains(&digits.len()) || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let mut result: String = if digits.len() <= 4 { digits.chars().flat_map(|c| [c, c]).collect() } else { digits.to_string() };
    result.make_ascii_uppercase();
    if result.len() == 8 && result.ends_with("FF") {
        result.truncate(6);
    }
    Some(format!("#{result}"))
}

/// The TextMate tokens of a document's lines, tokenized from the top as far as a line is drawn, and
/// the colorized brackets of the whole document.
#[derive(Default)]
pub struct Tokenization {
    grammar: Option<Grammar>,
    /// The tokens of each tokenized line and the state after it; filled while drawing, so behind a RefCell.
    lines: RefCell<Vec<(Vec<Token>, State)>>,
    brackets: OnceCell<DocumentBrackets>,
}

impl Tokenization {
    pub fn new(grammar: Option<Grammar>) -> Self {
        Tokenization { grammar, lines: RefCell::default(), brackets: OnceCell::new() }
    }

    /// The token styles of line `index` over its tab-expanded text, brackets in their pair colors
    /// (`editor.bracketPairColorization.enabled` defaults on); none without a grammar.
    pub fn highlights(&self, theme: &Theme, languages: &Languages, lines: &[String], index: usize, tab_size: usize) -> Vec<(Range<usize>, HighlightStyle)> {
        if self.grammar.is_none() {
            return Vec::new();
        }
        let brackets = self.brackets(theme, languages, lines);
        self.tokenize_to(lines, index);
        let tokenized = self.lines.borrow();
        let highlights = token_highlights(&theme.tokens, &lines[index], &tokenized[index].0, tab_size);
        let expanded_offset = expanded_offsets(&lines[index], tab_size);
        let colors = bracket_colors(theme);
        let unexpected = theme.color("editorBracketHighlight.unexpectedBracket.foreground").map(Hsla::from);
        let mut colored = Vec::new();
        for bracket in &brackets.lines[index] {
            // An undefined color leaves the CSS rule invalid, so the token color stays.
            let color = if bracket.invalid { unexpected } else { colors.get((bracket.level % 30) as usize % colors.len().max(1)).copied() };
            if let Some(color) = color {
                colored.push((expanded_offset[bracket.range.start]..expanded_offset[bracket.range.end], color));
            }
        }
        overlay_colors(highlights, &colored)
    }

    /// The minimap's view of line `index` (`getMinimapLinesRenderingData`): each token's end byte offset with
    /// its foreground. Bracket pair colors are decorations upstream, so the minimap does not show them.
    pub fn minimap_runs(&self, theme: &Theme, lines: &[String], index: usize) -> Vec<(usize, [u8; 3])> {
        let color_map = theme.tokens.color_map();
        let color = |id: usize| {
            let color = Rgba::try_from(color_map[id].as_str()).expect("the token color map holds hex colors");
            [color.r, color.g, color.b].map(|channel| (channel * 255.).round() as u8)
        };
        let text = &lines[index];
        let default_run = vec![(text.len(), color(1))];
        if self.grammar.is_none() {
            return default_run;
        }
        self.tokenize_to(lines, index);
        let tokenized = self.lines.borrow();
        let tokens = &tokenized[index].0;
        if tokens.is_empty() {
            return default_run;
        }
        tokens.iter().map(|token| (token.end.min(text.len()), color(theme.tokens.style(&token.scopes).foreground))).collect()
    }

    /// The byte offsets where `renderViewLine` starts a span on line `index`: a token whose style differs from the
    /// one before it (tokens of equal metadata are one), and each colorized bracket's start and end.
    pub fn span_starts(&self, theme: &Theme, languages: &Languages, lines: &[String], index: usize) -> Vec<usize> {
        let mut starts = vec![0];
        if self.grammar.is_none() {
            return starts;
        }
        let brackets = self.brackets(theme, languages, lines);
        self.tokenize_to(lines, index);
        let tokenized = self.lines.borrow();
        let mut previous = None;
        let mut start = 0;
        for token in &tokenized[index].0 {
            let style = (theme.tokens.style(&token.scopes), token.token_type);
            if previous.is_some_and(|previous| previous != style) {
                starts.push(start);
            }
            previous = Some(style);
            start = token.end;
        }
        for bracket in &brackets.lines[index] {
            starts.extend([bracket.range.start, bracket.range.end]);
        }
        starts.retain(|&start| start < lines[index].len() || start == 0);
        starts.sort_unstable();
        starts.dedup();
        starts
    }

    /// `getLanguageId` of the token at byte `offset` of line `index`: an embedded language, else none.
    pub fn language_at(&self, lines: &[String], index: usize, offset: usize) -> Option<std::rc::Rc<str>> {
        self.grammar.as_ref()?;
        self.tokenize_to(lines, index);
        let tokenized = self.lines.borrow();
        let tokens = &tokenized[index].0;
        tokens.iter().find(|token| token.end > offset).or(tokens.last()).and_then(|token| token.scopes.language.clone())
    }

    /// `getStandardTokenType` of the token at byte `offset` of line `index` (`findTokenIndexAtOffset`).
    pub fn token_type_at(&self, lines: &[String], index: usize, offset: usize) -> textmate::StandardTokenType {
        if self.grammar.is_none() {
            return textmate::StandardTokenType::Other;
        }
        self.tokenize_to(lines, index);
        let tokenized = self.lines.borrow();
        let tokens = &tokenized[index].0;
        tokens.iter().find(|token| token.end > offset).or(tokens.last()).map_or(textmate::StandardTokenType::Other, |token| token.token_type)
    }

    /// The tokens of line `index` as (byte end, standard type, embedded language), or one `Other` token over
    /// the line without a grammar or tokens.
    pub fn line_tokens(&self, lines: &[String], index: usize) -> Vec<(usize, textmate::StandardTokenType, Option<std::rc::Rc<str>>)> {
        let len = lines[index].len();
        if self.grammar.is_none() {
            return vec![(len, textmate::StandardTokenType::Other, None)];
        }
        self.tokenize_to(lines, index);
        let tokens: Vec<_> = self.lines.borrow()[index].0.iter().map(|token| (token.end.min(len), token.token_type, token.scopes.language.clone())).collect();
        if tokens.is_empty() {
            return vec![(len, textmate::StandardTokenType::Other, None)];
        }
        tokens
    }

    /// `findMatchingBracketUp`: the line of the opening bracket that `close` closes in the innermost pair
    /// around (line, byte), its closing bracket typed or not.
    pub fn matching_bracket_up(&self, languages: &Languages, lines: &[String], language: &str, close: &str, line: usize, offset: usize) -> Option<usize> {
        let config = languages.brackets.get(language)?;
        let brackets = self.pairs(languages, lines, language);
        let pair = brackets.pairs.iter().rev().find(|pair| {
            let (open_line, open_range) = &pair.open;
            let starts_before = (*open_line, open_range.start) <= (line, offset);
            let ends_after = pair.close.as_ref().is_none_or(|(close_line, close_range)| (*close_line, close_range.end) >= (line, offset));
            starts_before && ends_after && config.closes(close, &lines[*open_line][open_range.clone()])
        })?;
        Some(pair.open.0)
    }

    /// The bracket pairs of the document in `language`, also without a grammar.
    // shortcut: pairs the whole document per call, where upstream queries its incremental bracket tree.
    pub fn pairs(&self, languages: &Languages, lines: &[String], language: &str) -> DocumentBrackets {
        if self.grammar.is_some() {
            return self.document_brackets(&textmate::Theme::new(&[]), languages, lines);
        }
        // nullTokenize: one Other token over each line, in the model's language.
        let style = textmate::Style { font_style: 0, foreground: 0, background: 0 };
        let spans: Vec<_> = lines.iter().map(|text| (text.as_str(), vec![Span { end: text.len(), language, token_type: textmate::StandardTokenType::Other, balanced_brackets: true, style }])).collect();
        DocumentBrackets::new(&spans, &languages.brackets)
    }

    /// `BracketMatchingController` with `editor.matchBrackets` "always": the pair whose bracket touches
    /// the cursor (line, char column), shown in the overview ruler, else the pair around it, not shown there.
    pub fn bracket_match(&self, theme: &Theme, languages: &Languages, lines: &[String], cursor: (usize, usize)) -> Option<([BracketRange; 2], bool)> {
        self.grammar.as_ref()?;
        let brackets = self.brackets(theme, languages, lines);
        let (line, column) = cursor;
        let offset = lines[line].char_indices().nth(column).map_or(lines[line].len(), |(offset, _)| offset);
        let owned = |[open, close]: [&BracketRange; 2]| [open.clone(), close.clone()];
        match brackets.match_bracket(line, offset) {
            Some(pair) => Some((owned(pair), true)),
            None => brackets.enclosing_brackets(line, offset).map(|pair| (owned(pair), false)),
        }
    }

    /// Drops the tokens from `line` on and the bracket pairs, after an edit starting at `line`.
    // shortcut: retokenizes from the edit down and re-pairs all brackets, where upstream updates both
    // incrementally (TokenizationStateStore, BracketPairsTree); upgrade when typing in large files lags.
    pub fn invalidate_from(&mut self, line: usize) {
        self.lines.get_mut().truncate(line);
        self.brackets = OnceCell::new();
    }

    /// The bracket pairs hold token styles of the theme they were paired under.
    pub fn theme_changed(&mut self) {
        self.brackets = OnceCell::new();
    }

    // shortcut: the whole document is tokenized on the first draw, where upstream tokenizes in the
    // background and pairs brackets without tokens meanwhile; upgrade when large files stall the first frame.
    fn brackets(&self, theme: &Theme, languages: &Languages, lines: &[String]) -> &DocumentBrackets {
        self.brackets.get_or_init(|| self.document_brackets(&theme.tokens, languages, lines))
    }

    fn document_brackets(&self, theme: &textmate::Theme, languages: &Languages, lines: &[String]) -> DocumentBrackets {
        if lines.is_empty() {
            return DocumentBrackets::default();
        }
        self.tokenize_to(lines, lines.len() - 1);
        let tokenized = self.lines.borrow();
        let mut spans = Vec::new();
        for (text, (tokens, _)) in lines.iter().zip(tokenized.iter()) {
            let line_spans = tokens
                .iter()
                .map(|token| Span {
                    end: token.end,
                    language: token.scopes.language.as_deref().unwrap_or_default(),
                    token_type: token.token_type,
                    balanced_brackets: token.balanced_brackets,
                    style: theme.style(&token.scopes),
                })
                .collect();
            spans.push((text.as_str(), line_spans));
        }
        DocumentBrackets::new(&spans, &languages.brackets)
    }

    fn tokenize_to(&self, lines: &[String], index: usize) {
        let grammar = self.grammar.as_ref().expect("tokenized with a grammar");
        let mut tokenized = self.lines.borrow_mut();
        while tokenized.len() <= index {
            let line = &lines[tokenized.len()];
            let previous = tokenized.last().map(|(_, state)| state);
            let result = if line.encode_utf16().count() > MAX_TOKENIZATION_LINE_LENGTH {
                None
            } else {
                // A grammar regex Oniguruma cannot run leaves the line without tokens, as nullTokenize does upstream.
                grammar.tokenize_line(line, previous).map_err(|error| eprintln!("tokenizing line {}: {error}", tokenized.len() + 1)).ok()
            };
            let entry = match (result, previous) {
                (Some(entry), _) => entry,
                (None, Some(state)) => (Vec::new(), state.clone()),
                (None, None) => (Vec::new(), grammar.tokenize_line("", None).expect("an empty line tokenizes").1),
            };
            tokenized.push(entry);
        }
    }
}

/// The non-transparent `editorBracketHighlight.foreground1..6`, the colors levels cycle through.
fn bracket_colors(theme: &Theme) -> Vec<Hsla> {
    (1..=6).filter_map(|n| theme.color(&format!("editorBracketHighlight.foreground{n}"))).filter(|color| color.a > 0.).map(Into::into).collect()
}

/// The bracket colors over the token styles: a bracket keeps its token's font style.
fn overlay_colors(highlights: Vec<(Range<usize>, HighlightStyle)>, colors: &[(Range<usize>, Hsla)]) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut result = Vec::new();
    for (range, style) in highlights {
        let mut start = range.start;
        for (bracket, color) in colors.iter().filter(|(bracket, _)| bracket.start < range.end && bracket.end > range.start) {
            let (bracket_start, bracket_end) = (bracket.start.max(range.start), bracket.end.min(range.end));
            if start < bracket_start {
                result.push((start..bracket_start, style));
            }
            let recolored = HighlightStyle {
                color: Some(*color),
                underline: style.underline.map(|underline| UnderlineStyle { color: Some(*color), ..underline }),
                strikethrough: style.strikethrough.map(|strikethrough| StrikethroughStyle { color: Some(*color), ..strikethrough }),
                ..style
            };
            result.push((bracket_start..bracket_end, recolored));
            start = bracket_end;
        }
        if start < range.end {
            result.push((start..range.end, style));
        }
    }
    result
}

/// The `.mtk*` styles of a line's tokens (`generateTokensCSSForColorMap`) over its tab-expanded text:
/// token backgrounds are not drawn, as in the editor.
fn token_highlights(theme: &textmate::Theme, text: &str, tokens: &[Token], tab_size: usize) -> Vec<(Range<usize>, HighlightStyle)> {
    let expanded_offset = expanded_offsets(text, tab_size);
    let color_map = theme.color_map();
    tokens
        .iter()
        .filter(|token| token.start < token.end.min(text.len()))
        .map(|token| {
            let style = theme.style(&token.scopes);
            let range = expanded_offset[token.start]..expanded_offset[token.end.min(text.len())];
            let color = color_map.get(style.foreground).and_then(|hex| Rgba::try_from(hex.as_str()).ok());
            let font_style = style.font_style.max(0);
            let highlight = HighlightStyle {
                color: color.map(Into::into),
                font_style: (font_style & textmate::ITALIC != 0).then_some(FontStyle::Italic),
                font_weight: (font_style & textmate::BOLD != 0).then_some(FontWeight::BOLD),
                underline: (font_style & textmate::UNDERLINE != 0).then(|| UnderlineStyle { thickness: px(1.), color: color.map(Into::into), wavy: false }),
                strikethrough: (font_style & textmate::STRIKETHROUGH != 0).then(|| StrikethroughStyle { thickness: px(1.), color: color.map(Into::into) }),
                ..Default::default()
            };
            (range, highlight)
        })
        .collect()
}

/// For each byte offset of `text` (and its end), the offset in `expand_tabs(text, tab_size)`.
fn expanded_offsets(text: &str, tab_size: usize) -> Vec<usize> {
    let mut offsets = vec![0; text.len() + 1];
    let (mut expanded, mut visible) = (0, 0);
    for (index, c) in text.char_indices() {
        for byte in 0..c.len_utf8() {
            offsets[index + byte] = expanded;
        }
        let width = if c == '\t' { tab_size - visible % tab_size } else { 1 };
        expanded += if c == '\t' { width } else { c.len_utf8() };
        visible += width;
    }
    offsets[text.len()] = expanded;
    offsets
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::rc::Rc;

    use serde_json::Value;
    use textmate::ScopeStack;

    use super::*;
    use crate::languages::Languages;
    use crate::theme::load_theme;

    /// The default themes the colorize results hold, by file name (`getThemeName` in themes.test.contribution.ts).
    const THEMES: [&str; 10] = ["dark_plus", "light_plus", "dark_vs", "light_vs", "hc_black", "dark_modern", "hc_light", "light_modern", "2026-dark", "2026-light"];

    /// `Snapper._tokenize`: per line, consecutive tokens with equal scopes merged.
    fn capture_tokens(languages: &Languages, language: &str, content: &str) -> Vec<(String, Rc<ScopeStack>)> {
        let grammar = languages.grammars.grammar(language).unwrap_or_else(|| panic!("no grammar for {language}"));
        let mut result: Vec<(String, Rc<ScopeStack>)> = Vec::new();
        let mut state = None;
        for line in content.split("\r\n").flat_map(|line| line.split(['\r', '\n'])) {
            let (tokens, next) = grammar.tokenize_line(line, state.as_ref()).unwrap();
            let mut last_scopes: Option<String> = None;
            for token in tokens {
                let text = &line[token.start.min(line.len())..token.end.min(line.len())];
                let scopes = token.scopes.names().join(" ");
                if last_scopes.as_deref() == Some(scopes.as_str()) {
                    result.last_mut().unwrap().0.push_str(text);
                } else {
                    result.push((text.to_string(), token.scopes.clone()));
                    last_scopes = Some(scopes);
                }
            }
            state = Some(next);
        }
        result
    }

    /// Every colorize-tests fixture gives the reference's tokens, scopes and, in each default theme, colors.
    #[test]
    fn colorize_fixtures_match_the_reference() {
        let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let extensions = exthost::scan::scan_builtin_extensions(&app_root.join("extensions")).unwrap();
        let languages = Languages::new(&extensions);
        let defaults = extensions.iter().find(|extension| extension.location.ends_with("theme-defaults")).unwrap();
        let themes: Vec<_> = THEMES
            .iter()
            .map(|name| {
                let contribution = defaults.description["contributes"]["themes"].as_array().unwrap().iter().find(|theme| theme["path"] == format!("./themes/{name}.json")).unwrap();
                let id = contribution["id"].as_str().or(contribution["label"].as_str()).unwrap();
                (*name, load_theme(&extensions, id).unwrap())
            })
            .collect();
        let tests = app_root.join("extensions/vscode-colorize-tests/test");
        let mut fixtures: Vec<PathBuf> = std::fs::read_dir(tests.join("colorize-fixtures")).unwrap().map(|entry| entry.unwrap().path()).collect();
        fixtures.sort();
        let mut failures = Vec::new();
        for fixture in &fixtures {
            let name = fixture.file_name().unwrap().to_string_lossy();
            let results = tests.join("colorize-results").join(format!("{}.json", name.replacen('.', "_", 1)));
            let expected: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(&results).unwrap()).unwrap();
            let content = std::fs::read_to_string(fixture).unwrap();
            let language = languages.language_of(fixture, content.split(['\r', '\n']).next().unwrap());
            let actual = capture_tokens(&languages, language, &content);
            let actual: Vec<_> = actual.into_iter().filter(|(text, _)| !text.is_empty()).collect();
            for index in 0..expected.len().max(actual.len()) {
                let (Some(want), Some((text, scopes))) = (expected.get(index), actual.get(index)) else {
                    failures.push(format!("{name}: {} tokens, the reference has {}", actual.len(), expected.len()));
                    break;
                };
                let names = scopes.names().join(" ");
                if want["c"] != text.as_str() || want["t"] != names.as_str() {
                    failures.push(format!("{name}: token {index}: expected {:?} {:?}, got {text:?} {names:?}", want["c"], want["t"]));
                    break;
                }
                for (theme_name, theme) in &themes {
                    let Some(explanation) = want["r"][theme_name].as_str() else { continue };
                    let want_color = explanation.rsplit(": ").next().unwrap();
                    let color = &theme.tokens.color_map()[theme.tokens.style(scopes).foreground];
                    if want_color != color {
                        failures.push(format!("{name}: token {index} {text:?} in {theme_name}: expected {explanation}, got {color}"));
                    }
                }
            }
        }
        assert!(failures.is_empty(), "{} mismatches over {} fixtures:\n{}", failures.len(), fixtures.len(), failures.join("\n"));
    }

    /// An edit recolors the text: the tokens drawn after it equal a fresh tokenization of the new text.
    #[test]
    fn edits_drop_stale_tokens_and_brackets() {
        let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let extensions = exthost::scan::scan_builtin_extensions(&app_root.join("extensions")).unwrap();
        let languages = Languages::new(&extensions);
        let theme = load_theme(&extensions, "Dark Modern").unwrap();
        let path = std::env::temp_dir().join(format!("code-gpui-tokens-{}.ts", std::process::id()));
        std::fs::write(&path, "let a = (1);\nlet b = 2;\n").unwrap();
        let document = crate::editor::TextDocument::read(&path, &languages, &crate::default_settings().lock().unwrap()).unwrap();
        std::fs::remove_file(&path).unwrap();
        let mut group = crate::editor::EditorGroup::default();
        group.open(document, false);
        let editor = group.active_mut().unwrap();
        let highlights = |editor: &crate::editor::OpenEditor, index| editor.document.tokens.highlights(&theme, &languages, &editor.document.lines, index, 4);
        let before = (highlights(editor, 0), highlights(editor, 1));
        assert!(editor.document.tokens.bracket_match(&theme, &languages, &editor.document.lines, (0, 8)).is_some());

        editor.move_to(crate::editor::Position::new(0, 0), false);
        editor.type_text("/* ").unwrap();
        let fresh = Tokenization::new(languages.grammars.grammar("typescript"));
        let lines = editor.document.lines.clone();
        for index in 0..2 {
            assert_eq!(highlights(editor, index), fresh.highlights(&theme, &languages, &lines, index, 4), "line {index}");
        }
        assert_ne!((highlights(editor, 0), highlights(editor, 1)), before);
        assert!(editor.document.tokens.bracket_match(&theme, &languages, &lines, (0, 11)).is_none());
    }

    #[test]
    fn normalizes_colors_like_color_theme_data() {
        assert_eq!(normalize_color("#abc").as_deref(), Some("#AABBCC"));
        assert_eq!(normalize_color("#abcdefff").as_deref(), Some("#ABCDEF"));
        assert_eq!(normalize_color("#abcdef80").as_deref(), Some("#ABCDEF80"));
        assert_eq!(normalize_color("abcdef"), None);
        assert_eq!(normalize_color("#abcdeg"), None);
        assert_eq!(expanded_offsets("\ta\u{e9}", 4), [0, 4, 5, 5, 7]);
    }
}
