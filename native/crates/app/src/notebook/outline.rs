//! notebookOutlineEntryFactory.ts with OutlineEntry.ts and notebookOutlineDataSource.ts: a notebook's cells as outline
//! entries (markdown headers, else each cell's first line), nested by level, and the entry of the focused cell.

use pulldown_cmark::{Event, Parser, Tag, TagEnd};

/// `NotebookOutlineConstants.NonHeaderOutlineLevel`.
pub const NON_HEADER_LEVEL: u8 = 7;

/// What the outline reads of one cell.
pub struct CellSource<'a> {
    pub markup: bool,
    pub text: &'a str,
    pub executing: bool,
}

#[derive(Debug, PartialEq)]
pub struct Entry {
    pub cell: usize,
    pub level: u8,
    pub label: String,
    /// `OutlineEntry.icon`.
    pub icon: &'static str,
    pub markup: bool,
    pub parent: Option<usize>,
}

/// `recomputeState`: the entries of every cell in order, each with its parent (the nearest entry above of a lower
/// level).
pub fn entries(cells: &[CellSource]) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::new();
    for (index, cell) in cells.iter().enumerate() {
        for (level, label) in cell_entries(cell) {
            let icon = match (cell.executing, cell.markup) {
                (true, _) => "sync",
                (false, true) => "markdown",
                (false, false) => "code",
            };
            entries.push(Entry { cell: index, level, label, icon, markup: cell.markup, parent: None });
        }
    }
    let mut stack: Vec<usize> = Vec::new();
    for index in 0..entries.len() {
        while stack.last().is_some_and(|parent| entries[*parent].level >= entries[index].level) {
            stack.pop();
        }
        entries[index].parent = stack.last().copied();
        stack.push(index);
    }
    entries
}

/// `getOutlineEntries` without cached symbols: a markup cell's headers, else the first non-empty line of the cell.
fn cell_entries(cell: &CellSource) -> Vec<(u8, String)> {
    if cell.markup {
        let content: String = cell.text.chars().take(10000).collect();
        let headers = markdown_headers(&content);
        if !headers.is_empty() {
            return headers;
        }
    }
    let line = first_non_empty_line(cell.text);
    let preview = match cell.markup {
        true => plaintext(&line),
        false => line,
    };
    let preview = preview.trim();
    vec![(NON_HEADER_LEVEL, if preview.is_empty() { "empty cell".to_owned() } else { preview.to_owned() })]
}

/// `getMarkdownHeadersInCellFallbackToHtmlTags`: the top level headings, else the first `<hN>` tag.
fn markdown_headers(content: &str) -> Vec<(u8, String)> {
    let mut headers = Vec::new();
    let mut depth = 0;
    let mut heading: Option<(u8, String)> = None;
    for event in Parser::new_ext(content, pulldown_cmark::Options::ENABLE_TABLES | pulldown_cmark::Options::ENABLE_STRIKETHROUGH) {
        match event {
            Event::Start(Tag::Heading { level, .. }) if depth == 0 => heading = Some((level as u8, String::new())),
            Event::End(TagEnd::Heading(_)) if heading.is_some() && depth == 0 => {
                let (level, text) = heading.take().expect("checked above");
                headers.push((level, text.trim().to_owned()));
            }
            Event::Start(_) => depth += 1,
            Event::End(_) => depth -= 1,
            Event::Text(text) | Event::Code(text) => {
                if let Some((_, label)) = &mut heading {
                    label.push_str(&text);
                }
            }
            _ => {}
        }
    }
    if headers.is_empty()
        && let Some(captures) = regex::Regex::new(r"(?i)<h([1-6]).*>(.*)</h([1-6])>").expect("a valid pattern").captures(content)
        && captures[1] == captures[3]
    {
        headers.push((captures[1].parse().expect("one digit"), captures[2].trim().to_owned()));
    }
    headers
}

/// `getCellFirstNonEmptyLine`.
fn first_non_empty_line(text: &str) -> String {
    match text.lines().find(|line| !line.trim().is_empty()) {
        Some(line) => line.to_owned(),
        None => text.chars().take(100).collect(),
    }
}

/// `renderAsPlaintext` of one line of markdown.
fn plaintext(line: &str) -> String {
    let mut text = String::new();
    for event in Parser::new(line) {
        if let Event::Text(part) | Event::Code(part) = event {
            text.push_str(&part);
        }
    }
    text
}

/// `recomputeActive`: the first entry of the focused cell.
pub fn active(entries: &[Entry], focus: usize) -> Option<usize> {
    entries.iter().position(|entry| entry.cell == focus)
}

/// `NotebookBreadcrumbsProvider.getBreadcrumbElements`: the active entry and its parents, outermost first, code
/// cells only with `notebook.breadcrumbs.showCodeCells`.
pub fn breadcrumbs(entries: &[Entry], active: Option<usize>, show_code_cells: bool) -> Vec<&Entry> {
    let mut chain = Vec::new();
    let mut candidate = active;
    while let Some(index) = candidate {
        let entry = &entries[index];
        if show_code_cells || entry.markup {
            chain.insert(0, entry);
        }
        candidate = entry.parent;
    }
    chain
}

/// `filterEntry` with `notebook.outline.showMarkdownHeadersOnly` and `notebook.outline.showCodeCells`: whether the
/// Outline view leaves the entry out.
pub fn filtered(entry: &Entry, markdown_headers_only: bool, show_code_cells: bool) -> bool {
    (markdown_headers_only && entry.markup && entry.level == NON_HEADER_LEVEL) || (!show_code_cells && !entry.markup)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(markup: bool, text: &str) -> CellSource<'_> {
        CellSource { markup, text, executing: false }
    }

    #[test]
    fn headers_nest_cells_below_them() {
        let cells = [cell(true, "# Sample notebook\nIntro"), cell(false, "\n  print('hi')\n"), cell(true, "## Part\n### Sub"), cell(false, ""), cell(true, "some **bold** text")];
        let entries = entries(&cells);
        let labels: Vec<(&str, u8, Option<usize>)> = entries.iter().map(|entry| (entry.label.as_str(), entry.level, entry.parent)).collect();
        assert_eq!(labels, [("Sample notebook", 1, None), ("print('hi')", 7, Some(0)), ("Part", 2, Some(0)), ("Sub", 3, Some(2)), ("empty cell", 7, Some(3)), ("some bold text", 7, Some(3))]);
        assert_eq!(entries[1].icon, "code");
        let crumbs: Vec<&str> = breadcrumbs(&entries, active(&entries, 3), true).iter().map(|entry| entry.label.as_str()).collect();
        assert_eq!(crumbs, ["Sample notebook", "Part", "Sub", "empty cell"]);
        let crumbs: Vec<&str> = breadcrumbs(&entries, active(&entries, 3), false).iter().map(|entry| entry.label.as_str()).collect();
        assert_eq!(crumbs, ["Sample notebook", "Part", "Sub"]);
    }

    #[test]
    fn html_headers_and_nested_headings() {
        assert_eq!(markdown_headers("<h2 class=\"x\">Title</h2>"), [(2, "Title".to_owned())]);
        assert_eq!(markdown_headers("> # quoted"), []);
        assert_eq!(markdown_headers("Title\n====="), [(1, "Title".to_owned())]);
    }
}
