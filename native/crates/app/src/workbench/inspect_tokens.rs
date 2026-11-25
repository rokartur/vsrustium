//! inspectEditorTokens.ts: Developer: Inspect Editor Tokens and Scopes, the `token-inspect-widget` below the
//! cursor with the TextMate token there, its decoded metadata, its scopes and the theme rule that colors it.
// shortcut: semantic tokens are not in the port (milestone 13), so their rows are left out; add them with semantic tokens.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{AnyElement, Bounds, Context, Div, FontWeight, KeyUpEvent, Size, TextRun, Window, canvas, deferred, div, prelude::*, px};
use serde::Serialize;
use serde_json::Value;
use textmate::StandardTokenType;

use super::Workbench;
use super::editor_message::horizontal_in_page;
use crate::commands::Outcome;
use crate::editor::{OpenEditor, Position, TextDocument};
use crate::theme::{Theme, parse_hex, relative_luminance};

const UI_FONT: &str = ".SystemUIFont";
const MONOSPACE_FONT: &str = "Menlo";
/// style.css `.monaco-workbench`: 13px text with a 1.4em line height, which the widget inherits.
const FONT_SIZE: f32 = 13.;
const LINE_HEIGHT: f32 = 18.2;
/// The user agent's `h2`: 1.5em, with 0.83em margins above and below.
const TOKEN_FONT_SIZE: f32 = FONT_SIZE * 1.5;
const TOKEN_MARGIN: f32 = TOKEN_FONT_SIZE * 0.83;
/// The user agent's `hr` margins of 0.5em.
const SEPARATOR_MARGIN: f32 = FONT_SIZE * 0.5;
/// `.tiw-metadata-key`: `min-width: 150px`.
const KEY_MIN_WIDTH: f32 = 150.;
/// contentWidgets.ts `_layoutBoxInPage`: the widget keeps 22px from the window's top and bottom.
const PAGE_PADDING: f32 = 22.;

#[derive(Default)]
pub(super) struct InspectTokens {
    /// The model and language the widget opened on; another one stops it.
    shown: Option<(u32, String)>,
    widget_bounds: Rc<Cell<Option<Bounds<f32>>>>,
}

#[derive(Debug, PartialEq)]
struct TokenInspection {
    text: String,
    length: usize,
    metadata: Vec<(&'static str, String)>,
    /// Innermost first.
    scopes: Vec<String>,
    /// The raw selector and the settings as JSON.
    theme_rule: Option<(String, String)>,
}

impl Workbench {
    pub(super) fn register_inspect_tokens_commands(&mut self) {
        self.commands.register("editor.action.inspectTMScopes", |workbench, _, _, cx| {
            let shown = match (&workbench.inspect_tokens.shown, workbench.editors.active()) {
                (None, Some(editor)) => Some((editor.model_id, editor.document().language_id.clone())),
                _ => None,
            };
            workbench.inspect_tokens.shown = shown;
            workbench.inspect_tokens.widget_bounds.set(None);
            cx.notify();
            Outcome::done()
        });
    }

    /// `onDidChangeModel` and `onDidChangeModelLanguage` stop the inspector.
    pub(super) fn stop_inspect_tokens_on_model_change(&mut self) {
        let Some((model_id, language)) = &self.inspect_tokens.shown else { return };
        if self.editors.active().is_none_or(|editor| editor.model_id != *model_id || editor.document().language_id != *language) {
            self.inspect_tokens.shown = None;
        }
    }

    /// `onKeyUp` of Escape in the editor stops the inspector.
    pub(super) fn inspect_tokens_key_up(&mut self, event: &KeyUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key == "escape" && self.inspect_tokens.shown.take().is_some() {
            cx.notify();
        }
    }

