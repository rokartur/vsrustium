use super::*;
use serde_json::json;

fn blocks(value: &str) -> Vec<Block> {
    parse(&MarkdownString { value, is_trusted: &Value::Null, support_theme_icons: true })
}

fn paragraph(block: &Block) -> &Inline {
    match block {
        Block::Paragraph(inline) => inline,
        other => panic!("not a paragraph: {other:?}"),
    }
}

#[test]
fn parses_a_typescript_hover() {
    let parsed = blocks("```typescript\nfunction main(): number\n```\nReturns the **answer** and `42`.");
    assert_eq!(parsed[0], Block::Code { language: "typescript".into(), text: "function main(): number".into() });
    let inline = paragraph(&parsed[1]);
    assert_eq!(inline.text, "Returns the answer and 42.");
    assert!(inline.spans.iter().any(|span| span.bold && &inline.text[span.range.clone()] == "answer"));
    assert!(inline.spans.iter().any(|span| span.code && &inline.text[span.range.clone()] == "42"));
}

#[test]
fn tight_list_items_keep_one_paragraph() {
    let parsed = blocks("- one *two*\n- three");
    let Block::List { start: None, items } = &parsed[0] else { panic!("{parsed:?}") };
    assert_eq!(items.len(), 2);
    assert_eq!(paragraph(&items[0][0]).text, "one two");
}

#[test]
fn drops_untrusted_command_links_and_html() {
    let parsed = blocks("[run](command:foo.bar) [web](https://example.com) <b>x</b>");
    let inline = paragraph(&parsed[0]);
    let links: Vec<_> = inline.spans.iter().filter_map(|span| span.link.as_deref()).collect();
    assert_eq!(links, ["https://example.com"]);
    let trusted = json!({ "enabledCommands": ["foo.bar"] });
    let parsed = parse(&MarkdownString { value: "[run](command:foo.bar?%5B1%5D)", is_trusted: &trusted, support_theme_icons: false });
    assert_eq!(paragraph(&parsed[0]).spans[0].link.as_deref(), Some("command:foo.bar?%5B1%5D"));
}

#[test]
fn theme_icons_split_the_text() {
    let parsed = blocks("$(error) failed $(sync~spin)");
    let inline = paragraph(&parsed[0]);
    assert_eq!(inline.text, " failed ");
    assert_eq!(inline.icons, [(0, "error".to_owned()), (8, "sync".to_owned())]);
    assert!(is_empty(&json!([{ "value": "" }, ""])));
    assert!(!is_empty(&json!([{ "value": "x" }])));
}
