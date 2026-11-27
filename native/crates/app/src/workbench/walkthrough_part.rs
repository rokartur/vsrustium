//! walkThroughPart.ts, walkThroughInput.ts, walkThroughActions.ts, editorWalkThrough.ts and
//! walkThroughContentProvider.ts: Help > Editor Playground, a markdown page (media/walkThroughPart.css) whose code
//! blocks are live code editors over `walkThroughSnippet` models, scrolled with the page. The focused snippet is the
//! group's code editor (`EditorGroup::stacked_editors`), the others draw as `embedded_code_editor`s.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;

use gpui::{
    AnyElement, Context, Div, FocusHandle, FontWeight, InteractiveText, MouseButton, MouseDownEvent, ScrollDelta, ScrollHandle, ScrollWheelEvent, StyledText,
    TextRun, Window, div, point, prelude::*, px,
};
use serde_json::{Value, json};

use super::Workbench;
use super::welcome_page::WALKTHROUGH_SCHEME;
use crate::commands::Outcome;
use crate::editor::view_state::ViewStates;
use crate::editor::{OpenEditor, TextDocument};
use crate::list_tree::wheel_scroll_delta;
use crate::markdown::{Block, Inline, MarkdownString};

#[cfg(test)]
mod tests;

/// `Schemas.walkThroughSnippet`.
pub const SNIPPET_SCHEME: &str = "walkThroughSnippet";
const MODULE_ID: &str = "vs/workbench/contrib/welcomeWalkthrough/browser/editor/vs_code_editor_walkthrough";
/// vs_code_editor_walkthrough.ts on the desktop (`isServerless` false), its `|` already backticks.
const CONTENT: &str = include_str!("walkthrough_part/vs_code_editor_walkthrough.md");
const VIEW_STATE_STORAGE_KEY: &str = "memento/workbench.editor.walkThroughPart";
const VIEW_STATE_MEMENTO_KEY: &str = "walkThroughEditorViewState";
const UNBOUND_COMMAND: &str = "unbound";
const UI_FONT: &str = ".SystemUIFont";
/// `--monaco-monospace-font` starts with SF Mono, which GPUI cannot load by name; the editor's Menlo stands in.
const MONOSPACE_FONT: &str = "Menlo";

#[derive(Default)]
pub struct Playground {
    focus: Option<FocusHandle>,
    scroll: ScrollHandle,
    view_states: ViewStates,
    /// Whether a playground tab was open at the last editor change, to save its view state when it closes.
    was_open: bool,
    /// The focused snippet's model id and cursor as last drawn: a new one scrolls the page to it.
    revealed_cursor: Cell<Option<(u32, usize)>>,
    /// The page after `expandMacros`, with the keybindings generation it resolved.
    expanded_content: RefCell<Option<(u64, String)>>,
}

impl Workbench {
    pub(super) fn register_playground_commands(&mut self) {
        self.commands.register("workbench.action.showInteractivePlayground", |workbench, _, window, cx| {
            workbench.open_playground(window, cx);
            Outcome::done()
        });
        // walkThroughActions.ts: 3 times `editor.fontSize` per arrow, a page per page key.
        let commands: [(&str, fn(&Workbench) -> f32); 4] = [
            ("workbench.action.interactivePlayground.arrowUp", |workbench| -workbench.playground_arrow_scroll_height()),
            ("workbench.action.interactivePlayground.arrowDown", |workbench| workbench.playground_arrow_scroll_height()),
            ("workbench.action.interactivePlayground.pageUp", |workbench| -f32::from(workbench.playground.scroll.bounds().size.height)),
            ("workbench.action.interactivePlayground.pageDown", |workbench| f32::from(workbench.playground.scroll.bounds().size.height)),
        ];
        for (id, delta) in commands {
            self.commands.register(id, move |workbench, _, _, cx| {
                if workbench.playground_active() {
                    let top = workbench.playground_scroll_top() + delta(workbench);
                    workbench.set_playground_scroll_top(top);
                    cx.notify();
                }
                Outcome::done()
            });
        }
    }

