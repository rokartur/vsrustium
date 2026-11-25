//! markersViewActions.ts `QuickFixAction` with markersTreeViewer.ts `MarkerViewModel`: a hovered marker row asks for
//! the quick fixes of its range after 300 ms; with fixes, the row shows the light bulb in place of the severity icon,
//! and a click lists their titles. Running one opens the marker's file and applies the fix.
// shortcut: only markers of a model the window has open get fixes; upstream also waits for the model to open.

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{Bounds, Context, Pixels, Task, Window};
use serde_json::json;

use super::Workbench;
use super::code_actions::CodeActionSet;
use crate::commands::Outcome;
use crate::markers::Marker;
use crate::menus::MenuAction;

/// `MarkersViewModel.hoverDelayer`.
const HOVER_DELAY: Duration = Duration::from_millis(300);

#[derive(Default)]
pub(super) struct ProblemsQuickFixes {
    /// The marker whose fixes are asked or shown.
    marker: Option<Arc<Marker>>,
    set: Option<Rc<CodeActionSet>>,
    request: Option<Task<()>>,
    /// The `problems.quickFix.<index>` commands registered so far.
    commands: usize,
}

impl Workbench {
    /// `onMarkerMouseHover`: a new marker replaces the fixes of the previous one.
    pub(super) fn problems_marker_hovered(&mut self, marker: &Arc<Marker>, cx: &mut Context<Self>) {
        let fixes = &mut self.problems_quick_fixes;
        if fixes.marker.as_ref() == Some(marker) {
            return;
        }
        fixes.marker = Some(marker.clone());
        if let Some(set) = fixes.set.take() {
            self.release_code_action_set(set);
        }
        let Some(editor) = self.editors.editors.iter().chain(&self.editors.models).find(|editor| editor.document().uri_string() == marker.resource) else {
            self.problems_quick_fixes.request = None;
            return;
        };
        let request = self.request_marker_quick_fixes(editor, marker);
        let marker = marker.clone();
        self.problems_quick_fixes.request = Some(cx.spawn(async move |workbench, cx| {
            cx.background_executor().timer(HOVER_DELAY).await;
            let set = request.await;
            let _ = workbench.update(cx, |workbench, cx| {
                if workbench.problems_quick_fixes.marker.as_ref() != Some(&marker) || set.valid.is_empty() {
                    workbench.release_code_action_set(Rc::new(set));
                    return;
                }
                workbench.problems_quick_fixes.set = Some(Rc::new(set));
                cx.notify();
            });
        }));
    }

    /// The light bulb of `marker`'s row, `None` without fixes. `QuickFixAction.autoFixable` only adds a class no CSS
    /// rule styles, and markers.css colors `codicon-light-bulb`, not the `codicon-lightbulb` the action has, so the
    /// icon is always the plain one in the row's color.
    pub(super) fn problems_quick_fix_icon(&self, marker: &Marker) -> Option<&'static str> {
        let fixes = &self.problems_quick_fixes;
        fixes.set.as_ref().filter(|_| fixes.marker.as_deref() == Some(marker)).map(|_| "lightbulb")
    }

    /// `QuickFixActionViewItem.showQuickFixes`: the fixes' titles in a context menu 10px right of and 4px below the
    /// light bulb's top left corner.
    pub(super) fn show_problems_quick_fixes(&mut self, anchor: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(set) = self.problems_quick_fixes.set.clone() else { return };
        while self.problems_quick_fixes.commands < set.valid.len() {
            let index = self.problems_quick_fixes.commands;
            self.commands.register(format!("problems.quickFix.{index}"), move |workbench, _, window, cx| {
                workbench.run_problems_quick_fix(index, window, cx);
                Outcome::done()
            });
            self.problems_quick_fixes.commands += 1;
        }
        let actions = set.valid.iter().enumerate().map(|(index, item)| MenuAction::Command { id: format!("problems.quickFix.{index}"), title: item.action["title"].as_str().unwrap_or_default().to_owned(), enabled: true, checked: false, alt: None }).collect();
        let anchor = Bounds { origin: gpui::point(anchor.origin.x + gpui::px(10.), anchor.origin.y + anchor.size.height + gpui::px(4.)), size: gpui::Size::default() };
        let context = self.key_context(window);
        self.show_context_menu_actions(actions, &context, anchor, Vec::new(), window, cx);
    }

    /// `toActions`' run: open the file at the marker, then apply the fix.
    fn run_problems_quick_fix(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(set), Some(marker)) = (self.problems_quick_fixes.set.clone(), self.problems_quick_fixes.marker.clone()) else { return };
        let Some(item) = set.valid.get(index).cloned() else { return };
        let target = json!({ "resource": crate::debug::uri_from_string(&marker.resource), "startLineNumber": marker.start_line, "startColumn": marker.start_column });
        self.open_marker_related(&target, window, cx);
        self.apply_code_action(item, set, false, window.window_handle(), cx).detach();
    }
}
