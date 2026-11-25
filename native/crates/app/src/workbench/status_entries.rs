//! Extension status bar entries (statusbarItem.ts): `$(icon)` labels, colors, tooltip and command.

use std::time::Duration;

use gpui::prelude::FluentBuilder;
use gpui::{Animation, AnimationExt, Context, Div, Stateful, InteractiveElement, ParentElement, Rgba, StatefulInteractiveElement, Styled, Transformation, div, percentage, px, svg};
use serde_json::Value;

use super::Workbench;
use super::hover::{HoverContent, HoverOptions};
use crate::main_thread_status_bar::StatusEntry;
use crate::theme::parse_hex;

/// markers.contribution.ts: `status.problems` sits at priority 50 on the left.
pub(super) const PROBLEMS_PRIORITY: f64 = 50.;
/// editorStatus.ts: the editor entries sit at 100.1 (language) to 100.8 on the right; extension entries in between
/// go before all of them.
pub(super) const EDITOR_STATUS_PRIORITY: f64 = 100.1;

impl Workbench {
    /// The entries of one side whose priority `keep` accepts, in statusbarModel.ts order.
    pub(super) fn extension_status_items(&self, left: bool, keep: impl Fn(f64) -> bool, foreground: Rgba, cx: &mut Context<Self>) -> Div {
        let mut items = div().h_full().flex();
        for entry in self.status_entries.iter().filter(|entry| entry.align_left == left && keep(entry.priority)) {
            items = items.child(self.extension_status_item(entry, foreground, cx));
        }
        items
    }

    pub(super) fn extension_status_item(&self, entry: &StatusEntry, foreground: Rgba, cx: &mut Context<Self>) -> Stateful<Div> {
        let (color, background) = match entry.kind {
            Some(kind) => (self.color(&format!("statusBarItem.{kind}Foreground")), self.theme.color(&format!("statusBarItem.{kind}Background"))),
            None => (entry.color.as_ref().and_then(|color| self.entry_color(color)).unwrap_or(foreground), entry.background_color.as_ref().and_then(|color| self.entry_color(color))),
        };
        let mut label = self.highlighted_label(&entry.text, &[], color, 16., color);
        if entry.show_progress {
            // statusbarItem.ts: a non-breaking space between the spinner and the text.
            label = self.highlighted_label(&format!("\u{a0}{}", entry.text), &[], color, 16., color);
            // codicon-animations.css: `codicon-modifier-spin` turns once in 1.5s in 30 steps.
            let path = self.app_root.join("node_modules/@vscode/codicons/src/icons/loading.svg");
            let spinner = svg()
                .external_path(path.to_string_lossy().into_owned())
                .size(px(16.))
                .flex_none()
                .text_color(color)
                .with_animation("status-progress-spin", Animation::new(Duration::from_millis(1500)).repeat(), |spinner, delta| {
                    spinner.with_transformation(Transformation::rotate(percentage((delta * 30.).floor() / 30.)))
                });
            label = div().flex().items_center().child(spinner).child(label);
        }
        let command = entry.command.clone();
        let item = div()
            .id(gpui::SharedString::from(format!("status-entry-{}", entry.entry_id)))
            .relative()
            .h_full()
            .flex_none()
            .mx(px(3.))
            .px(px(5.))
            .flex()
            .items_center()
            .whitespace_nowrap()
            .text_color(color)
            .when_some(background, |item, background| item.bg(background))
            .when(command.is_some(), |item| item.cursor_pointer())
            .child(label)
            .when_some(entry.tooltip.clone(), |item, tooltip| item.child(self.hover_target(format!("status-entry-hover-{}", entry.entry_id), HoverContent::Text(tooltip), HoverOptions::status_bar())))
            .on_click(cx.listener(move |workbench, _, window, cx| {
                let Some(command) = &command else { return };
                let (id, args) = match command {
                    Value::String(id) => (id.clone(), Vec::new()),
                    command => (command["id"].as_str().unwrap_or_default().to_owned(), command["arguments"].as_array().cloned().unwrap_or_default()),
                };
                let outcome = workbench.execute_command(&id, args, window, cx);
                super::quick_access::show_failure(id, outcome, window, cx);
            }));
        item
    }

    /// A string `#rrggbb` color or a `ThemeColor` `{ id }`.
    fn entry_color(&self, color: &Value) -> Option<Rgba> {
        match color {
            Value::String(hex) => parse_hex(hex),
            color => self.theme.color(color["id"].as_str()?),
        }
    }
}
