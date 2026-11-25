//! markdownRenderer.ts and editorMarkdownCodeBlockRenderer.ts from the parsed blocks: paragraphs, headings and
//! lists in the UI font, code blocks tokenized in the editor font, links that open or run their command.

use gpui::{Context, Div, FontStyle, FontWeight, InteractiveText, IntoElement, ParentElement, Rgba, StrikethroughStyle, Styled, StyledText, TextRun, div, px};
use serde_json::Value;

use super::Workbench;
use crate::markdown::{Block, Inline};

const UI_FONT: &str = ".SystemUIFont";
const EDITOR_FONT: &str = "Menlo";
const EDITOR_FONT_SIZE: f32 = 12.;

/// The fonts and colors one rendering of markdown uses: the hover's contents, or another widget's.
#[derive(Clone, Copy)]
pub(super) struct MarkdownStyle {
    pub font_size: f32,
    pub line_height: f32,
    pub foreground: Rgba,
}

impl Workbench {
    /// The blocks stacked with `p, .code, ul` 8px apart (hoverWidget.css `margin: 8px 0`, none at the ends); a code
    /// block without a language takes `model_language` (`editor.getModel().getLanguageId()`).
    pub(super) fn markdown_blocks(&self, id: &str, blocks: &[Block], style: MarkdownStyle, model_language: &str, cx: &mut Context<Self>) -> Div {
        let mut column = div().flex().flex_col().gap(px(8.));
        for (index, block) in blocks.iter().enumerate() {
            column = column.child(self.markdown_block(&format!("{id}-{index}"), block, style, model_language, cx));
        }
        column
    }

    fn markdown_block(&self, id: &str, block: &Block, style: MarkdownStyle, model_language: &str, cx: &mut Context<Self>) -> Div {
        match block {
            Block::Paragraph(inline) => div().line_height(px(style.line_height)).child(self.markdown_inline(id, inline, style, cx)),
            // hoverWidget.css: headings at line-height 1.1, the browser's h1-h6 sizes.
            Block::Heading(level, inline) => {
                let scale = [2., 1.5, 1.17, 1., 0.83, 0.67][(*level as usize).clamp(1, 6) - 1];
                let size = style.font_size * scale;
                div().text_size(px(size)).line_height(px(size * 1.1)).font_weight(FontWeight::BOLD).child(self.markdown_inline(id, inline, MarkdownStyle { font_size: size, line_height: size * 1.1, ..style }, cx))
            }
            Block::Code { language, text } => self.markdown_code(language, text, model_language),
            Block::List { start, items } => {
                let mut list = div().flex().flex_col().pl(px(20.));
                for (index, item) in items.iter().enumerate() {
                    let marker = match start {
                        Some(first) => format!("{}.", first + index as u64),
                        None => "\u{2022}".to_owned(),
                    };
                    let item = self.markdown_blocks(&format!("{id}-{index}"), item, style, model_language, cx).flex_1().gap(px(0.));
                    list = list.child(div().flex().child(div().absolute().ml(px(-20.)).w(px(16.)).flex().justify_end().child(marker)).child(item));
                }
                list
            }
            Block::Quote(blocks) => self.markdown_blocks(id, blocks, style, model_language, cx).pl(px(10.)).border_l(px(5.)),
            // hoverWidget.css `hr`: 1px at half the hover border's alpha, 4px above, -4px below, across the padding.
            Block::Rule => {
                let mut color = self.color("editorHoverWidget.border");
                color.a *= 0.5;
                div().mt(px(4.)).mb(px(-4.)).mx(px(-8.)).h(px(1.)).bg(color)
            }
        }
    }

