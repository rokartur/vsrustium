use serde_json::{Value, json};

use super::*;

fn file_uri(path: &str) -> Value {
    json!({ "$mid": 1, "scheme": "file", "authority": "", "path": path, "query": "", "fragment": "" })
}

fn code_cell(source: &str) -> Value {
    json!({ "cellKind": CODE, "language": "typescript", "source": source, "outputs": [], "metadata": {} })
}

fn notebook(sources: &[&str]) -> NotebookModel {
    let cells: Vec<Value> = sources.iter().map(|source| code_cell(source)).collect();
    NotebookModel::new(file_uri("/w/a.nb"), "test", &json!({ "metadata": {}, "cells": cells }), json!({})).0
}

fn texts(model: &NotebookModel, all: &[NewCellText]) -> Vec<String> {
    model.cells.iter().map(|cell| all.iter().find(|text| text.uri == cell.uri).expect("created").source.clone()).collect()
}

#[test]
fn cell_uris_encode_the_handle_in_base_7_and_the_scheme() {
    let uri = cell_uri(&file_uri("/w/a.nb"), 0);
    assert_eq!(uri["scheme"], CELL_SCHEME);
    assert_eq!(uri["fragment"], "W0sZmlsZQ==");
    assert_eq!(cell_uri(&file_uri("/w/a.nb"), 7)["fragment"], "X10sZmlsZQ==");
    assert_eq!(parse_cell_uri(&cell_uri(&file_uri("/w/a.nb"), 50)), Some((file_uri("/w/a.nb"), 50)));
}

#[test]
fn edits_apply_from_the_last_index_and_send_one_event() {
    let (mut model, mut created) = {
        let cells = vec![code_cell("a"), code_cell("b"), code_cell("c")];
        NotebookModel::new(file_uri("/w/a.nb"), "test", &json!({ "cells": cells }), json!({}))
    };
    let edits = [json!({ "editType": 1, "index": 0, "count": 1, "cells": [] }), json!({ "editType": 1, "index": 2, "count": 1, "cells": [] })];
    let applied = model.apply_edits(&edits).unwrap();
    created.extend(applied.added);
    assert_eq!(texts(&model, &created), ["b"]);
    assert_eq!(applied.removed.len(), 2);
    let event = applied.event.unwrap();
    assert_eq!(event["versionId"], 1);
    assert_eq!(event["rawEvents"][0]["changes"][0][0], 2);
    assert_eq!(event["rawEvents"][1]["changes"][0][0], 0);
    assert!(model.dirty);
}

#[test]
fn inserted_cells_get_new_handles_and_their_text() {
    let mut model = notebook(&["a"]);
    let applied = model.apply_edits(&[json!({ "editType": 1, "index": 0, "count": 0, "cells": [code_cell("x"), code_cell("y")] })]).unwrap();
    assert_eq!(model.cells.iter().map(|cell| cell.handle).collect::<Vec<_>>(), [1, 2, 0]);
    assert_eq!(applied.added.iter().map(|text| text.source.as_str()).collect::<Vec<_>>(), ["x", "y"]);
    let dto = &applied.event.unwrap()["rawEvents"][0]["changes"][0][2][0];
    assert_eq!(dto["source"], json!(["x"]));
    assert_eq!(dto["handle"], 1);
}

#[test]
fn transient_changes_leave_the_notebook_clean() {
    let cells = vec![code_cell("a")];
    let (mut model, _) = NotebookModel::new(file_uri("/w/a.nb"), "test", &json!({ "cells": cells }), json!({ "transientOutputs": true, "transientCellMetadata": { "trusted": true } }));
    let output = json!({ "outputId": "o1", "items": [{ "mime": "text/plain", "valueBytes": [104, 105] }] });
    let applied = model.apply_edits(&[json!({ "editType": 2, "index": 0, "outputs": [output] })]).unwrap();
    assert_eq!(applied.event.unwrap()["rawEvents"][0]["outputs"][0]["items"][0]["valueBytes"], json!({ "$vsbuffer": "aGk=" }));
    model.apply_edits(&[json!({ "editType": 8, "index": 0, "metadata": { "trusted": true } })]).unwrap();
    model.apply_edits(&[json!({ "editType": 9, "handle": 0, "internalMetadata": { "executionOrder": 1 } })]).unwrap();
    assert!(!model.dirty);
    assert_eq!(model.version_id, 3);
    model.apply_edits(&[json!({ "editType": 8, "index": 0, "metadata": { "inputCollapsed": true } })]).unwrap();
    assert!(model.dirty);
    assert_eq!(model.cells[0].metadata, json!({ "trusted": true, "inputCollapsed": true }));
}

#[test]
fn output_items_find_their_cell_by_output_id() {
    let mut model = notebook(&["a", "b"]);
    let output = json!({ "outputId": "o1", "items": [{ "mime": "text/plain", "valueBytes": [1] }] });
    model.apply_edits(&[json!({ "editType": 2, "handle": 1, "outputs": [output] })]).unwrap();
    let item = json!({ "mime": "text/plain", "valueBytes": { "$vsbuffer": "Ag==" } });
    let applied = model.apply_edits(&[json!({ "editType": 7, "outputId": "o1", "append": true, "items": [item] })]).unwrap();
    assert_eq!(model.cells[1].outputs[0].items.iter().map(|item| item.data.clone()).collect::<Vec<_>>(), [vec![1], vec![2]]);
    assert_eq!(applied.event.unwrap()["rawEvents"][0]["index"], 1);
    // An output no cell has any more drops the edit.
    let applied = model.apply_edits(&[json!({ "editType": 7, "outputId": "gone", "items": [] })]).unwrap();
    assert!(applied.event.is_none());
}

#[test]
fn snapshots_leave_out_transient_metadata() {
    let cells = vec![json!({ "cellKind": CODE, "language": "python", "source": "x", "outputs": [], "metadata": { "keep": 1, "drop": 2 } })];
    let (model, _) = NotebookModel::new(file_uri("/w/a.nb"), "test", &json!({ "cells": cells, "metadata": { "doc": 1 } }), json!({ "transientCellMetadata": { "drop": true } }));
    let snapshot = model.snapshot(|_| "x2".into());
    assert_eq!(snapshot["cells"][0]["metadata"], json!({ "keep": 1 }));
    assert_eq!(snapshot["cells"][0]["source"], "x2");
    assert_eq!(snapshot["metadata"], json!({ "doc": 1 }));
}
