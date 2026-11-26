//! stickyScrollProvider.ts `StickyLineCandidateProvider.update` with the outline provider of
//! stickyScrollModelProvider.ts: the active editor's sticky scopes come from its document symbols, and only an
//! outline without symbols leaves them to the folding regions (`OpenEditor::sticky_model`).

use std::time::Duration;

use config::configuration::Overrides;
use gpui::Context;
use serde_json::json;

use super::Workbench;
use crate::editor::sticky_scroll::from_outline_model;

/// `StickyModelProvider._updateScheduler`, a `Delayer(300)`.
const UPDATE_DELAY: Duration = Duration::from_millis(300);
/// `StickyLineCandidateProvider._updateSoon`: content changes wait 50ms before the update.
const CONTENT_CHANGE_DELAY: Duration = Duration::from_millis(50);

#[derive(Default)]
pub(super) struct StickyOutline {
    /// Each update's number: an older outline answer is dropped (`_cts.dispose(true)`).
    generation: u64,
    /// The model id and version of the active editor last updated for.
    seen: Option<(u32, u32)>,
}

impl Workbench {
    /// `onDidChangeModel` and `onDidChangeModelContent` of the active editor.
    pub(super) fn sticky_outline_frame(&mut self, cx: &mut Context<Self>) {
        let active = self.editors.active().map(|editor| (editor.model_id, editor.text_model().version_id));
        if active.is_none() || active == self.sticky_outline.seen {
            return;
        }
        let same_model = self.sticky_outline.seen.map(|(model_id, _)| model_id) == active.map(|(model_id, _)| model_id);
        self.sticky_outline.seen = active;
        self.update_sticky_outline(if same_model { CONTENT_CHANGE_DELAY + UPDATE_DELAY } else { UPDATE_DELAY }, cx);
    }

    /// `documentSymbolProvider.onDidChange` and `editor.stickyScroll` changes.
    pub(super) fn sticky_outline_providers_changed(&mut self, cx: &mut Context<Self>) {
        self.update_sticky_outline(UPDATE_DELAY, cx);
    }

    fn update_sticky_outline(&mut self, delay: Duration, cx: &mut Context<Self>) {
        let Some(model_id) = self.editors.active().map(|editor| editor.model_id) else { return };
        self.sticky_outline.generation += 1;
        let generation = self.sticky_outline.generation;
        cx.spawn(async move |workbench, cx| {
            cx.background_executor().timer(delay).await;
            let _ = workbench.update(cx, |workbench, cx| workbench.compute_sticky_outline(generation, model_id, cx));
        })
        .detach();
    }

    /// `StickyModelFromCandidateOutlineProvider.computeStickyModel`, asked only with sticky scroll on, the
    /// `outlineModel` default model and a model small enough to tokenize.
    fn compute_sticky_outline(&mut self, generation: u64, model_id: u32, cx: &mut Context<Self>) {
        if generation != self.sticky_outline.generation {
            return;
        }
        let Some(editor) = self.editors.active().filter(|editor| editor.model_id == model_id) else { return };
        let document = editor.document();
        let overrides = Overrides { resource: Some(&document.path), override_identifier: Some(&document.language_id) };
        let settings = self.settings.lock().unwrap();
        let wanted = settings.get_value(Some("editor.stickyScroll.enabled"), &overrides) == Some(json!(true))
            && settings.get_value(Some("editor.stickyScroll.defaultModel"), &overrides).is_none_or(|model| model == json!("outlineModel"))
            && !document.too_large_for_tokenization;
        drop(settings);
        drop(document);
        if !wanted {
            self.editors.active_mut().expect("found above").sticky_outline = None;
            return;
        }
        let outline = self.outline_model(model_id, cx);
        cx.spawn(async move |workbench, cx| {
            let outline = outline.await;
            let _ = workbench.update(cx, |workbench, cx| {
                if generation != workbench.sticky_outline.generation {
                    return;
                }
                let Some(editor) = workbench.editors.active_mut().filter(|editor| editor.model_id == model_id) else { return };
                let preferred = editor.sticky_outline.as_ref().and_then(|(_, provider)| provider.clone());
                editor.sticky_outline = outline.and_then(|outline| from_outline_model(&outline, preferred.as_deref()));
                cx.notify();
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;
    use serde_json::json;

    use crate::editor::Viewport;
    use crate::workbench::goto_symbol::tests::{answer, open};

    /// The document symbols give the scopes; the flat text has no folding regions to fall back to.
    #[gpui::test]
    fn sticky_lines_come_from_the_document_symbols(cx: &mut TestAppContext) {
        let text = (1..=40).map(|line| format!("line {line}")).collect::<Vec<_>>().join("\n");
        let (workbench, cx, receiver, _) = open(cx, "sticky-outline", &text);
        let viewport = Viewport { width: 400., height: 360., char_width: 7.2, line_height: 18., sticky_scroll_lines: 5, top_whitespace: 0., stop_rendering_line_after: None };
        let sticky_lines = |cx: &mut gpui::VisualTestContext| {
            workbench.update(cx, |workbench, _| {
                let editor = workbench.editors.active_mut().unwrap();
                editor.set_scroll(10. * viewport.line_height, 0., viewport);
                editor.sticky_scroll_state(viewport).start_line_numbers
            })
        };
        assert_eq!(sticky_lines(cx), Vec::<usize>::new());
        workbench.update(cx, |workbench, cx| {
            workbench.language_features.register(crate::workbench::document_symbols::REGISTER, vec![json!(11), json!("*"), json!("Test")]);
            workbench.language_providers_changed(crate::workbench::document_symbols::REGISTER, cx);
        });
        cx.executor().advance_clock(super::UPDATE_DELAY);
        cx.run_until_parked();
        let lines = |start: usize, end: usize| json!({ "startLineNumber": start, "startColumn": 1, "endLineNumber": end, "endColumn": 1 });
        answer(&receiver, "$provideDocumentSymbols", json!([{ "name": "A", "detail": "", "kind": 5, "range": lines(1, 30), "selectionRange": lines(2, 2), "children": [] }]), cx);
        assert_eq!(sticky_lines(cx), vec![2]);
    }
}
