//! workbench/contrib/markdown markdownDocumentRenderer.ts `renderMarkdownDocument`: GitHub markdown as the
//! document HTML of the extension editor and the walkthroughs, with heading ids (markedGfmHeadingIdPlugin.ts) and
//! the sanitizer's link and media rules (domSanitize.ts `hookDomPurifyHrefAndSrcSanitizer`). pulldown-cmark stands
//! in for marked.
// shortcut: code blocks are not syntax highlighted (upstream tokenizes them with the language's grammar).

use pulldown_cmark::{CowStr, Event, Options, Parser, Tag, TagEnd, html};

/// `MarkdownDocumentSanitizerConfig`: the protocols links and images may use, `None` for any (`override: '*'`).
pub struct Sanitizer {
    pub link_protocols: Option<&'static [&'static str]>,
    pub media_protocols: &'static [&'static str],
    pub allow_relative_media: bool,
}

impl Default for Sanitizer {
    /// `defaultAllowedLinkProtocols` and the sanitizer's media default, both http and https.
    fn default() -> Self {
        Sanitizer { link_protocols: Some(&["http", "https"]), media_protocols: &["http", "https"], allow_relative_media: false }
    }
}

/// Raw HTML that would act before or outside the webview's CSP (`<meta http-equiv="refresh">`, `<base>`, frames)
/// is shown as text, as DOMPurify removes it.
pub fn render_markdown_document(text: &str, sanitizer: &Sanitizer) -> String {
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS | Options::ENABLE_GFM;
    let mut events: Vec<Event> = Vec::new();
    let mut parser = Parser::new_ext(text, options);
    while let Some(event) = parser.next() {
        match event {
            Event::Html(raw) | Event::InlineHtml(raw) if is_unsafe_html(&raw) => events.push(Event::Text(raw)),
            Event::Start(Tag::Image { dest_url, title, .. }) => {
                // The alt text is the image's inline content, as text.
                let mut alt = String::new();
                for inner in parser.by_ref() {
                    match inner {
                        Event::End(TagEnd::Image) => break,
                        Event::Text(text) | Event::Code(text) => alt.push_str(&text),
                        _ => {}
                    }
                }
                let src = allowed(&dest_url, Some(sanitizer.media_protocols), sanitizer.allow_relative_media).then(|| format!(" src=\"{}\"", escape(&dest_url))).unwrap_or_default();
                let title = if title.is_empty() { String::new() } else { format!(" title=\"{}\"", escape(&title)) };
                events.push(Event::InlineHtml(CowStr::from(format!("<img{src} alt=\"{}\"{title}>", escape(&alt)))));
            }
            Event::Start(Tag::Link { .. }) | Event::End(TagEnd::Link) => events.push(sanitize_link(event, sanitizer)),
            Event::Start(Tag::Heading { level, .. }) => {
                let start = events.len();
                let mut raw = String::new();
                for inner in parser.by_ref() {
                    match &inner {
                        Event::End(TagEnd::Heading(_)) => break,
                        Event::Text(text) | Event::Code(text) => raw.push_str(text),
                        _ => {}
                    }
                    events.push(sanitize_link(inner, sanitizer));
                }
                let level = level as usize;
                events.insert(start, Event::Html(CowStr::from(format!("<h{level} id=\"{}\">", escape(&slugify(&raw))))));
                events.push(Event::Html(CowStr::from(format!("</h{level}>\n"))));
            }
            other => events.push(other),
        }
    }
    let mut out = String::new();
    html::push_html(&mut out, events.into_iter());
    out
}

/// A link keeps its `href` only with an allowed protocol or as a fragment; other events pass.
fn sanitize_link<'a>(event: Event<'a>, sanitizer: &Sanitizer) -> Event<'a> {
    match event {
        Event::Start(Tag::Link { dest_url, title, .. }) => {
            let keep = dest_url.starts_with('#') || allowed(&dest_url, sanitizer.link_protocols, false);
            let href = if keep { format!(" href=\"{}\"", escape(&dest_url)) } else { String::new() };
            let title = if title.is_empty() { String::new() } else { format!(" title=\"{}\"", escape(&title)) };
            Event::InlineHtml(CowStr::from(format!("<a{href}{title}>")))
        }
        Event::End(TagEnd::Link) => Event::InlineHtml(CowStr::Borrowed("</a>")),
        other => other,
    }
}

/// markedGfmHeadingIdPlugin.ts `slugify`: trimmed, lowercase, each whitespace a `-`.
fn slugify(heading: &str) -> String {
    heading.trim().to_lowercase().chars().map(|c| if c.is_whitespace() { '-' } else { c }).collect()
}

/// domSanitize.ts `validateLink`: a URL with an allowed scheme, or a relative path when allowed.
fn allowed(url: &str, protocols: Option<&[&str]>, allow_relative: bool) -> bool {
    let Some(protocols) = protocols else { return true };
    let scheme = url.split_once(':').map(|(scheme, _)| scheme).filter(|scheme| !scheme.is_empty() && scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)));
    match scheme {
        Some(scheme) => protocols.iter().any(|protocol| protocol.eq_ignore_ascii_case(scheme)),
        None => allow_relative,
    }
}

fn is_unsafe_html(raw: &str) -> bool {
    let lower = raw.to_ascii_lowercase();
    ["<meta", "<base", "<iframe", "<frame", "<object", "<embed", "<script", "<form", "<link"].iter().any(|tag| lower.contains(tag))
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;").replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    // markdownDocumentRenderer.test.ts
    #[test]
    fn removes_relative_images_unless_allowed() {
        assert_eq!(render_markdown_document("![alt](src/img.png)", &Sanitizer::default()), "<p><img alt=\"alt\"></p>\n");
        let relative = Sanitizer { allow_relative_media: true, ..Sanitizer::default() };
        assert_eq!(render_markdown_document("![alt](src/img.png)", &relative), "<p><img src=\"src/img.png\" alt=\"alt\"></p>\n");
    }

    #[test]
    fn renders_github_markdown_with_heading_ids_and_safe_links() {
        let render = |text| render_markdown_document(text, &Sanitizer::default());
        assert_eq!(render("# Big Title\n\n| a |\n|---|\n| b |\n\n~~x~~"), "<h1 id=\"big-title\">Big Title</h1>\n<table><thead><tr><th>a</th></tr></thead><tbody>\n<tr><td>b</td></tr>\n</tbody></table>\n<p><del>x</del></p>\n");
        assert_eq!(render("<meta http-equiv=\"refresh\" content=\"0;url=https://x\">\n\n<b>ok</b>"), "&lt;meta http-equiv=\"refresh\" content=\"0;url=https://x\"&gt;\n<p><b>ok</b></p>\n");
        assert_eq!(render("[a](https://x) [b](command:foo) [c](#top)"), "<p><a href=\"https://x\">a</a> <a>b</a> <a href=\"#top\">c</a></p>\n");
        assert_eq!(render_markdown_document("[b](command:foo)", &Sanitizer { link_protocols: None, ..Sanitizer::default() }), "<p><a href=\"command:foo\">b</a></p>\n");
    }
}
