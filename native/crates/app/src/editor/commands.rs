//! Editor commands by their upstream id, and the macOS default keys that run them (the `primary`/`mac`
//! keybindings of coreCommands.ts and the editor contributions).

use super::{OpenEditor, Reveal, Viewport};

/// Key, as GPUI writes a keystroke (`Keystroke::unparse`), and command id. Several commands on one key are
/// tried in order of their keybinding weight; the first whose precondition holds runs.
pub const KEYBINDINGS: &[(&str, &str)] = &[
    ("alt-cmd-up", "editor.action.insertCursorAbove"),
    ("alt-cmd-down", "editor.action.insertCursorBelow"),
    ("alt-shift-i", "editor.action.insertCursorAtEndOfEachLineSelected"),
    ("escape", "removeSecondaryCursors"),
    ("shift-escape", "removeSecondaryCursors"),
    ("escape", "cancelSelection"),
    ("shift-escape", "cancelSelection"),
    ("alt-cmd-shift-left", "cursorColumnSelectLeft"),
    ("alt-cmd-shift-right", "cursorColumnSelectRight"),
    ("alt-cmd-shift-up", "cursorColumnSelectUp"),
    ("alt-cmd-shift-down", "cursorColumnSelectDown"),
    ("alt-cmd-shift-pageup", "cursorColumnSelectPageUp"),
    ("alt-cmd-shift-pagedown", "cursorColumnSelectPageDown"),
];

/// The commands bound to `keystroke`, in the order they are tried.
pub fn for_key(keystroke: &str) -> impl Iterator<Item = &'static str> + '_ {
    KEYBINDINGS.iter().filter(move |(key, _)| *key == keystroke).map(|(_, command)| *command)
}

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
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_is_written_as_gpui_writes_a_keystroke() {
        for (key, command) in KEYBINDINGS {
            assert_eq!(gpui::Keystroke::parse(key).expect("a valid keystroke").unparse(), *key, "{command}");
        }
    }

    #[test]
    fn escape_tries_removing_secondary_cursors_before_cancelling_the_selection() {
        assert_eq!(for_key("escape").collect::<Vec<_>>(), ["removeSecondaryCursors", "cancelSelection"]);
    }
}
