use super::*;
use serde_json::json;

fn link(path: &str, range: [usize; 4]) -> Location {
    let range = json!({ "startLineNumber": range[0], "startColumn": range[1], "endLineNumber": range[2], "endColumn": range[3] });
    Location::from_dto(&json!({ "uri": { "scheme": "file", "path": path }, "range": range })).unwrap()
}

fn path(model: &ReferencesModel, index: usize) -> &str {
    model.references[index].location.resource["path"].as_str().unwrap()
}

#[test]
fn nearest_reference() {
    let model = ReferencesModel::new(vec![link("/out/obj/can", [1, 1, 1, 1]), link("/out/obj/can2", [1, 1, 1, 1]), link("/src/can", [1, 1, 1, 1])], "FOO");
    let nearest = |uri: &str| path(&model, model.nearest_reference(uri, (1, 1)).unwrap()).to_string();
    assert_eq!(nearest("file:///src/can"), "/src/can");
    assert_eq!(nearest("file:///src/someOtherFileInSrc"), "/src/can");
    assert_eq!(nearest("file:///out/someOtherFile"), "/out/obj/can");
    assert_eq!(nearest("file:///out/obj/can2222"), "/out/obj/can2");
}

#[test]
fn sorts_groups_and_drops_duplicates_keeping_the_providers_first() {
    let model = ReferencesModel::new(vec![link("/b", [3, 1, 3, 4]), link("/a", [2, 1, 2, 2]), link("/b", [1, 1, 1, 2]), link("/b", [3, 1, 3, 4])], "References");
    assert_eq!(model.groups.iter().map(|group| group.uri.as_str()).collect::<Vec<_>>(), ["file:///a", "file:///b"]);
    assert_eq!(model.references.len(), 3);
    assert_eq!(model.groups[1].children, [1, 2]);
    assert_eq!(model.first_reference(), Some(2));
    assert_eq!(model.reference_at("file:///b", (3, 2)), Some(2));
    assert_eq!(model.reference_at("file:///b", (2, 1)), None);
}

#[test]
fn next_and_previous_cycle_in_the_file_then_cross_files() {
    let model = ReferencesModel::new(vec![link("/a", [1, 1, 1, 2]), link("/a", [2, 1, 2, 2]), link("/b", [1, 1, 1, 2])], "References");
    assert_eq!(model.next_or_previous(0, true), 1);
    assert_eq!(model.next_or_previous(1, true), 2);
    assert_eq!(model.next_or_previous(2, true), 0);
    assert_eq!(model.next_or_previous(0, false), 2);
    assert_eq!(model.next_or_previous(2, false), 1);
}
