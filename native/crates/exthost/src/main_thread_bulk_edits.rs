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
        Some(Arg::Json(value) | Arg::JsonWithBuffers { json: value, .. }) => value.clone(),
        _ => Value::Null,
    };
    let undo_redo_group_id = match args.get(1) {
        Some(Arg::Json(value)) => value.as_u64(),
        _ => None,
    };
    let applied = match apply(&mut Providers { session }, &dto, undo_redo_group_id) {
        Ok(()) => true,
        Err(error) => {
            eprintln!("IGNORING workspace edit: {error}");
            false
        }
    };
    Message::ReplyOk { req, value: Reply::Json(json!(applied)) }
}

/// `BulkEdit.perform`: consecutive file edits run as one `BulkFileEdits`, in the order the workspace edit lists them.
pub(crate) fn apply(fs: &mut Providers, dto: &Value, undo_redo_group_id: Option<u64>) -> files::Result<()> {
    let mut file_edits = Vec::new();
    for edit in dto["edits"].as_array().into_iter().flatten() {
        if edit.get("oldResource").is_none() && edit.get("newResource").is_none() {
            operations::apply(fs, std::mem::take(&mut file_edits), undo_redo_group_id)?;
            // Text and notebook cell edits need the text models of milestone 8 (port/roadmap.md); until then they fail here.
            return Err(files::plain_error("text and cell edits are not supported yet"));
        }
        file_edits.push(file_edit(edit)?);
    }
    // The bulk edit undo element waits for the workbench undo stack of the Explorer operations (port/notes.md).
    operations::apply(fs, file_edits, undo_redo_group_id)?;
    Ok(())
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