    /// The resource of `WalkThroughInput`: the module's file as a browser URI with the walkThrough scheme and the
    /// module id as its query; `authority` first, as `uri_parts` keys it.
    pub(super) fn playground_path(&self) -> PathBuf {
        let query = json!({ "moduleId": MODULE_ID });
        PathBuf::from(format!("//vscode-app{}/out/{MODULE_ID}.md?{query}", self.app_root.display()))
    }

    pub(super) fn is_playground(&self, editor: &OpenEditor) -> bool {
        editor.document().is(WALKTHROUGH_SCHEME, &self.playground_path())
    }

    /// The active tab is the Editor Playground (`activeEditorPane instanceof WalkThroughPart`).
    pub(super) fn playground_active(&self) -> bool {
        self.editors.active_tab().is_some_and(|tab| self.is_playground(tab))
    }

    pub(super) fn playground_open(&self) -> bool {
        self.editors.all_editors().any(|editor| self.is_playground(editor))
    }

    /// `interactivePlaygroundFocus`: the page has the focus, not one of its editors.
    pub(super) fn playground_focused(&self, window: &Window) -> bool {
        self.playground.focus.as_ref().is_some_and(|focus| focus.is_focused(window))
    }

    /// editorWalkThrough.ts `EditorWalkThroughAction`: opens the playground pinned, or activates it; the page takes
    /// the focus. A new one creates its snippet models and gets its last scroll position back.
    fn open_playground(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.playground_path();
        self.leave_locked_group(WALKTHROUGH_SCHEME, &path);
        match self.editors.editors.iter().position(|editor| editor.document().is(WALKTHROUGH_SCHEME, &path)) {
            Some(index) => self.editors.activate(index),
            None => {
                self.playground.expanded_content.take();
                let document = self.new_playground_document();
                self.editors.open(document, false);
            }
        }
        self.editors_changed(cx);
        self.focus_playground_page(window, cx);
    }

    /// A new `WalkThroughInput` (also `EditorWalkThroughInputSerializer.deserialize`): its snippet models, and the
    /// scroll position its editor memento kept.
    pub(super) fn new_playground_document(&mut self) -> TextDocument {
        let path = self.playground_path();
        self.create_snippet_models();
        let state = self.playground.view_states.load(&crate::editor::uri_string(WALKTHROUGH_SCHEME, &path));
        let top = state.map_or(0., |state| state["viewState"]["scrollTop"].as_f64().unwrap_or(0.) as f32);
        // Before the first layout there is no maximum to clamp to.
        self.playground.scroll.set_offset(point(px(0.), px(-top)));
        TextDocument::from_text(WALKTHROUGH_SCHEME, &path, "", "utf8".into(), None, &self.languages, &self.settings.lock().unwrap())
    }

    /// walkThroughContentProvider.ts `provideTextContent`: one model per code block, `<resource>#<n>.<lang>`, in the
    /// language its info string names (`getLanguageIdByLanguageName`).
    fn create_snippet_models(&mut self) {
        let page = self.playground_path().to_string_lossy().into_owned();
        for (index, (lang, text)) in snippets(&parse(CONTENT)).into_iter().enumerate() {
            let path = PathBuf::from(format!("{page}#{}.{lang}", index + 1));
            if self.editors.model(SNIPPET_SCHEME, &path).is_some() {
                continue;
            }
            let language = self.languages.id_by_name(&lang).unwrap_or("plaintext").to_owned();
            let document = TextDocument::from_text(SNIPPET_SCHEME, &path, &text, "utf8".into(), Some(&language), &self.languages, &self.settings.lock().unwrap());
            self.editors.open_model(document);
        }
    }

    /// The snippet models of the playground, in page order.
    fn snippet_models(&self) -> Vec<&OpenEditor> {
        snippet_models(&self.editors)
    }