    /// The widget at the primary cursor, recomputed on every render, so on every cursor move and theme change;
    /// below the line (`ContentWidgetPositionPreference.BELOW`), above it when only that fits in the window.
    pub(super) fn inspect_tokens_widget(&self, editor: &OpenEditor, content_left: f32, char_width: f32, window: &Window) -> Option<AnyElement> {
        let (model_id, _) = self.inspect_tokens.shown.as_ref()?;
        let position = editor.selection().head;
        if *model_id != editor.model_id || editor.folding.is_hidden(position.line) {
            return None;
        }
        let view_line = editor.position_view_line(position, false);
        let anchor_left = editor.segment_visible_column(editor.segment(view_line), position.column) * char_width;
        let anchor_top = editor.line_top(view_line, self.editors.viewport) - editor.scroll.top;
        let measured = self.inspect_tokens.widget_bounds.get().map(|bounds| bounds.size);
        let size = measured.unwrap_or(Size { width: 0., height: 0. });
        let editor_bounds = self.text_area_bounds.get();
        let window_size = window.viewport_size().map(f32::from);
        let left = horizontal_in_page(anchor_left - editor.scroll.left + content_left, size.width, editor_bounds, window_size);
        let below_top = anchor_top + self.editors.viewport.line_height;
        let fits_below = editor_bounds.origin.y + below_top + size.height <= window_size.height - PAGE_PADDING;
        let above_top = anchor_top - size.height;
        let fits_above = editor_bounds.origin.y + above_top >= PAGE_PADDING;
        let top = if fits_below || !fits_above { below_top } else { above_top };
        let widget_bounds = self.inspect_tokens.widget_bounds.clone();
        let measure = canvas(
            move |bounds, window, _| {
                let bounds: Bounds<f32> = bounds.map(f32::from);
                if widget_bounds.get().map(|old| old.size) != Some(bounds.size) {
                    window.refresh();
                }
                widget_bounds.set(Some(bounds));
            },
            |_, _, _, _| (),
        )
        .absolute()
        .inset_0();
        let widget = div()
            .id("token-inspect-widget")
            .absolute()
            .left(px(left))
            .top(px(top))
            .max_w(px((editor_bounds.size.width * 0.66).max(500.)))
            .p(px(10.))
            .border_1()
            .border_color(self.color("editorHoverWidget.border"))
            .bg(self.color("editorHoverWidget.background"))
            .text_color(self.color("editor.foreground"))
            .font_family(UI_FONT)
            .text_size(px(FONT_SIZE))
            .line_height(px(LINE_HEIGHT))
            .cursor_default()
            .occlude()
            .when(measured.is_none(), |widget| widget.invisible())
            .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
            .child(measure);
        let widget = match inspect(&editor.document(), position, &self.theme) {
            None => widget.child("No grammar or semantic tokens available."),
            Some(inspection) => widget.children(self.inspection_content(inspection, window)),
        };
        Some(deferred(widget).with_priority(1).into_any_element())
    }