    /// `tokenizeToString` of the code in the language its alias names, else the model's, in the editor's font
    /// (`applyFontInfo`).
    fn markdown_code(&self, language: &str, text: &str, model_language: &str) -> Div {
        let language = match language {
            "" => model_language.to_owned(),
            alias => self.language_by_name(alias).unwrap_or_else(|| "plaintext".to_owned()),
        };
        let tokens = crate::syntax::Tokenization::new(self.languages.grammars.grammar(&language));
        let lines: Vec<String> = text.split('\n').map(str::to_owned).collect();
        let line_height = crate::editor::LINE_HEIGHT;
        let mut code = div().flex().flex_col().font_family(EDITOR_FONT).text_size(px(EDITOR_FONT_SIZE)).line_height(px(line_height));
        for index in 0..lines.len() {
            let runs = tokens.token_styles(&self.theme, &lines, index, 4);
            code = code.child(div().min_h(px(line_height)).child(StyledText::new(crate::editor::expand_tabs(&lines[index], 4)).with_highlights(runs)));
        }
        div().child(code)
    }

    /// `getLanguageIdByLanguageName`: an id or a language name, ignoring case.
    fn language_by_name(&self, alias: &str) -> Option<String> {
        let alias = alias.to_lowercase();
        if self.languages.is_registered(&alias) {
            return Some(alias);
        }
        self.languages.sorted_names().into_iter().find(|(name, _)| name.to_lowercase() == alias).map(|(_, id)| id.to_owned())
    }

    /// A paragraph's runs: bold, italic, strikethrough, `code` in the editor font on `textCodeBlock.background`,
    /// links in `textLink.foreground` that open on click.
    fn markdown_inline(&self, id: &str, inline: &Inline, style: MarkdownStyle, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let link_color = self.color("textLink.foreground");
        let code_background = self.color("textCodeBlock.background");
        let mut runs = Vec::new();
        let mut links = Vec::new();
        let mut offset = 0;
        let run = |len: usize, span: Option<&crate::markdown::Span>| {
            let mut font = gpui::font(UI_FONT);
            let mut run = TextRun { len, font: font.clone(), color: style.foreground.into(), background_color: None, underline: None, strikethrough: None };
            let Some(span) = span else { return run };
            if span.code {
                font = gpui::font(EDITOR_FONT);
                run.background_color = Some(code_background.into());
            }
            if span.bold {
                font.weight = FontWeight::BOLD;
            }
            if span.italic {
                font.style = FontStyle::Italic;
            }
            if span.strike {
                run.strikethrough = Some(StrikethroughStyle { thickness: px(1.), color: None });
            }
            if span.link.is_some() {
                run.color = link_color.into();
            }
            run.font = font;
            run
        };
        for span in &inline.spans {
            if span.range.start > offset {
                runs.push(run(span.range.start - offset, None));
            }
            runs.push(run(span.range.len(), Some(span)));
            if let Some(link) = &span.link {
                links.push((span.range.clone(), link.clone()));
            }
            offset = span.range.end;
        }
        if inline.text.len() > offset {
            runs.push(run(inline.text.len() - offset, None));
        }
        let ranges: Vec<_> = links.iter().map(|(range, _)| range.clone()).collect();
        let targets: Vec<String> = links.into_iter().map(|(_, target)| target).collect();
        let text = StyledText::new(inline.text.clone()).with_runs(runs);
        let workbench = cx.entity().downgrade();
        InteractiveText::new(id.to_owned(), text).on_click(ranges, move |index, window, cx| {
            let target = targets[index].clone();
            let _ = workbench.update(cx, |workbench, cx| workbench.open_markdown_link(&target, window, cx));
        })
    }

    /// `openerService.open` of a rendered link: `command:` runs the command with the JSON arguments of its query,
    /// `file:` opens the file, anything else goes to the system.
    pub(super) fn open_markdown_link(&mut self, target: &str, window: &mut gpui::Window, cx: &mut Context<Self>) {
        if let Some(command) = target.strip_prefix("command:") {
            let (id, query) = command.split_once('?').unwrap_or((command, ""));
            let args = match crate::editor_history::percent_decode(query).and_then(|json| serde_json::from_str::<Value>(&json).ok()) {
                Some(Value::Array(args)) => args,
                Some(arg) => vec![arg],
                None => Vec::new(),
            };
            self.execute_command(id, args, window, cx);
            return;
        }
        if target.starts_with("file:")
            && let Some(path) = crate::editor_history::file_path(target)
        {
            self.open_file(path, true, window, cx);
            return;
        }
        cx.open_url(target);
    }
}