    fn focus_playground_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focus = self.playground.focus.get_or_insert_with(|| cx.focus_handle()).clone();
        window.focus(&focus, cx);
        cx.notify();
    }

    /// A press on a snippet that is not the focused one: it becomes the group's code editor and takes the keyboard.
    fn focus_snippet(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.editors.active_tab().filter(|tab| self.is_playground(tab)).map(|tab| tab.model_id) else { return };
        self.editors.stacked_editors.insert(tab, crate::diff_editor::Side { scheme: SNIPPET_SCHEME.to_owned(), path });
        self.editors_changed(cx);
        window.focus(&self.editor_focus, cx);
        cx.notify();
    }

    /// `clearInput`: a closed playground saves its scroll position into the editor memento.
    pub(super) fn drop_closed_playground(&mut self) {
        let open = self.playground_open();
        if self.playground.was_open && !open {
            let resource = crate::editor::uri_string(WALKTHROUGH_SCHEME, &self.playground_path());
            let offset = self.playground.scroll.offset();
            self.playground.view_states.save(&resource, json!({ "viewState": { "scrollTop": -f32::from(offset.y), "scrollLeft": -f32::from(offset.x) } }));
            self.playground.revealed_cursor.set(None);
        }
        self.playground.was_open = open;
    }

    pub(super) fn load_playground_view_states(&mut self) {
        let stored = self.workspace_storage.lock().unwrap().get(VIEW_STATE_STORAGE_KEY).map(str::to_owned);
        self.playground.view_states = stored.map(|raw| ViewStates::from_memento_key(&raw, VIEW_STATE_MEMENTO_KEY)).unwrap_or_default();
    }

    /// `saveState` on shutdown: the open playground's scroll position, then the memento.
    pub(super) fn store_playground_view_states(&mut self) {
        if self.playground_open() {
            let resource = crate::editor::uri_string(WALKTHROUGH_SCHEME, &self.playground_path());
            let offset = self.playground.scroll.offset();
            self.playground.view_states.save(&resource, json!({ "viewState": { "scrollTop": -f32::from(offset.y), "scrollLeft": -f32::from(offset.x) } }));
        }
        let memento = self.playground.view_states.to_memento_key(VIEW_STATE_MEMENTO_KEY);
        self.workspace_storage.lock().unwrap().set(VIEW_STATE_STORAGE_KEY, memento);
    }

    fn playground_scroll_top(&self) -> f32 {
        -f32::from(self.playground.scroll.offset().y)
    }

    /// `scrollbar.setScrollPosition`, clamped to the content as the next layout would.
    fn set_playground_scroll_top(&self, top: f32) {
        let max = f32::from(self.playground.scroll.max_offset().y);
        let top = if max > 0. { top.clamp(0., max) } else { top.max(0.) };
        self.playground.scroll.set_offset(point(px(0.), px(-top)));
    }

    /// scrollableElement.ts `_onMouseWheel` of the page's `DomScrollableElement`: default sensitivities, no smooth scroll.
    fn playground_wheel(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let (x, y) = match event.delta {
            ScrollDelta::Pixels(delta) => (f32::from(delta.x) / 40., f32::from(delta.y) / 40.),
            ScrollDelta::Lines(delta) => (delta.x, delta.y),
        };
        let (_, delta_top) = wheel_scroll_delta(x, y, event.modifiers.alt, 1., 5.);
        self.set_playground_scroll_top(self.playground_scroll_top() + delta_top);
        cx.notify();
    }

    /// walkThroughPart.ts `getArrowScrollHeight`.
    fn playground_arrow_scroll_height(&self) -> f32 {
        let font_size = self.setting("editor.fontSize").as_f64().filter(|size| *size >= 1.).unwrap_or(12.) as f32;
        3. * font_size
    }

    /// The page's markdown after `expandMacros` and the platform's spans (`.mac-only` and the others,
    /// `multiCursorModifier`), as the reference draws it on macOS.
    /// `decorateContent` resolves the shortcuts once, in the context of the moment, and again when the keybindings
    /// change: the labels do not follow the focus.
    fn playground_markdown(&self, window: &Window) -> Vec<Block> {
        let generation = self.keybindings.generation;
        let mut expanded = self.playground.expanded_content.borrow_mut();
        if expanded.as_ref().is_none_or(|(resolved, _)| *resolved != generation) {
            let context = self.key_context(window);
            let resolver = &self.keybindings.resolver;
            let text = expand_macros(CONTENT, |command| resolver.lookup_primary_keybinding(command, &context).map(|item| crate::keybindings::label(&item.keybinding)));
            *expanded = Some((generation, text));
        }
        let text = &expanded.as_ref().expect("set above").1;
        // walkThroughPart.ts `multiCursorModifier`: `UILabelProvider.modifierLabels[OS]` of the add-cursor modifier.
        let modifier = match self.setting("editor.multiCursorModifier") == json!("ctrlCmd") {
            true => "\u{2318}",
            false => "\u{2325}",
        };
        parse(&expand_spans(text, modifier))
    }

    /// The page: `.walkThroughContent` in a scrollable element, the snippets' editors where their code blocks are.
    pub(super) fn playground_body(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let blocks = self.playground_markdown(window);
        let snippets = self.snippet_models();
        let mut next_snippet = 0;
        let mut children = Vec::new();
        for (index, block) in blocks.iter().enumerate() {
            children.push((self.page_block(&format!("playground-{index}"), block, &snippets, &mut next_snippet, window, cx), block_margins(block)));
        }
        self.reveal_playground_cursor(&blocks, &snippets);
        // The browser collapses the margins of neighbors; GPUI adds them, so each block gets the larger one above it.
        let mut page = div()
            .id("walkthrough-playground")
            .flex_1()
            .min_h_0()
            // Without it a flex item is as wide as its longest unwrapped paragraph.
            .min_w_0()
            .flex()
            .flex_col()
            // Hidden, not scroll: GPUI's own wheel handler moves 22px a line, the page 50.
            .overflow_hidden()
            .track_scroll(&self.playground.scroll)
            .on_scroll_wheel(cx.listener(|workbench, event: &ScrollWheelEvent, _, cx| workbench.playground_wheel(event, cx)))
            .px(px(20.))
            .py(px(10.))
            .font_family(UI_FONT)
            .text_size(px(13.))
            .line_height(px(22.))
            .text_color(self.color("foreground"))
            .on_mouse_down(MouseButton::Left, cx.listener(|workbench, _: &MouseDownEvent, window, cx| workbench.focus_playground_page(window, cx)))
            // The scroll handle's content starts at the first child: a 0 high one keeps the first block's margin in it.
            .child(div().flex_none());
        let mut margin_bottom = 0.;
        for (element, (margin_top, margin_below)) in children {
            page = page.child(div().flex_none().w_full().min_w_0().flex().flex_col().mt(px(f32::max(margin_top, margin_bottom))).child(element));
            margin_bottom = margin_below;
        }
        page = page.child(div().flex_none().h(px(margin_bottom)));
        if let Some(focus) = &self.playground.focus {
            page = page.track_focus(focus);
        }
        div().flex_1().min_h_0().flex().child(page)
    }

    /// `onDidChangeCursorPosition`: a new cursor position of the focused snippet scrolls the page the least that
    /// shows its line.
    fn reveal_playground_cursor(&self, blocks: &[Block], snippets: &[&OpenEditor]) {
        let Some(editor) = self.editors.active().filter(|editor| editor.document().scheme == SNIPPET_SCHEME) else { return };
        let view_line = editor.head_view_line(0);
        let cursor = Some((editor.model_id, view_line));
        if self.playground.revealed_cursor.replace(cursor) == cursor {
            return;
        }
        let Some(snippet) = snippets.iter().position(|snippet| std::ptr::eq(*snippet, editor)) else { return };
        let Some(child) = snippet_block_index(blocks, snippet) else { return };
        // Child 0 is the spacer before the blocks.
        let Some(bounds) = self.playground.scroll.bounds_for_item(child + 1) else { return };
        let container = self.playground.scroll.bounds();
        let viewport = self.editors.viewport;
        let editor_top = f32::from(bounds.top() - container.top());
        let line_top = editor_top + editor.line_top(view_line, viewport) - editor.scroll.top;
        let line_bottom = line_top + viewport.line_height;
        let (scroll_top, height) = (self.playground_scroll_top(), f32::from(container.size.height));
        if scroll_top > line_top {
            self.set_playground_scroll_top(line_top);
        } else if scroll_top < line_bottom - height {
            self.set_playground_scroll_top(line_bottom - height);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn page_block(&self, id: &str, block: &Block, snippets: &[&OpenEditor], next_snippet: &mut usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match block {
            Block::Paragraph(inline) => div().w_full().child(self.page_inline(id, inline, FontWeight::NORMAL, cx)).into_any_element(),
            Block::Heading(level, inline) => {
                let (size, line_height) = match level {
                    1 => (40., 48.),
                    2 => (30., 22.),
                    3 => (22., 22.),
                    _ => (12., 22.),
                };
                // walkThroughPart.css: h1 to h3 `font-weight: lighter`. shortcut: h4's uppercase is left out, the page has none.
                let weight = if *level < 4 { FontWeight::THIN } else { FontWeight::NORMAL };
                div().text_size(px(size)).line_height(px(line_height)).child(self.page_inline(id, inline, weight, cx)).into_any_element()
            }
            Block::Code { .. } => {
                let snippet = snippets.get(*next_snippet).copied();
                *next_snippet += 1;
                match snippet {
                    Some(editor) => self.snippet_editor(editor, window, cx).into_any_element(),
                    None => div().into_any_element(),
                }
            }
            Block::List { start, items } => {
                let mut list = div().w_full().flex().flex_col().pl(px(40.));
                for (index, item) in items.iter().enumerate() {
                    let marker = match start {
                        Some(first) => format!("{}.", first + index as u64),
                        None => "\u{2022}".to_owned(),
                    };
                    let mut content = div().w_full().min_w_0().flex().flex_col();
                    for (block_index, block) in item.iter().enumerate() {
                        // A tight item's text has no paragraph margins.
                        content = content.child(self.page_block(&format!("{id}-{index}-{block_index}"), block, snippets, next_snippet, window, cx));
                    }
                    let marker = div().absolute().left(px(-40.)).w(px(36.)).flex().justify_end().child(marker);
                    list = list.child(div().w_full().relative().child(marker).child(content));
                }
                list.into_any_element()
            }
            Block::Quote(blocks) => {
                let mut quote = div()
                    .ml(px(5.))
                    .mr(px(7.))
                    .pl(px(10.))
                    .pr(px(16.))
                    .border_l(px(5.))
                    .border_color(self.color("textBlockQuote.border"))
                    .bg(self.color("textBlockQuote.background"))
                    .flex()
                    .flex_col();
                for (index, inner) in blocks.iter().enumerate() {
                    if index > 0 {
                        quote = quote.child(div().h(px(f32::max(block_margins(&blocks[index - 1]).1, block_margins(inner).0))));
                    }
                    quote = quote.child(self.page_block(&format!("{id}-{index}"), inner, snippets, next_snippet, window, cx));
                }
                quote.into_any_element()
            }
            Block::Rule => div().h(px(2.)).border_b(px(2.)).border_color(self.color("foreground")).into_any_element(),
        }
    }

    /// `.walkThroughEditorContainer`: the snippet's editor `max(lineCount + 1, 4)` lines high, on
    /// `walkThrough.embeddedEditorBackground`. Another snippet's lines answer the mouse for the focused editor, so a
    /// cover takes its presses: it focuses the snippet and clicks there.
    fn snippet_editor(&self, editor: &OpenEditor, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let height = snippet_height(editor, self.editor_metrics(editor, window).line_height);
        let active = self.editors.active().is_some_and(|active| std::ptr::eq(active, editor));
        let path = editor.document().path.clone();
        div()
            .h(px(height))
            .flex()
            .overflow_hidden()
            .bg(self.color("walkThrough.embeddedEditorBackground"))
            .when(self.theme.kind.is_high_contrast(), |area| area.border_1().border_color(self.color("contrastBorder")))
            .relative()
            // The editor keeps its press; the page would take the focus from it.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(self.embedded_code_editor(editor, window, cx))
            .when(!active, |area| {
                let cover = div().absolute().inset_0().block_mouse_except_scroll().on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |workbench, event: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        workbench.focus_snippet(path.clone(), window, cx);
                        workbench.click_snippet_at(event, window, cx);
                    }),
                );
                area.child(cover)
            })
    }

    /// The focused snippet's press, as its line would take it; the snippet's box is where the page laid it out.
    fn click_snippet_at(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active().filter(|editor| editor.document().scheme == SNIPPET_SCHEME) else { return };
        let Some(snippet) = self.snippet_models().iter().position(|snippet| std::ptr::eq(*snippet, editor)) else { return };
        let Some(child) = snippet_block_index(&parse(CONTENT), snippet) else { return };
        // Child 0 is the spacer before the blocks; layout bounds leave out the scroll offset.
        let Some(bounds) = self.playground.scroll.bounds_for_item(child + 1) else { return };
        let origin = bounds.origin + self.playground.scroll.offset();
        let metrics = self.editor_metrics(editor, window);
        let last_line = editor.view_line_count() - 1;
        let view_line = ((f32::from(event.position.y - origin.y) + editor.scroll.top) / metrics.line_height).max(0.) as usize;
        let line = editor.model_line(view_line.min(last_line));
        let x = ((f32::from(event.position.x - origin.x) - metrics.content_left() + editor.scroll.left) / metrics.char_width).max(0.);
        let visible = editor.line_layout(line).model_visible(x);
        self.click_text(line, visible, event.modifiers, event.modifiers.shift, event.click_count, cx);
        cx.notify();
    }

    /// A paragraph's runs in `weight`: `code` and `.shortcut` in the monospace font in `textPreformat` colors, links in
    /// `textLink.foreground`; a `#heading` link scrolls the page to it, others open.
    fn page_inline(&self, id: &str, inline: &Inline, weight: FontWeight, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let foreground = self.color("foreground");
        let link_color = self.color("textLink.foreground");
        let (code_color, code_background) = (self.color("textPreformat.foreground"), self.color("textPreformat.background"));
        let mut runs = Vec::new();
        let mut links = Vec::new();
        let mut offset = 0;
        let font = gpui::Font { weight, ..gpui::font(UI_FONT) };
        let plain = |len: usize| TextRun { len, font: font.clone(), color: foreground.into(), background_color: None, underline: None, strikethrough: None };
        for span in &inline.spans {
            if span.range.start > offset {
                runs.push(plain(span.range.start - offset));
            }
            let mut run = plain(span.range.len());
            if span.code {
                run.font = gpui::font(MONOSPACE_FONT);
                run.color = code_color.into();
                run.background_color = Some(code_background.into());
            }
            if span.bold {
                run.font.weight = FontWeight::BOLD;
            }
            if span.italic {
                run.font.style = gpui::FontStyle::Italic;
            }
            if let Some(link) = &span.link {
                run.color = link_color.into();
                links.push((span.range.clone(), link.clone()));
            }
            runs.push(run);
            offset = span.range.end;
        }
        if inline.text.len() > offset {
            runs.push(plain(inline.text.len() - offset));
        }
        let ranges: Vec<_> = links.iter().map(|(range, _)| range.clone()).collect();
        let targets: Vec<String> = links.into_iter().map(|(_, target)| target).collect();
        let workbench = cx.entity().downgrade();
        InteractiveText::new(id.to_owned(), StyledText::new(inline.text.clone()).with_runs(runs)).on_click(ranges, move |index, window, cx| {
            let target = targets[index].clone();
            let _ = workbench.update(cx, |workbench, cx| workbench.open_playground_link(&target, window, cx));
        })
    }

    /// walkThroughPart.ts `registerClickHandler`: a same-page `#hash` scrolls its heading to 20px below the top.
    fn open_playground_link(&mut self, target: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(hash) = target.strip_prefix('#') else {
            self.open_markdown_link(target, window, cx);
            return;
        };
        let blocks = parse(CONTENT);
        let heading = blocks.iter().position(|block| matches!(block, Block::Heading(_, inline) if super::markdown_document::slugify(&inline.text) == hash));
        let Some(bounds) = heading.and_then(|index| self.playground.scroll.bounds_for_item(index + 1)) else { return };
        let container = self.playground.scroll.bounds();
        self.set_playground_scroll_top(f32::from(bounds.top() - container.top()) - 20.);
        cx.notify();
    }
}

