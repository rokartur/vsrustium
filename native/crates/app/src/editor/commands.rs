//! Editor commands by their upstream id (coreCommands.ts and the editor contributions).

use super::{OpenEditor, Reveal, Viewport};

/// The ids `run` knows: multicursor.ts, coreCommands.ts and folding.ts actions. Their keys and `when` clauses
/// are upstream's default keybindings (keybindings/default-macos.json).
pub const IDS: &[&str] = &[
    "editor.action.insertCursorAbove",
    "editor.action.insertCursorBelow",
    "editor.action.insertCursorAtEndOfEachLineSelected",
    "editor.action.addCursorsToBottom",
    "editor.action.addCursorsToTop",
    "removeSecondaryCursors",
    "cancelSelection",
    "cursorColumnSelectLeft",
    "cursorColumnSelectRight",
    "cursorColumnSelectUp",
    "cursorColumnSelectDown",
    "cursorColumnSelectPageUp",
    "cursorColumnSelectPageDown",
    "editor.unfold",
    "editor.unfoldRecursively",
    "editor.fold",
    "editor.foldRecursively",
    "editor.toggleFold",
    "editor.toggleFoldRecursively",
    "editor.foldAll",
    "editor.unfoldAll",
    "editor.foldAllExcept",
    "editor.unfoldAllExcept",
    "editor.foldAllBlockComments",
    "editor.foldAllMarkerRegions",
    "editor.unfoldAllMarkerRegions",
    "editor.gotoParentFold",
    "editor.gotoPreviousFold",
    "editor.gotoNextFold",
    "editor.createFoldingRangeFromSelection",
    "editor.removeManualFoldingRanges",
    "editor.toggleImportFold",
    "editor.foldLevel1",
    "editor.foldLevel2",
    "editor.foldLevel3",
    "editor.foldLevel4",
    "editor.foldLevel5",
    "editor.foldLevel6",
    "editor.foldLevel7",
];

/// Runs `command` on the editor and says what to reveal, or `None` when its precondition does not hold
/// or the id is unknown.
pub fn run(editor: &mut OpenEditor, command: &str, viewport: Viewport) -> Option<Reveal> {
    let page_size = viewport.page_size();
    Some(match command {
        "editor.action.insertCursorAbove" => editor.insert_cursors_vertically(false),
        "editor.action.insertCursorBelow" => editor.insert_cursors_vertically(true),
        "editor.action.insertCursorAtEndOfEachLineSelected" => editor.insert_cursors_at_line_ends(),
        "editor.action.addCursorsToBottom" => editor.add_cursors_to_edge(true),
        "editor.action.addCursorsToTop" => editor.add_cursors_to_edge(false),
        "removeSecondaryCursors" => return editor.remove_secondary_cursors(),
        "cancelSelection" => return editor.cancel_selection(),
        "cursorColumnSelectLeft" => editor.column_select_horizontally(false),
        "cursorColumnSelectRight" => editor.column_select_horizontally(true),
        "cursorColumnSelectUp" => editor.column_select_vertically(false, 1),
        "cursorColumnSelectDown" => editor.column_select_vertically(true, 1),
        "cursorColumnSelectPageUp" => editor.column_select_vertically(false, page_size),
        "cursorColumnSelectPageDown" => editor.column_select_vertically(true, page_size),
        _ => return editor.run_folding_command(command),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::{EditorGroup, TextDocument};

    #[test]
    fn run_knows_every_registered_id() {
        let languages = crate::languages::Languages::new(&[]);
        let document = TextDocument::from_bytes("file", std::path::Path::new("/a.txt"), b"a\n  b\nc", &languages, &crate::default_settings().lock().unwrap());
        let mut group = EditorGroup::default();
        group.open(document, false);
        let viewport = group.viewport;
        let editor = group.active_mut().unwrap();
        for id in IDS {
            // An unknown id and a failed precondition both answer None; the folding ids always answer.
            if id.starts_with("editor.") && !id.starts_with("editor.action.") {
                assert!(run(editor, id, viewport).is_some(), "{id}");
            }
        }
    }
}
