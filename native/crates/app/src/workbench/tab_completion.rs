//! tabCompletion.ts: with `editor.tabCompletion` `onlySnippets`, Tab inserts the snippet whose prefix ends the text
//! left of the cursor, or whose prefix the selection equals; two or more open the suggest widget with just them.

use gpui::Context;
use serde_json::{Value, json};

use super::Workbench;
use super::snippet_completions::snippet_language;
use super::snippet_controller::SnippetInsert;
use super::suggest_controller::TriggerOptions;
use crate::commands::Outcome;
use crate::editor::{OpenEditor, Position, utf16_column};
use crate::snippets::{Snippet, SnippetGetOptions, snippet_completion};

/// The handle `showSimpleSuggestions` gives the tab completion provider.
pub(super) const TAB_COMPLETION_PROVIDER: u64 = u64::MAX - 5;
/// `MAX_PREFIX_LENGTH` of `getNonWhitespacePrefix`.
const MAX_PREFIX_LENGTH: usize = 100;

impl Workbench {
    pub(super) fn register_tab_completion(&mut self) {
        self.commands.register("insertSnippet", |workbench, _, _, cx| {
            workbench.perform_snippet_completions(cx);
            Outcome::done()
        });
    }

    /// `hasSnippetCompletions`, set on every cursor change while tab completion is on.
    pub(super) fn tab_completion_context(&self, context: &mut std::collections::HashMap<String, Value>) {
        let Some(editor) = self.editors.active() else { return };
        if self.suggest_setting(editor, "editor.tabCompletion") != json!("onlySnippets") {
            return;
        }
        context.insert("hasSnippetCompletions".into(), json!(!self.tab_completion_snippets(editor).is_empty()));
    }

    /// `_updateSnippets`: an empty selection takes the snippets whose prefix ends the non-whitespace text before the
    /// cursor, a one-line selection of at most 100 characters those whose prefix it equals.
    fn tab_completion_snippets(&self, editor: &OpenEditor) -> Vec<Snippet> {
        let selection = editor.selection();
        let head = selection.head;
        let language = snippet_language(editor, head.line, head.column, &self.languages);
        let path = (editor.document().scheme == "file").then(|| editor.document().path.to_string_lossy().into_owned());
        let snippets = self.read_snippets(Some(&language), path.as_deref(), SnippetGetOptions::default());
        let line = &editor.document().lines[head.line];
        if selection.is_empty() {
            let prefix = non_whitespace_prefix(line, head.column);
            return snippets.into_iter().filter(|snippet| !prefix.is_empty() && prefix.ends_with(&snippet.prefix)).collect();
        }
        let (start, end) = (selection.start(), selection.end());
        if start.line != end.line || end.column - start.column > MAX_PREFIX_LENGTH {
            return Vec::new();
        }
        let selected: String = line.chars().skip(start.column).take(end.column - start.column).collect();
        snippets.into_iter().filter(|snippet| snippet.prefix == selected).collect()
    }

    /// `performSnippetCompletions`: one snippet replaces its prefix before every cursor, more go to the suggest
    /// widget.
    fn perform_snippet_completions(&mut self, cx: &mut Context<Self>) {
        self.refresh_snippet_files();
        let Some(editor) = self.editors.active() else { return };
        let mut snippets = self.tab_completion_snippets(editor);
        if snippets.len() > 1 {
            self.trigger_suggest(TriggerOptions { provider_filter: vec![TAB_COMPLETION_PROVIDER], ..TriggerOptions::new(false) }, cx);
            return;
        }
        let Some(snippet) = snippets.pop() else { return };
        let clipboard = snippet.needs_clipboard().then(|| cx.read_from_clipboard().and_then(|item| item.text())).flatten();
        let before = snippet.prefix.chars().count();
        let ranges: Vec<_> = editor.selections.iter().map(|selection| (Position { line: selection.head.line, column: selection.head.column.saturating_sub(before) }, selection.head, snippet.body.as_str(), false)).collect();
        self.insert_snippets(SnippetInsert { snippets: &ranges, clipboard: clipboard.as_deref(), undo_stop_before: true, undo_stop_after: true, reason: "snippet" }, cx);
    }

    /// The tab completion provider's `provideCompletionItems`: each snippet over its prefix before the cursor.
    pub(super) fn tab_completion_suggestions(&self) -> Value {
        let Some(editor) = self.editors.active() else { return Value::Null };
        let head = editor.selection().head;
        let line = &editor.document().lines[head.line];
        let end = utf16_column(line, head.column) + 1;
        let suggestions: Vec<Value> = self
            .tab_completion_snippets(editor)
            .iter()
            .map(|snippet| {
                let start = utf16_column(line, head.column.saturating_sub(snippet.prefix.chars().count())) + 1;
                let range = json!({ "startLineNumber": head.line + 1, "startColumn": start, "endLineNumber": head.line + 1, "endColumn": end });
                snippet_completion(snippet, range)
            })
            .collect();
        json!({ "b": suggestions })
    }
}

/// `getNonWhitespacePrefix`: the text before `column` after its last whitespace, looking back at most 100 characters.
fn non_whitespace_prefix(line: &str, column: usize) -> String {
    let before: Vec<char> = line.chars().take(column).collect();
    let min = before.len().saturating_sub(MAX_PREFIX_LENGTH);
    match before[min..].iter().rposition(|c| c.is_whitespace()) {
        Some(index) => before[min + index + 1..].iter().collect(),
        None if min == 0 => before.iter().collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prefix_is_the_text_after_the_last_whitespace() {
        assert_eq!(non_whitespace_prefix("let x = foo.bar", 15), "foo.bar");
        assert_eq!(non_whitespace_prefix("  log", 5), "log");
        assert_eq!(non_whitespace_prefix("abc def", 3), "abc");
        assert_eq!(non_whitespace_prefix("x ", 2), "");
        assert_eq!(non_whitespace_prefix(&"a".repeat(120), 120), "");
        assert_eq!(non_whitespace_prefix(&format!("{} b", "a".repeat(120)), 122), "b");
    }
}