/// Every snippet model, in page order (their fragments count from 1).
pub(crate) fn snippet_models(editors: &crate::editor::EditorGroup) -> Vec<&OpenEditor> {
    let mut snippets: Vec<(u32, &OpenEditor)> = Vec::new();
    for model in editors.models.iter().filter(|model| model.document().scheme == SNIPPET_SCHEME) {
        let path = model.document().path.to_string_lossy().into_owned();
        let index = path.rsplit_once('#').and_then(|(_, fragment)| fragment.split('.').next()?.parse().ok()).unwrap_or(0);
        snippets.push((index, model));
    }
    snippets.sort_by_key(|(index, _)| *index);
    snippets.into_iter().map(|(_, model)| model).collect()
}

/// Whether `tab` is an Editor Playground tab, for the editors the host sees.
pub(crate) fn is_playground_tab(tab: &OpenEditor) -> bool {
    let document = tab.document();
    document.scheme == WALKTHROUGH_SCHEME && crate::editor::split_query(WALKTHROUGH_SCHEME, &document.path).0.to_string_lossy().ends_with(&format!("{MODULE_ID}.md"))
}

fn snippet_height(editor: &OpenEditor, line_height: f32) -> f32 {
    (editor.document().lines.len() + 1).max(4) as f32 * line_height
}

