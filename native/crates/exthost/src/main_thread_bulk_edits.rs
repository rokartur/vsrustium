//! MainThreadBulkEdits (workbench/api/browser/mainThreadBulkEdits.ts): `workspace.applyEdit` over the bulk edit
//! service (contrib/bulkEdit/browser/bulkEditService.ts and bulkFileEdits.ts).

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use files::operations::{self, FileEdit, FileEditOptions};
use serde_json::{Value, json};

use crate::main_thread_files::{Providers, resource};
use crate::rpc::{Arg, Message, Reply};
use crate::session::Session;

/// `$tryApplyWorkspaceEdit`: `false` with a warning when an edit fails.
pub fn serve(session: &mut Session, req: u32, args: &[Arg]) -> Message {
    let dto = match args.first() {
        Some(Arg::Json(value)) => value.clone(),
        // Cell edits carry their outputs' bytes as buffers.
        Some(Arg::JsonWithBuffers { json, buffers }) => {
            let mut value = json.clone();
            crate::session::restore_buffer_refs(&mut value, buffers);
            value
        }
        _ => Value::Null,
    };
    let undo_redo_group_id = match args.get(1) {
        Some(Arg::Json(value)) => value.as_u64(),
        _ => None,
    };
    let needs_confirmation = dto["edits"].as_array().is_some_and(|edits| edits.iter().any(|edit| edit["metadata"]["needsConfirmation"] == true));
    let dto = match needs_confirmation {
        true => preview(session, &dto),
        false => dto,
    };
    let edits = dto["edits"].as_array().map_or(0, Vec::len);
    let applied = match apply(&mut Providers { session }, &dto, undo_redo_group_id) {
        Ok(()) => edits > 0,
        Err(error) => {
            eprintln!("IGNORING workspace edit: {error}");
            false
        }
    };
    // The model changes the workbench queued reach the host before the reply, as the model events fire before the edit resolves.
    session.flush_main_thread();
    Message::ReplyOk { req, value: Reply::Json(json!(applied)) }
}

/// bulkEditService.ts `_previewHandler`: the workbench's Refactor Preview answers the checked edits, none on Discard.
/// Extension tests show no preview.
pub(crate) fn preview(session: &mut Session, dto: &Value) -> Value {
    if session.runs_extension_tests() {
        return dto.clone();
    }
    match session.ask_workbench("workbench", "previewWorkspaceEdit", vec![dto.clone()]) {
        Some(Ok(checked)) => checked,
        _ => json!({ "edits": [] }),
    }
}

/// `BulkEdit.perform`: consecutive edits of one kind run together, in the order the workspace edit lists them.
pub(crate) fn apply(fs: &mut Providers, dto: &Value, undo_redo_group_id: Option<u64>) -> files::Result<()> {
    let edits = dto["edits"].as_array().map(Vec::as_slice).unwrap_or_default();
    for group in edits.chunk_by(|a, b| kind(a) == kind(b)) {
        match kind(&group[0]) {
            // The bulk edit undo element waits for the workbench undo stack of the Explorer operations (port/notes.md).
            EditKind::File => _ = operations::apply(fs, group.iter().map(file_edit).collect::<files::Result<_>>()?, undo_redo_group_id)?,
            EditKind::Text => text_edits(fs, group)?,
            EditKind::Cell => cell_edits(fs, group)?,
        }
    }
    Ok(())
}

#[derive(PartialEq)]
enum EditKind {
    File,
    Text,
    Cell,
}

fn kind(edit: &Value) -> EditKind {
    if edit.get("oldResource").is_some() || edit.get("newResource").is_some() {
        EditKind::File
    } else if edit.get("textEdit").is_some() {
        EditKind::Text
    } else {
        EditKind::Cell
    }
}

/// `BulkTextEdits.apply` runs in the workbench, which has the text models; the session reads what an extension's file
/// system provider has, since the GPUI thread cannot call the extension host.
fn text_edits(fs: &mut Providers, group: &[Value]) -> files::Result<()> {
    let mut provider_files: Vec<Value> = Vec::new();
    for edit in group {
        let uri = &edit["resource"];
        if uri["scheme"] == "file" || provider_files.iter().any(|file| file["uri"] == *uri) {
            continue;
        }
        if let Ok(bytes) = files::read_file(fs, &resource(uri)) {
            provider_files.push(json!({ "uri": uri, "bytes": bytes }));
        }
    }
    match fs.session.ask_workbench("workbench", "applyTextEdits", vec![json!(group), json!(provider_files)]) {
        Some(Ok(_)) => Ok(()),
        Some(Err(error)) => Err(files::plain_error(error["message"].as_str().unwrap_or_default())),
        None => Err(files::plain_error("Canceled")),
    }
}

/// bulkCellEdits.ts `BulkCellEdits.apply` runs in the workbench, which has the notebook models.
fn cell_edits(fs: &mut Providers, group: &[Value]) -> files::Result<()> {
    match fs.session.ask_workbench("workbench", "applyCellEdits", vec![json!(group)]) {
        Some(Ok(_)) => Ok(()),
        Some(Err(error)) => Err(files::plain_error(error["message"].as_str().unwrap_or_default())),
        None => Err(files::plain_error("Canceled")),
    }
}

/// `reviveWorkspaceEditDto` and `BulkFileEdits.apply` of one `IWorkspaceFileEditDto`.
fn file_edit(edit: &Value) -> files::Result<FileEdit> {
    let raw = &edit["options"];
    let flag = |key: &str| raw[key].as_bool();
    let options = FileEditOptions {
        overwrite: flag("overwrite"),
        ignore_if_not_exists: flag("ignoreIfNotExists"),
        ignore_if_exists: flag("ignoreIfExists"),
        recursive: flag("recursive"),
        copy: flag("copy"),
        folder: flag("folder"),
        skip_trash_bin: flag("skipTrashBin"),
        max_size: raw["maxSize"].as_f64(),
    };
    let old = edit.get("oldResource").map(resource);
    let new = edit.get("newResource").map(resource);
    Ok(match (old, new) {
        (Some(old), Some(new)) if options.copy == Some(true) => FileEdit::Copy { old, new, options },
        (Some(old), Some(new)) => FileEdit::Rename { old, new, options },
        (Some(old), None) => FileEdit::Delete { old, options, undoes_create: false },
        (None, Some(new)) => FileEdit::Create { new, options, contents: contents(&raw["contents"])? },
        (None, None) => unreachable!("apply passes only file edits"),
    })
}

/// `options.contents`: base64 bytes; a data transfer file has no resolver outside drop and paste.
fn contents(contents: &Value) -> files::Result<Option<Vec<u8>>> {
    if contents.is_null() {
        return Ok(None);
    }
    if contents["type"] != "base64" {
        return Err(files::plain_error("Could not revive data transfer file"));
    }
    let value = contents["value"].as_str().unwrap_or_default();
    STANDARD.decode(value).map(Some).map_err(|error| files::plain_error(error.to_string()))
}
