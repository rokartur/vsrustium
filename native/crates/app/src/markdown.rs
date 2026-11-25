//! markdownRenderer.ts `renderMarkdown` up to the DOM: an `IMarkdownString` parsed (pulldown-cmark in place of
//! marked) into the blocks and styled text the views draw.

use std::ops::Range;

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use serde_json::Value;

#[cfg(test)]
mod tests;

#[derive(Debug, PartialEq)]
pub enum Block {
    Paragraph(Inline),
    Heading(u8, Inline),
    /// A fenced or indented code block: its language alias and text without the last newline.
    Code { language: String, text: String },
    List { start: Option<u64>, items: Vec<Vec<Block>> },
    Quote(Vec<Block>),
    Rule,
}

/// The text of a paragraph or heading with its styled ranges and, with `supportThemeIcons`, its `$(icon)`s.
#[derive(Debug, Default, PartialEq)]
pub struct Inline {
    pub text: String,
    pub spans: Vec<Span>,
    /// The byte offset in `text` each icon sits at, and its codicon id.
    pub icons: Vec<(usize, String)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Span {
    pub range: Range<usize>,
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
    /// The link target, kept only where `isLinkAllowed` lets it through.
    pub link: Option<String>,
}

/// `IMarkdownString` with the fields the renderer reads.
pub struct MarkdownString<'a> {
    pub value: &'a str,
    pub is_trusted: &'a Value,
    pub support_theme_icons: bool,
}

impl<'a> MarkdownString<'a> {
    pub fn from_value(value: &'a Value) -> Self {
        MarkdownString { value: value["value"].as_str().unwrap_or_default(), is_trusted: &value["isTrusted"], support_theme_icons: value["supportThemeIcons"] == true }
    }

    /// `isTrusted`: `true`, or `{ enabledCommands }` naming the command.
    pub fn allows_command(&self, command: &str) -> bool {
        match self.is_trusted {
            Value::Bool(trusted) => *trusted,
            Value::Object(trust) => trust["enabledCommands"].as_array().is_some_and(|commands| commands.iter().any(|enabled| enabled == command)),
            _ => false,
        }
    }
}

/// `isEmptyMarkdownString`.
pub fn is_empty(markdown: &Value) -> bool {
    match markdown {
        Value::Array(strings) => strings.iter().all(is_empty),
        Value::String(text) => text.is_empty(),
        Value::Object(_) => markdown["value"].as_str().is_none_or(str::is_empty),
        _ => true,
    }
}

/// The rendered text as `innerText` reads it, runs of spaces and tabs collapsed (markdownHoverParticipant.ts
/// `getAccessibleContent`).
pub fn plain_text(blocks: &[Block]) -> String {
    let mut lines = Vec::new();
    for block in blocks {
        match block {
            Block::Paragraph(inline) | Block::Heading(_, inline) => lines.push(inline.text.clone()),
            Block::Code { text, .. } => lines.push(text.clone()),
            Block::List { items, .. } => lines.extend(items.iter().map(|item| plain_text(item))),
            Block::Quote(blocks) => lines.push(plain_text(blocks)),
            Block::Rule => {}
        }
    }
    let text = lines.join("\n");
    let mut collapsed = String::new();
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            collapsed.push('\n');
        }
        collapsed.push_str(&line.split([' ', '\t']).filter(|word| !word.is_empty()).collect::<Vec<_>>().join(" "));
    }
    collapsed
}

/// `renderMarkdown`'s parse with marked's GFM defaults; HTML without `supportHtml` renders nothing, as its
/// `renderer.html` override.
pub fn parse(markdown: &MarkdownString) -> Vec<Block> {
    // preprocessMarkdownString: values that are too long would freeze the UI.
    let value = match markdown.value.char_indices().nth(100_000) {
        Some((end, _)) => format!("{}…", &markdown.value[..end]),
        None => markdown.value.to_owned(),
    };
    let mut builder = Builder { markdown, stack: vec![Container::Root(Vec::new())], styles: Vec::new() };
    for event in Parser::new_ext(&value, Options::ENABLE_STRIKETHROUGH) {
        builder.event(event);
    }
    match builder.stack.pop() {
        Some(Container::Root(blocks)) => blocks,
        _ => unreachable!("every container closes before the root"),
    }
}