/// The top-level block that holds code block `snippet` (0-based).
fn snippet_block_index(blocks: &[Block], snippet: usize) -> Option<usize> {
    let mut seen = 0;
    for (index, block) in blocks.iter().enumerate() {
        let count = count_code_blocks(block);
        if snippet < seen + count {
            return Some(index);
        }
        seen += count;
    }
    None
}

fn count_code_blocks(block: &Block) -> usize {
    match block {
        Block::Code { .. } => 1,
        Block::List { items, .. } => items.iter().flatten().map(count_code_blocks).sum(),
        Block::Quote(blocks) => blocks.iter().map(count_code_blocks).sum(),
        _ => 0,
    }
}

/// The CSS margins above and below a block (the browser's `p`, `ul`, `hr` and walkThroughPart.css headings); a
/// quote, without padding above or below, takes those of its first and last blocks, which collapse through it.
fn block_margins(block: &Block) -> (f32, f32) {
    match block {
        Block::Paragraph(_) | Block::List { .. } => (13., 13.),
        Block::Heading(1, _) => (20., 15.),
        Block::Heading(2, _) => (30., 10.),
        Block::Heading(3, _) => (20., 10.),
        Block::Heading(..) => (30., 10.),
        Block::Rule => (6.5, 6.5),
        Block::Code { .. } => (0., 0.),
        Block::Quote(blocks) => (blocks.first().map_or(0., |block| block_margins(block).0), blocks.last().map_or(0., |block| block_margins(block).1)),
    }
}