    fn inspection_content(&self, inspection: TokenInspection, window: &Window) -> Vec<AnyElement> {
        let separator = || div().h(px(1.)).bg(self.color("editorHoverWidget.border"));
        let length = format!("{} {}", inspection.length, if inspection.length == 1 { "char" } else { "chars" });
        let token = div()
            .flex()
            .justify_between()
            .items_start()
            .gap(px(4.))
            .my(px(TOKEN_MARGIN))
            .font_family(MONOSPACE_FONT)
            .text_size(px(TOKEN_FONT_SIZE))
            .font_weight(FontWeight::BOLD)
            .child(div().min_w_0().child(inspection.text))
            .child(div().flex_none().text_size(px(TOKEN_FONT_SIZE * 0.6)).font_weight(FontWeight::NORMAL).child(length));
        let metadata: Vec<_> = inspection.metadata.into_iter().map(|(key, value)| (key, div().child(value))).collect();
        // `.tiw-theme-selector` is a `code` element: the browser collapses the JSON's newlines and tabs to spaces.
        let rule = match inspection.theme_rule {
            Some((selector, settings)) => div().font_family(MONOSPACE_FONT).child(selector).child(settings.split_whitespace().collect::<Vec<_>>().join(" ")),
            None => div().child("No theme selector"),
        };
        let scopes = div().line_height(px(15.)).children(inspection.scopes.into_iter().map(|scope| div().child(scope)));
        let textmate = vec![("textmate scopes", scopes), ("foreground", rule)];
        vec![
            token.into_any_element(),
            separator().mb(px(SEPARATOR_MARGIN)).into_any_element(),
            metadata_table(metadata, window).into_any_element(),
            separator().my(px(SEPARATOR_MARGIN)).into_any_element(),
            metadata_table(textmate, window).into_any_element(),
        ]
    }
}

/// `table.tiw-metadata-table`: the user agent's 2px border spacing and 1px cell padding, the keys' column as wide
/// as its widest key and at least 150px, the values in the monospace font, wrapping.
fn metadata_table(rows: Vec<(&'static str, Div)>, window: &Window) -> Div {
    let key_width = rows.iter().map(|(key, _)| text_width(key, window)).fold(KEY_MIN_WIDTH, f32::max);
    div().flex().flex_col().gap(px(2.)).p(px(2.)).children(rows.into_iter().map(|(key, value)| {
        div()
            .flex()
            .gap(px(2.))
            .child(div().flex_none().w(px(key_width)).pl(px(1.)).pr(px(10.)).py(px(1.)).whitespace_nowrap().child(key))
            .child(value.flex_1().min_w_0().p(px(1.)).font_family(MONOSPACE_FONT))
    }))
}

fn text_width(text: &'static str, window: &Window) -> f32 {
    let run = TextRun { len: text.len(), font: gpui::font(UI_FONT), color: gpui::black(), background_color: None, underline: None, strikethrough: None };
    f32::from(window.text_system().shape_line(text.into(), px(FONT_SIZE), &[run], None).width)
}

/// `_compute` for the TextMate token at `position`; none without a grammar.
// shortcut: a line over `editor.maxTokenizationLineLength` has no tokens here, where upstream tokenizes it anew; reuse a fresh tokenizer if that matters.
fn inspect(document: &TextDocument, position: Position, theme: &Theme) -> Option<TokenInspection> {
    let line = &document.lines[position.line];
    let offset = line.char_indices().nth(position.column).map_or(line.len(), |(offset, _)| offset);
    let (range, scopes, token_type) = document.tokens.token_at(&document.lines, position.line, offset)?;
    let text = &line[range];
    let style = theme.tokens.style(&scopes);
    let color_map = theme.tokens.color_map();
    let language = scopes.language.as_deref().unwrap_or(&document.language_id);
    let names = scopes.names();
    Some(TokenInspection {
        text: render_token_text(text),
        length: text.encode_utf16().count(),
        metadata: metadata_rows(language, token_type, &color_map[style.foreground], &color_map[style.background], style.font_style),
        scopes: names.iter().rev().map(|name| name.to_string()).collect(),
        theme_rule: find_matching_theme_rule(theme.tokens.rules(), &names),
    })
}

/// `renderTokenText`: over 40 characters keeps the first and last 20 around an ellipsis; a tab shows as an arrow,
/// a space as a middle dot.
fn render_token_text(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let shown: Vec<char> = match chars.len() > 40 {
        true => chars[..20].iter().copied().chain(['…']).chain(chars[chars.len() - 20..].iter().copied()).collect(),
        false => chars,
    };
    shown
        .into_iter()
        .map(|c| match c {
            '\t' => '\u{2192}',
            ' ' => '\u{00B7}',
            c => c,
        })
        .collect()
}

/// The first table's rows: language, standard token type, then `_formatMetadata` of the TextMate metadata.
fn metadata_rows(language: &str, token_type: StandardTokenType, foreground: &str, background: &str, font_style: i32) -> Vec<(&'static str, String)> {
    let type_name = match token_type {
        StandardTokenType::Other => "Other",
        StandardTokenType::Comment => "Comment",
        StandardTokenType::String => "String",
        StandardTokenType::RegEx => "RegEx",
    };
    let mut rows = vec![("language", language.to_string()), ("standard token type", type_name.to_string()), ("foreground", foreground.to_string()), ("background", background.to_string())];
    let foreground = parse_hex(foreground).expect("the token color map holds hex colors");
    let background = parse_hex(background).expect("the token color map holds hex colors");
    match background.a == 1. {
        true => rows.push(("contrast ratio", format!("{:.2}", contrast_ratio(make_opaque(foreground, background), background)))),
        false => rows.push(("Contrast ratio cannot be precise for background colors that use transparency", String::new())),
    }
    let styles = [(textmate::BOLD, "bold"), (textmate::ITALIC, "italic"), (textmate::UNDERLINE, "underline"), (textmate::STRIKETHROUGH, "strikethrough")];
    let names: Vec<_> = styles.into_iter().filter(|(bit, _)| font_style & bit != 0).map(|(_, name)| name).collect();
    if !names.is_empty() {
        rows.push(("font style", names.join(" ")));
    }
    rows
}

/// `Color.makeOpaque`: a translucent color blended onto the opaque background, channels truncated as `RGBA` does.
fn make_opaque(color: gpui::Rgba, background: gpui::Rgba) -> gpui::Rgba {
    if color.a == 1. {
        return color;
    }
    let alpha = (color.a * 1000.).round() / 1000.;
    let blend = |channel: f32, under: f32| {
        let (channel, under) = ((channel * 255.).round(), (under * 255.).round());
        (under - alpha * (under - channel)).clamp(0., 255.).trunc() / 255.
    };
    gpui::Rgba { r: blend(color.r, background.r), g: blend(color.g, background.g), b: blend(color.b, background.b), a: 1. }
}

/// `Color.getContrastRatio`.
fn contrast_ratio(a: gpui::Rgba, b: gpui::Rgba) -> f64 {
    let (a, b) = (relative_luminance(a), relative_luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// TMHelper.ts `findMatchingThemeRule` with `onlyColorRules` false: from the innermost scope out, the most specific
/// selector matching the scope under its parents, a later rule winning a tie; its raw selector and settings JSON.
fn find_matching_theme_rule(rules: &[Value], scopes: &[&str]) -> Option<(String, String)> {
    for index in (0..scopes.len()).rev() {
        if let Some((selector, settings)) = matching_rule(rules, scopes[index], &scopes[..index]) {
            return Some((selector.to_string(), settings_json(settings)));
        }
    }
    None
}

/// `findMatchingThemeRule2`.
fn matching_rule<'a>(rules: &'a [Value], scope: &str, parents: &[&str]) -> Option<(&'a str, &'a Value)> {
    let mut result: Option<(&str, &Value)> = None;
    for rule in rules.iter().rev() {
        let selectors: Vec<&str> = match &rule["scope"] {
            Value::String(scope) => scope.split(',').map(str::trim).collect(),
            Value::Array(scopes) => scopes.iter().filter_map(Value::as_str).collect(),
            _ => continue,
        };
        for selector in selectors {
            if selector_matches(selector, scope, parents) && result.is_none_or(|(best, _)| more_specific(selector, best)) {
                result = Some((selector, &rule["settings"]));
            }
        }
    }
    result
}

/// `ThemeRule`: the last space-separated piece is the scope, the ones before it the parent scopes.
fn split_selector(selector: &str) -> (&str, Vec<&str>) {
    let mut pieces: Vec<&str> = selector.split(' ').collect();
    let scope = pieces.pop().expect("split yields a piece");
    (scope, pieces)
}

/// `ThemeRule._matches`: the scope matches, and the parent selectors match parent scopes in order, innermost last.
fn selector_matches(selector: &str, scope: &str, parents: &[&str]) -> bool {
    let (selector_scope, selector_parents) = split_selector(selector);
    if !matches_one(selector_scope, scope) {
        return false;
    }
    let mut remaining = selector_parents.iter().rev().peekable();
    for parent in parents.iter().rev() {
        if remaining.peek().is_some_and(|selector| matches_one(selector, parent)) {
            remaining.next();
        }
    }
    remaining.peek().is_none()
}

/// `ThemeRule._matchesOne`: the same scope or a scope under it.
fn matches_one(selector: &str, scope: &str) -> bool {
    scope.strip_prefix(selector).is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
}

/// `ThemeRule.isMoreSpecific`: a longer scope, then more parent scopes, then longer parent scopes in order.
fn more_specific(selector: &str, other: &str) -> bool {
    let ((scope, parents), (other_scope, other_parents)) = (split_selector(selector), split_selector(other));
    let order = scope.len().cmp(&other_scope.len()).then(parents.len().cmp(&other_parents.len())).then_with(|| parents.iter().map(|parent| parent.len()).cmp(other_parents.iter().map(|parent| parent.len())));
    order.is_gt()
}

/// `JSON.stringify(settings, null, '\t')`.
fn settings_json(settings: &Value) -> String {
    let mut json = Vec::new();
    settings.serialize(&mut serde_json::Serializer::with_formatter(&mut json, serde_json::ser::PrettyFormatter::with_indent(b"\t"))).expect("a JSON value serializes");
    String::from_utf8(json).expect("serde_json writes UTF-8")
}

#[cfg(test)]
mod tests;
