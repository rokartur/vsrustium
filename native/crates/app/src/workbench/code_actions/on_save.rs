//! saveParticipants.ts `CodeActionOnSaveParticipant`: on an explicit save `editor.codeActionsOnSave` applies the
//! source actions of its kinds over the whole file, after trimming whitespace and before formatting.

use std::path::Path;

use gpui::{Context, Task};
use serde_json::{Value, json};

use super::*;
use crate::workbench::auto_save::SaveReason;
use crate::workbench::text_input::SavePhase;

const SOURCE_FIX_ALL: &str = "source.fixAll";

impl Workbench {
    /// `participate`: `None` when the setting asks for nothing on this save.
    pub(in crate::workbench) fn code_actions_on_save(&mut self, scheme: &str, path: &Path, reason: SaveReason, cx: &mut Context<Self>) -> Option<Task<()>> {
        // A save on window change runs while no window is active; the port has one window per process.
        let window = cx.active_window().or_else(|| cx.windows().first().copied())?;
        let editor = self.editors.model(scheme, path)?;
        let (kinds, excludes) = on_save_kinds(&self.language_setting(editor, "editor.codeActionsOnSave"), reason);
        if kinds.is_empty() {
            return None;
        }
        self.run_save_participants(scheme, path, reason, SavePhase::BeforeFormat);
        let (scheme, path) = (scheme.to_owned(), path.to_owned());
        Some(cx.spawn(async move |workbench, cx| {
            for kind in kinds {
                let filter = Filter { include: Some(kind), excludes: excludes.clone(), include_source_actions: true, only_preferred: false };
                let trigger = Trigger { kind: AUTO, source: Source::Default, filter: filter.clone(), auto_apply: None, not_available: None, window: Some(window) };
                let calls = workbench.update(cx, |workbench, _| {
                    let editor = workbench.editors.model(&scheme, &path)?;
                    Some(workbench.code_action_calls(editor, full_range(&editor.document().lines), &trigger))
                });
                let Ok(Some(calls)) = calls else { return };
                let set = Rc::new(collect_code_actions(calls, &filter, &Rc::new(RefCell::new(Vec::new()))).await);
                // A failing action does not stop the others.
                for item in set.valid.clone() {
                    let Ok(apply) = workbench.update(cx, |workbench, cx| workbench.apply_code_action(item, set.clone(), false, window, cx)) else { return };
                    apply.await;
                }
                let _ = workbench.update(cx, |workbench, _| workbench.release_code_action_set(set));
            }
        }))
    }
}

/// The kinds to run in order and the kinds to leave out: an array setting runs on explicit saves only; an object
/// runs its kinds that are not `never`, `explicit` (or `true`) ones only on explicit saves, fix-all kinds first, a
/// kind inside another listed one dropped.
fn on_save_kinds(setting: &Value, reason: SaveReason) -> (Vec<String>, Vec<String>) {
    if reason == SaveReason::Auto {
        return (Vec::new(), Vec::new());
    }
    let explicit = reason == SaveReason::Explicit;
    let (items, excludes): (Vec<&str>, Vec<String>) = match setting {
        Value::Array(items) if explicit => (items.iter().filter_map(Value::as_str).collect(), Vec::new()),
        Value::Object(map) => {
            let on = |value: &Value| value.as_bool() != Some(false) && !value.is_null() && value != "" && value != "never";
            let items = map.iter().filter(|(_, value)| on(value)).map(|(kind, _)| kind.as_str()).collect();
            (items, map.iter().filter(|(_, value)| *value == "never").map(|(kind, _)| kind.clone()).collect())
        }
        _ => return (Vec::new(), Vec::new()),
    };
    let mut kinds: Vec<&str> = items.iter().copied().filter(|kind| items.iter().all(|other| other == kind || !contains(other, kind))).collect();
    if let Value::Object(map) = setting {
        kinds.sort_by_key(|kind| !contains(SOURCE_FIX_ALL, kind));
        kinds.retain(|kind| map[*kind] == "always" || (explicit && (map[*kind] == "explicit" || map[*kind] == true)));
    }
    (kinds.into_iter().map(str::to_owned).collect(), excludes)
}

fn full_range(lines: &[String]) -> Value {
    let last = lines.len().max(1);
    let end_column = lines.last().map_or(0, |line| line.encode_utf16().count()) + 1;
    json!({ "startLineNumber": 1, "startColumn": 1, "endLineNumber": last, "endColumn": end_column })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_saves_run_the_kinds_fix_all_first_without_subsets() {
        let setting = json!({ "source.organizeImports": "explicit", "source.fixAll": true, "source.fixAll.eslint": "explicit", "source.addMissingImports": "never", "source.sortImports": "always" });
        let (kinds, excludes) = on_save_kinds(&setting, SaveReason::Explicit);
        assert_eq!(kinds, ["source.fixAll", "source.organizeImports", "source.sortImports"]);
        assert_eq!(excludes, ["source.addMissingImports"]);
        assert_eq!(on_save_kinds(&setting, SaveReason::FocusChange).0, ["source.sortImports"], "only always on other saves");
        assert!(on_save_kinds(&setting, SaveReason::Auto).0.is_empty());
        assert_eq!(on_save_kinds(&json!(["source.fixAll"]), SaveReason::Explicit).0, ["source.fixAll"]);
        assert!(on_save_kinds(&json!(["source.fixAll"]), SaveReason::WindowChange).0.is_empty());
    }
}