/// The code blocks' info strings and texts, in page order (walkThroughInput.ts `renderer.code`).
fn snippets(blocks: &[Block]) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for block in blocks {
        match block {
            Block::Code { language, text } => found.push((language.clone(), text.clone())),
            Block::List { items, .. } => found.extend(items.iter().flat_map(|item| snippets(item))),
            Block::Quote(blocks) => found.extend(snippets(blocks)),
            _ => {}
        }
    }
    found
}

/// marked with no HTML and untrusted links: `command:` links render as their text, as domSanitize leaves them.
fn parse(text: &str) -> Vec<Block> {
    crate::markdown::parse(&MarkdownString { value: text, is_trusted: &Value::Bool(false), support_theme_icons: false })
}

/// walkThroughPart.ts `expandMacros`: each `kb(command)` becomes its keybinding's label in a `.shortcut`, or
/// "unbound". The shortcut is a code span here, fenced so a backtick in the label stays.
fn expand_macros(text: &str, label: impl Fn(&str) -> Option<String>) -> String {
    let mut expanded = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("kb(") {
        let after = &rest[start + 3..];
        let command_len = after.find(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '-')).unwrap_or(after.len());
        if command_len == 0 || !after[command_len..].starts_with(')') {
            expanded.push_str(&rest[..start + 3]);
            rest = after;
            continue;
        }
        expanded.push_str(&rest[..start]);
        let shortcut = label(&after[..command_len]).unwrap_or_else(|| UNBOUND_COMMAND.to_owned());
        expanded.push_str(&format!("`` {shortcut} ``"));
        rest = &after[command_len + 1..];
    }
    expanded.push_str(rest);
    expanded
}

