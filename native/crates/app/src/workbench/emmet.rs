//! The emmet contribution (emmetActions.ts, expandAbbreviation.ts): Emmet: Expand Abbreviation runs the emmet
//! extension's `emmet.expandAbbreviation` with the language at the cursor and the emmet mode its grammar belongs to.

use serde_json::{Value, json};

use super::Workbench;
use super::snippet_completions::snippet_language;

/// `EmmetEditorAction.emmetSupportedModes`.
const EMMET_SUPPORTED_MODES: [&str; 14] = ["html", "css", "xml", "xsl", "haml", "jade", "jsx", "slim", "scss", "sass", "less", "stylus", "styl", "svg"];

impl Workbench {
    pub(super) fn register_emmet(&mut self) {
        self.commands.register("editor.emmet.action.expandAbbreviation", |workbench, _, window, cx| {
            let argument = workbench.emmet_language();
            workbench.execute_command("emmet.expandAbbreviation", vec![argument], window, cx)
        });
    }

    /// `getLanguage` at the selection's start, `null` without an editor.
    fn emmet_language(&self) -> Value {
        let Some(editor) = self.editors.active() else { return Value::Null };
        let start = editor.selection().start();
        let language_id = snippet_language(editor, start.line, start.column, &self.languages);
        let syntax = language_id.rsplit('.').next().unwrap_or_default();
        if syntax.is_empty() {
            return Value::Null;
        }
        let parent_mode = parent_mode(syntax, self.languages.grammars.scope_name(syntax));
        json!({ "language": syntax, "parentMode": parent_mode })
    }
}

/// `checkParentMode`: the last segment of the grammar's scope name, other than the first, that emmet supports.
fn parent_mode<'a>(syntax: &'a str, scope_name: Option<&'a str>) -> &'a str {
    let Some(scope_name) = scope_name else { return syntax };
    let segments: Vec<&str> = scope_name.split('.').collect();
    for index in 1..segments.len() {
        let language = segments[segments.len() - index];
        if EMMET_SUPPORTED_MODES.contains(&language) {
            return language;
        }
    }
    syntax
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_take_the_emmet_mode_of_their_grammar() {
        assert_eq!(parent_mode("markdown", Some("text.html.markdown")), "html");
        assert_eq!(parent_mode("handlebars", Some("text.html.handlebars")), "html");
        assert_eq!(parent_mode("nunjucks", Some("text.html.nunjucks")), "html");
        assert_eq!(parent_mode("laravel-blade", Some("text.html.php.laravel-blade")), "html");
        assert_eq!(parent_mode("rust", Some("source.rust")), "rust");
        assert_eq!(parent_mode("plaintext", None), "plaintext");
    }
}
