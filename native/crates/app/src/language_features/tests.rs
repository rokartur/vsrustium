use super::*;
use serde_json::json;

fn file(path: &str) -> Value {
    json!({ "scheme": "file", "authority": "", "path": path })
}

#[test]
fn score_matches_language_selector_ts() {
    let uri = file("/testbed/file.fb");
    let score = |selector: Value| score(&selector, &uri, "farboo", true);
    assert_eq!(score(json!({})), 0);
    assert_eq!(score(json!(null)), 0);
    assert_eq!(score(json!("")), 0);
    assert_eq!(score(json!({ "language": "*" })), 5);
    assert_eq!(score(json!("*")), 5);
    assert_eq!(score(json!("farboo")), 10);
    assert_eq!(score(json!({ "language": "farboo", "scheme": "" })), 10);
    assert_eq!(score(json!({ "language": "farboo", "scheme": "*" })), 10);
    assert_eq!(score(json!({ "scheme": "*" })), 5);
    assert_eq!(score(json!({ "language": "farboo", "scheme": "http" })), 0);
    assert_eq!(score(json!({ "pattern": "**/*.fb" })), 10);
    assert_eq!(score(json!({ "pattern": "**/*.fb", "scheme": "file" })), 10);
    assert_eq!(score(json!({ "pattern": "**/*.ts" })), 0);
    assert_eq!(score(json!(["fooLang", "*"])), 5);
    assert_eq!(score(json!(["*", "farboo"])), 10);
    assert_eq!(score(json!({ "language": "farboo", "notebookType": "*" })), 0);
    let relative = json!({ "pattern": { "baseUri": file("/home/user/Desktop"), "pattern": "*.json" } });
    assert_eq!(super::score(&relative, &file("/home/user/Desktop/test.json"), "json", true), 10);
}

#[test]
fn score_needs_access_to_all_models_for_unsynchronized_documents() {
    let uri = file("/my/file.js");
    assert_eq!(score(&json!("javascript"), &uri, "javascript", false), 0);
    assert_eq!(score(&json!({ "language": "javascript", "scheme": "file", "hasAccessToAllModels": true }), &uri, "javascript", false), 10);
    assert_eq!(score(&json!(["fooLang", "*", { "language": "*", "hasAccessToAllModels": true }]), &uri, "javascript", false), 5);
}

#[test]
fn ordered_sorts_by_score_then_builtin_then_newest() {
    let mut features = LanguageFeatures::default();
    features.register("$registerHoverProvider", vec![json!(1), json!([{ "language": "*" }])]);
    features.register("$registerHoverProvider", vec![json!(2), json!([{ "language": "typescript", "isBuiltin": true }])]);
    features.register("$registerHoverProvider", vec![json!(3), json!([{ "language": "typescript" }])]);
    features.register("$registerHoverProvider", vec![json!(4), json!([{ "language": "typescript" }])]);
    features.register("$registerHoverProvider", vec![json!(5), json!([{ "language": "markdown" }])]);
    features.register("$registerDocumentHighlightProvider", vec![json!(6), json!([{ "language": "typescript" }])]);
    let uri = file("/src/main.ts");
    let candidate = Candidate { uri: &uri, language: "typescript" };
    let handles = |features: &LanguageFeatures| features.ordered("$registerHoverProvider", &candidate).iter().map(|registration| registration.handle).collect::<Vec<_>>();
    assert_eq!(handles(&features), [4, 3, 2, 1]);
    assert_eq!(features.unregister(4).as_deref(), Some("$registerHoverProvider"));
    assert_eq!(features.unregister(4), None);
    assert_eq!(handles(&features), [3, 2, 1]);
    features.register("$registerHoverProvider", vec![json!(7), json!([{ "language": "typescript", "exclusive": true }])]);
    assert_eq!(handles(&features), [7]);
}