/// The page's `<span class="...">`s as walkThroughPart.css draws them on macOS: `.windows-only` and `.linux-only`
/// without `.mac-only` hidden, `.multi-cursor-modifier` filled with `modifier`, `.shortcut` as a code span.
fn expand_spans(text: &str, modifier: &str) -> String {
    const OPEN: &str = "<span class=\"";
    const CLOSE: &str = "</span>";
    let mut expanded = String::new();
    // Per open span: where its content starts in `expanded`, and its classes.
    let mut open: Vec<(usize, String)> = Vec::new();
    let mut rest = text;
    loop {
        let next_open = rest.find(OPEN);
        let next_close = rest.find(CLOSE);
        match (next_open, next_close) {
            (Some(start), close) if close.is_none_or(|close| start < close) => {
                expanded.push_str(&rest[..start]);
                let after = &rest[start + OPEN.len()..];
                let Some(end) = after.find("\">") else { break };
                open.push((expanded.len(), after[..end].to_owned()));
                rest = &after[end + 2..];
            }
            (_, Some(close)) => {
                expanded.push_str(&rest[..close]);
                rest = &rest[close + CLOSE.len()..];
                let Some((content_start, classes)) = open.pop() else { continue };
                let content = expanded.split_off(content_start);
                let classes: Vec<&str> = classes.split_whitespace().collect();
                let other_platform = classes.iter().any(|class| matches!(*class, "windows-only" | "linux-only"));
                if other_platform && !classes.contains(&"mac-only") {
                    continue;
                }
                let content = if classes.contains(&"multi-cursor-modifier") { modifier.to_owned() } else { content };
                match classes.contains(&"shortcut") && !content.starts_with('`') {
                    true => expanded.push_str(&format!("`` {content} ``")),
                    false => expanded.push_str(&content),
                }
            }
            _ => break,
        }
    }
    expanded.push_str(rest);
    expanded
}
