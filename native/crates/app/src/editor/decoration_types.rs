//! `vscode.window.createTextEditorDecorationType` on the main side: codeEditorService.ts `DecorationTypeOptionsProvider`
//! (the `IDecorationRenderOptions` an extension registers, with its `light` and `dark` overrides) and the ranges
//! `$trySetDecorations` / `$trySetDecorationsFast` set per text editor and type.

use serde_json::Value;

use super::Position;

/// A color an extension gives: a CSS color string or a `ThemeColor` id.
#[derive(Clone, Debug, PartialEq)]
pub enum DecorationColor {
    Css(String),
    Theme(String),
}

impl DecorationColor {
    fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::String(css) => Some(DecorationColor::Css(css.clone())),
            Value::Object(theme) => theme.get("id").and_then(Value::as_str).map(|id| DecorationColor::Theme(id.to_string())),
            _ => None,
        }
    }
}

/// `IContentDecorationRenderOptions` of `before` and `after`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Attachment {
    pub content_text: String,
    pub color: Option<DecorationColor>,
    pub background_color: Option<DecorationColor>,
    pub font_style: Option<String>,
    pub font_weight: Option<String>,
}

/// The render options one theme kind sees: the base options with its `light` or `dark` object over them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DecorationStyle {
    pub is_whole_line: bool,
    pub background_color: Option<DecorationColor>,
    pub color: Option<DecorationColor>,
    pub outline_color: Option<DecorationColor>,
    pub outline_width: Option<String>,
    pub border_color: Option<DecorationColor>,
    pub border_width: Option<String>,
    pub font_style: Option<String>,
    pub font_weight: Option<String>,
    pub text_decoration: Option<String>,
    pub opacity: Option<f32>,
    pub before: Option<Attachment>,
    pub after: Option<Attachment>,
    pub overview_ruler_color: Option<DecorationColor>,
    /// `OverviewRulerLane`: 1 left, 2 center, 4 right, 7 full.
    pub overview_ruler_lane: u64,
}

impl DecorationStyle {
    /// `IDecorationRenderOptions` for a dark (or high contrast dark) theme when `dark`.
    pub fn from_options(options: &Value, dark: bool) -> Self {
        let mut style = DecorationStyle { is_whole_line: options["isWholeLine"] == true, overview_ruler_lane: options["overviewRulerLane"].as_u64().unwrap_or(7), ..DecorationStyle::default() };
        style.apply(options);
        style.apply(&options[if dark { "dark" } else { "light" }]);
        style
    }

    fn apply(&mut self, options: &Value) {
        let color = |key: &str| DecorationColor::from_value(&options[key]);
        let text = |key: &str| options[key].as_str().map(str::to_string);
        set(&mut self.background_color, color("backgroundColor"));
        set(&mut self.color, color("color"));
        set(&mut self.outline_color, color("outlineColor"));
        set(&mut self.outline_width, text("outlineWidth"));
        set(&mut self.border_color, color("borderColor"));
        set(&mut self.border_width, text("borderWidth"));
        set(&mut self.font_style, text("fontStyle"));
        set(&mut self.font_weight, text("fontWeight"));
        set(&mut self.text_decoration, text("textDecoration"));
        set(&mut self.opacity, options["opacity"].as_str().and_then(|opacity| opacity.parse().ok()));
        set(&mut self.overview_ruler_color, color("overviewRulerColor"));
        for (key, slot) in [("before", &mut self.before), ("after", &mut self.after)] {
            let attachment = &options[key];
            if !attachment.is_object() {
                continue;
            }
            let mut merged = slot.take().unwrap_or_default();
            if let Some(text) = attachment["contentText"].as_str() {
                merged.content_text = text.to_string();
            }
            set(&mut merged.color, DecorationColor::from_value(&attachment["color"]));
            set(&mut merged.background_color, DecorationColor::from_value(&attachment["backgroundColor"]));
            set(&mut merged.font_style, attachment["fontStyle"].as_str().map(str::to_string));
            set(&mut merged.font_weight, attachment["fontWeight"].as_str().map(str::to_string));
            *slot = Some(merged);
        }
    }
}

fn set<T>(slot: &mut Option<T>, value: Option<T>) {
    if value.is_some() {
        *slot = value;
    }
}

/// One decoration of a type in an editor: 0-based model positions, and the `renderOptions` of `IDecorationOptions`.
#[derive(Clone, Debug, PartialEq)]
pub struct TextDecoration {
    pub key: String,
    pub start: Position,
    pub end: Position,
    pub render_options: Option<Value>,
}

/// `$trySetDecorations(id, key, ranges)`: `IDecorationOptions` with 1-based `range`s.
pub fn decorations_from_options(key: &str, options: &Value) -> Vec<TextDecoration> {
    options
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|option| {
            let range = &option["range"];
            let position = |line: &str, column: &str| Some(Position::new((range[line].as_u64()? as usize).saturating_sub(1), (range[column].as_u64()? as usize).saturating_sub(1)));
            Some(TextDecoration {
                key: key.to_string(),
                start: position("startLineNumber", "startColumn")?,
                end: position("endLineNumber", "endColumn")?,
                render_options: option.get("renderOptions").filter(|options| options.is_object()).cloned(),
            })
        })
        .collect()
}

/// `$trySetDecorationsFast(id, key, ranges)`: four 1-based numbers per range.
pub fn decorations_from_numbers(key: &str, numbers: &Value) -> Vec<TextDecoration> {
    let numbers: Vec<usize> = numbers.as_array().into_iter().flatten().filter_map(Value::as_u64).map(|number| number as usize).collect();
    numbers
        .chunks_exact(4)
        .map(|range| TextDecoration {
            key: key.to_string(),
            start: Position::new(range[0].saturating_sub(1), range[1].saturating_sub(1)),
            end: Position::new(range[2].saturating_sub(1), range[3].saturating_sub(1)),
            render_options: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn theme_overrides_apply_over_the_base_options() {
        let options = json!({
            "isWholeLine": true,
            "backgroundColor": { "id": "merge.currentHeaderBackground" },
            "outlineWidth": "1pt",
            "after": { "contentText": " (Current Change)", "color": { "id": "descriptionForeground" } },
            "light": { "backgroundColor": "#ff000033" },
        });
        let dark = DecorationStyle::from_options(&options, true);
        assert!(dark.is_whole_line);
        assert_eq!(dark.background_color, Some(DecorationColor::Theme("merge.currentHeaderBackground".into())));
        assert_eq!(dark.after.as_ref().unwrap().content_text, " (Current Change)");
        assert_eq!(DecorationStyle::from_options(&options, false).background_color, Some(DecorationColor::Css("#ff000033".into())));
    }

    #[test]
    fn ranges_become_zero_based_positions() {
        let fast = decorations_from_numbers("k", &json!([1, 1, 2, 5, 3, 2, 3, 4]));
        assert_eq!((fast[1].start, fast[1].end), (Position::new(2, 1), Position::new(2, 3)));
        let options = decorations_from_options("k", &json!([{ "range": { "startLineNumber": 4, "startColumn": 1, "endLineNumber": 4, "endColumn": 7 }, "renderOptions": { "after": { "contentText": "x" } } }]));
        assert_eq!(options[0].start, Position::new(3, 0));
        assert!(options[0].render_options.is_some());
    }
}
