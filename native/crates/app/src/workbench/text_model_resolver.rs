//! textModelResolverService.ts `ResourceModelCollection.providers`: the text content providers extensions register
//! with `$registerTextContentProvider`, asked newest first for the text of a resource of their scheme. The model
//! lifetime of `ResourceModelCollection` is `EditorGroup::release_models`.

use serde_json::Value;

/// Handles and schemes, newest first.
#[derive(Default)]
pub struct ContentProviders(Vec<(Value, String)>);

impl ContentProviders {
    /// `registerTextModelContentProvider`: `unshift`, the newest provider of a scheme is asked first.
    pub fn register(&mut self, handle: Value, scheme: String) {
        self.0.insert(0, (handle, scheme));
    }

    pub fn unregister(&mut self, handle: &Value) {
        self.0.retain(|(registered, _)| registered != handle);
    }

    /// `hasTextModelContentProvider`.
    pub fn has(&self, scheme: &str) -> bool {
        self.0.iter().any(|(_, registered)| registered == scheme)
    }

    pub fn handles(&self, scheme: &str) -> Vec<Value> {
        self.0.iter().filter(|(_, registered)| registered == scheme).map(|(handle, _)| handle.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Instant;

    use exthost::scan::scan_extension;
    use gpui::{TestAppContext, VisualTestContext};
    use serde_json::{Value, json};

    use super::ContentProviders;
    use crate::editor::{EditReason, EditorGroup, HOST_REFERENCE_AGE, Position, TextDocument};
    use crate::host_sync::HostSync;
    use crate::languages::Languages;
    use crate::theme::load_theme;
    use crate::workbench::Workbench;

    fn document(scheme: &str, path: &str, text: &str) -> TextDocument {
        let mut document = TextDocument::untitled(Path::new(path), text, None, &Languages::new(&[]), &crate::default_settings().lock().unwrap());
        document.scheme = scheme.into();
        document
    }

    /// `createModelReference` from the extension host at `opened`: a model without an editor.
    fn referenced(group: &mut EditorGroup, document: TextDocument, opened: Instant) {
        let index = group.open_model(document);
        group.models[index].text_model_mut().host_reference = Some(opened);
    }

    fn text(group: &EditorGroup, scheme: &str, path: &str) -> Option<String> {
        group.model(scheme, Path::new(path)).map(|model| model.document().lines.join("\n"))
    }

    fn make_dirty(group: &mut EditorGroup, scheme: &str, path: &str) {
        let model = group.model_mut(scheme, Path::new(path)).unwrap();
        model.execute_edits(vec![(Position::new(0, 0), Position::new(0, 0), "make dirty ".into(), false)], None, EditReason::Unknown("test"));
    }

    fn workbench(cx: &mut TestAppContext) -> (gpui::Entity<Workbench>, &mut VisualTestContext) {
        let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
        let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let theme = load_theme(&[themes], "Dark Modern").unwrap();
        cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), crate::default_settings(), HostSync::new(std::sync::mpsc::channel().0), cx))
    }

    /// `$tryOpenDocument` with the text main_thread.rs got from a content provider, or `Null`.
    fn open(workbench: &gpui::Entity<Workbench>, uri: &Value, provided: Value, cx: &mut VisualTestContext) -> Result<Value, Value> {
        workbench.update(cx, |workbench, cx| workbench.try_open_document(uri, &Value::Null, &provided, false, cx))
    }

    #[test]
    fn providers_of_a_scheme_are_asked_newest_first() {
        let mut providers = ContentProviders::default();
        providers.register(json!(1), "test".into());
        providers.register(json!(2), "other".into());
        providers.register(json!(3), "test".into());
        assert_eq!(providers.handles("test"), [json!(3), json!(1)]);
        providers.unregister(&json!(3));
        assert_eq!(providers.handles("test"), [json!(1)]);
        providers.unregister(&json!(1));
        assert!(!providers.has("test"));
        assert!(providers.has("other"));
    }

    #[gpui::test]
    fn resolve_resource(cx: &mut TestAppContext) {
        let (workbench, cx) = workbench(cx);
        let uri = json!({ "scheme": "test", "path": "/thePath" });
        assert_eq!(open(&workbench, &uri, json!("Hello Test"), cx), Ok(uri.clone()));
        let opened = Instant::now();
        workbench.update(cx, |workbench, _| {
            assert_eq!(text(&workbench.editors, "test", "/thePath").as_deref(), Some("Hello Test"));
            workbench.editors.release_models(opened + HOST_REFERENCE_AGE);
            assert_eq!(text(&workbench.editors, "test", "/thePath"), None);
        });
    }

    #[test]
    fn resolve_file() {
        let opened = Instant::now();
        let mut group = EditorGroup::default();
        referenced(&mut group, document("file", "/path/file_resolver.txt", "Hello Html"), opened);
        group.release_models(opened);
        assert_eq!(text(&group, "file", "/path/file_resolver.txt").as_deref(), Some("Hello Html"));
        group.release_models(opened + HOST_REFERENCE_AGE);
        assert_eq!(text(&group, "file", "/path/file_resolver.txt"), None);
    }

    #[test]
    fn resolved_dirty_file_eventually_disposes() {
        let mut group = EditorGroup::default();
        group.open(document("file", "/path/file_resolver.txt", "Hello Html"), false);
        make_dirty(&mut group, "file", "/path/file_resolver.txt");
        group.close(0);
        assert!(group.model("file", Path::new("/path/file_resolver.txt")).is_some(), "not disposed because model still dirty");
        group.model_mut("file", Path::new("/path/file_resolver.txt")).unwrap().undo();
        group.release_models(Instant::now());
        assert!(group.model("file", Path::new("/path/file_resolver.txt")).is_none(), "now disposed because model got reverted");
    }

    #[test]
    fn resolved_dirty_file_does_not_dispose_when_new_reference_created() {
        let opened = Instant::now();
        let released = opened + HOST_REFERENCE_AGE;
        let mut group = EditorGroup::default();
        referenced(&mut group, document("file", "/path/file_resolver.txt", "Hello Html"), opened);
        make_dirty(&mut group, "file", "/path/file_resolver.txt");
        group.release_models(released);
        assert!(group.model("file", Path::new("/path/file_resolver.txt")).is_some(), "not disposed because model still dirty");
        let model = group.model_mut("file", Path::new("/path/file_resolver.txt")).unwrap();
        model.text_model_mut().host_reference = Some(released);
        model.undo();
        group.release_models(released);
        assert!(group.model("file", Path::new("/path/file_resolver.txt")).is_some(), "not disposed because we got another ref meanwhile");
        group.release_models(released + HOST_REFERENCE_AGE);
        assert!(group.model("file", Path::new("/path/file_resolver.txt")).is_none(), "now disposed because last ref got disposed");
    }

    #[gpui::test]
    fn resolve_untitled(cx: &mut TestAppContext) {
        let (workbench, cx) = workbench(cx);
        let uri = json!({ "scheme": "untitled", "path": "Untitled-7" });
        workbench.update(cx, |workbench, _| workbench.editors.open_model(document("untitled", "Untitled-7", "")));
        assert_eq!(open(&workbench, &uri, Value::Null, cx), Ok(uri.clone()));
        let models = workbench.read_with(cx, |workbench, _| workbench.editors.models.len());
        assert_eq!(models, 1, "the untitled model is the one the reference resolves");
    }

    #[gpui::test]
    fn even_loading_documents_should_be_refcounted(cx: &mut TestAppContext) {
        let (workbench, cx) = workbench(cx);
        let uri = json!({ "scheme": "test", "path": "/thePath" });
        assert_eq!(open(&workbench, &uri, json!("Hello Test"), cx), Ok(uri.clone()));
        // main_thread.rs asks the provider only while the workbench has no model; the second open shares the first model.
        assert_eq!(open(&workbench, &uri, Value::Null, cx), Ok(uri.clone()));
        let (models, text) = workbench.read_with(cx, |workbench, _| (workbench.editors.models.len(), text(&workbench.editors, "test", "/thePath")));
        assert_eq!((models, text.as_deref()), (1, Some("Hello Test")), "they are the same model");
    }

    #[gpui::test]
    fn resolve_inmemory(cx: &mut TestAppContext) {
        let (workbench, cx) = workbench(cx);
        let uri = json!({ "scheme": "inmemory", "path": "/test/inMemoryDoc" });
        workbench.update(cx, |workbench, _| workbench.editors.open_model(document("inmemory", "/test/inMemoryDoc", "Hello InMemory")));
        assert_eq!(open(&workbench, &uri, Value::Null, cx), Ok(uri.clone()));
        let opened = Instant::now();
        workbench.update(cx, |workbench, _| {
            workbench.editors.release_models(opened);
            assert_eq!(text(&workbench.editors, "inmemory", "/test/inMemoryDoc").as_deref(), Some("Hello InMemory"), "not disposed before releasing the reference");
            workbench.editors.release_models(opened + HOST_REFERENCE_AGE);
            assert_eq!(text(&workbench.editors, "inmemory", "/test/inMemoryDoc"), None, "disposed after the reference is released");
        });
    }

    #[gpui::test]
    fn resolve_inmemory_throws_when_model_not_found(cx: &mut TestAppContext) {
        let (workbench, cx) = workbench(cx);
        let uri = json!({ "scheme": "inmemory", "path": "/test/nonExistent" });
        let error = open(&workbench, &uri, Value::Null, cx).unwrap_err();
        assert!(error["message"].as_str().unwrap().contains("Unable to resolve text model content for resource inmemory:/test/nonExistent"), "{error}");
    }

    #[test]
    fn resolve_inmemory_disposes_when_last_reference_released() {
        let mut group = EditorGroup::default();
        group.open(document("inmemory", "/test/inMemoryDispose", "Hello InMemory"), false);
        group.close(0);
        assert_eq!(text(&group, "inmemory", "/test/inMemoryDispose"), None);
    }

    #[test]
    fn resolve_inmemory_is_refcounted() {
        let opened = Instant::now();
        let mut group = EditorGroup::default();
        group.open(document("inmemory", "/test/inMemoryRefcount", "Hello InMemory"), false);
        group.editors[0].text_model_mut().host_reference = Some(opened);
        group.close(0);
        assert!(group.model("inmemory", Path::new("/test/inMemoryRefcount")).is_some(), "should not dispose while ref2 is still alive");
        group.release_models(opened + HOST_REFERENCE_AGE);
        assert!(group.model("inmemory", Path::new("/test/inMemoryRefcount")).is_none(), "should dispose after last reference released");
    }

    #[test]
    fn resolve_inmemory_reuses_model_when_re_acquired_during_dispose() {
        let opened = Instant::now();
        let mut group = EditorGroup::default();
        group.open(document("inmemory", "/test/inMemoryReuse", "Hello Reuse"), false);
        let model1 = group.editors[0].model_id;
        // The host acquires the model before the editor releases it.
        group.editors[0].text_model_mut().host_reference = Some(opened);
        group.close(0);
        group.show_model(0, false);
        assert_eq!(group.editors[0].model_id, model1, "should reuse the same model instance");
        assert_eq!(text(&group, "inmemory", "/test/inMemoryReuse").as_deref(), Some("Hello Reuse"));
    }
}