enum Container {
    Root(Vec<Block>),
    Paragraph(Inline),
    Heading(u8, Inline),
    Code { language: String, text: String },
    List { start: Option<u64>, items: Vec<Vec<Block>> },
    Item(Vec<Block>),
    Quote(Vec<Block>),
}

#[derive(Clone)]
enum Style {
    Bold,
    Italic,
    Strike,
    Link(Option<String>),
}

struct Builder<'a, 'b> {
    markdown: &'a MarkdownString<'b>,
    stack: Vec<Container>,
    styles: Vec<Style>,
}

impl Builder<'_, '_> {
    fn event(&mut self, event: Event) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.text(&text, false),
            Event::Code(text) => self.text(&text, true),
            Event::SoftBreak => self.text(" ", false),
            Event::HardBreak => self.text("\n", false),
            Event::Rule => self.push_block(Block::Rule),
            Event::Html(_) | Event::InlineHtml(_) | Event::FootnoteReference(_) | Event::TaskListMarker(_) => {}
            Event::InlineMath(text) | Event::DisplayMath(text) => self.text(&text, false),
        }
    }

    fn start(&mut self, tag: Tag) {
        let container = match tag {
            Tag::Paragraph => Container::Paragraph(Inline::default()),
            Tag::Heading { level, .. } => Container::Heading(level as u8, Inline::default()),
            Tag::CodeBlock(kind) => {
                let language = match kind {
                    CodeBlockKind::Fenced(info) => info.split_whitespace().next().unwrap_or_default().to_owned(),
                    CodeBlockKind::Indented => String::new(),
                };
                Container::Code { language, text: String::new() }
            }
            Tag::List(start) => Container::List { start, items: Vec::new() },
            Tag::Item => Container::Item(Vec::new()),
            Tag::BlockQuote(_) => Container::Quote(Vec::new()),
            Tag::Strong => return self.styles.push(Style::Bold),
            Tag::Emphasis => return self.styles.push(Style::Italic),
            Tag::Strikethrough => return self.styles.push(Style::Strike),
            Tag::Link { dest_url, .. } => return self.styles.push(Style::Link(self.allowed_link(&dest_url))),
            _ => return,
        };
        self.stack.push(container);
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Strong | TagEnd::Emphasis | TagEnd::Strikethrough | TagEnd::Link => drop(self.styles.pop()),
            TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::CodeBlock | TagEnd::List(_) | TagEnd::Item | TagEnd::BlockQuote(_) => {
                let block = match self.stack.pop().expect("an open container") {
                    Container::Paragraph(inline) => Block::Paragraph(inline),
                    Container::Heading(level, inline) => Block::Heading(level, inline),
                    Container::Code { language, mut text } => {
                        if text.ends_with('\n') {
                            text.pop();
                        }
                        Block::Code { language, text }
                    }
                    Container::List { start, items } => Block::List { start, items },
                    Container::Quote(blocks) => Block::Quote(blocks),
                    Container::Item(blocks) => {
                        if let Some(Container::List { items, .. }) = self.stack.last_mut() {
                            items.push(blocks);
                        }
                        return;
                    }
                    Container::Root(_) => unreachable!("the root never closes"),
                };
                self.push_block(block);
            }
            _ => {}
        }
    }

    fn push_block(&mut self, block: Block) {
        match self.stack.last_mut().expect("the root") {
            Container::Root(blocks) | Container::Item(blocks) | Container::Quote(blocks) => blocks.push(block),
            _ => {}
        }
    }

    /// Text of a tight list item has no paragraph around it, so it opens one.
    fn text(&mut self, text: &str, code: bool) {
        if let Some(Container::Code { text: code_text, .. }) = self.stack.last_mut() {
            return code_text.push_str(text);
        }
        if matches!(self.stack.last(), Some(Container::Item(_) | Container::Quote(_) | Container::Root(_))) {
            self.stack.push(Container::Paragraph(Inline::default()));
            self.append(text, code);
            let Some(Container::Paragraph(inline)) = self.stack.pop() else { unreachable!("pushed above") };
            return self.push_or_merge(inline);
        }
        self.append(text, code);
    }

    /// Consecutive loose texts of a tight item join one paragraph.
    fn push_or_merge(&mut self, inline: Inline) {
        let blocks = match self.stack.last_mut() {
            Some(Container::Item(blocks) | Container::Quote(blocks) | Container::Root(blocks)) => blocks,
            _ => return,
        };
        if let Some(Block::Paragraph(last)) = blocks.last_mut() {
            let offset = last.text.len();
            last.text.push_str(&inline.text);
            last.spans.extend(inline.spans.into_iter().map(|span| Span { range: span.range.start + offset..span.range.end + offset, ..span }));
            last.icons.extend(inline.icons.into_iter().map(|(at, icon)| (at + offset, icon)));
            return;
        }
        blocks.push(Block::Paragraph(inline));
    }

    fn append(&mut self, text: &str, code: bool) {
        let mut span = Span { code, ..Span::default() };
        for style in &self.styles {
            match style {
                Style::Bold => span.bold = true,
                Style::Italic => span.italic = true,
                Style::Strike => span.strike = true,
                Style::Link(link) => span.link = link.clone(),
            }
        }
        let support_icons = self.markdown.support_theme_icons && !code;
        let Some(Container::Paragraph(inline) | Container::Heading(_, inline)) = self.stack.last_mut() else { return };
        let mut rest = text;
        while !rest.is_empty() {
            let (before, icon, after) = match support_icons.then(|| split_icon(rest)).flatten() {
                Some((before, icon, after)) => (before, Some(icon), after),
                None => (rest, None, ""),
            };
            if !before.is_empty() {
                let start = inline.text.len();
                inline.text.push_str(before);
                inline.spans.push(Span { range: start..inline.text.len(), ..span.clone() });
            }
            if let Some(icon) = icon {
                inline.icons.push((inline.text.len(), icon.to_owned()));
            }
            rest = after;
        }
    }

    /// The link checks of `renderMarkdown`: `data:` and `javascript:` links, and `command:` links the string does not
    /// trust, drop to their text.
    fn allowed_link(&self, href: &str) -> Option<String> {
        let lower = href.to_ascii_lowercase();
        if href.is_empty() || lower.starts_with("data:") || lower.contains("javascript:") {
            return None;
        }
        if let Some(command) = lower.strip_prefix("command:") {
            let id = href["command:".len()..].trim_start_matches("///");
            let id = id.split('?').next().unwrap_or_default();
            if command.trim_start_matches("///").starts_with("_workbench.downloadresource") || !self.markdown.allows_command(id) {
                return None;
            }
        }
        Some(href.to_owned())
    }
}

/// The first `$(name)` or `$(name~modifier)` of `text`: the text before it, the codicon id, the rest.
fn split_icon(text: &str) -> Option<(&str, &str, &str)> {
    let mut search = 0;
    while let Some(found) = text[search..].find("$(") {
        let start = search + found;
        let body_start = start + 2;
        if let Some(close) = text[body_start..].find(')') {
            let body = &text[body_start..body_start + close];
            let name = body.split('~').next().unwrap_or_default();
            if !name.is_empty() && name.chars().all(|char| char.is_ascii_alphanumeric() || char == '-') {
                return Some((&text[..start], name, &text[body_start + close + 1..]));
            }
        }
        search = body_start;
    }
    None
}
