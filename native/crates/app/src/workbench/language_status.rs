//! languageStatus.ts and languageStatusService.ts: the statuses extensions set for a language (`$setLanguageStatus`)
//! show as one `{}` entry next to the language entry, whose hover lists them; a pinned status gets its own entry.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{Animation, AnimationExt, AnyElement, Bounds, Context, Pixels, Rgba, Size, Transformation, Window, canvas, div, percentage, prelude::*, px, size, svg};
use serde_json::{Value, json};

use super::Workbench;
use super::hover::{HoverContent, HoverOptions, text_width};
use super::quick_input_view::icon_at;
use super::status_entries::EDITOR_STATUS_PRIORITY;
use crate::commands::Outcome;
use crate::editor::marker_decorations::MAX_MARKER_DECORATIONS;
use crate::language_features::{Candidate, LanguageFeatures};
use crate::main_thread_status_bar::StatusEntry;
use crate::notifications::{Node, parse_linked_text};

#[cfg(test)]
mod tests;

const SET: &str = "$setLanguageStatus";
const ID: &str = "status.languageStatus";
const KEY_DEDICATED_ITEMS: &str = "languageStatus.dedicated";
const KEY_INTERACT_COUNT: &str = "languageStatus.interactCount";
pub(super) const PIN: &str = "_languageStatus.pin";
pub(super) const UNPIN: &str = "_languageStatus.unpin";
const BUSY: &str = "$(loading~spin)";
/// base/common/severity.ts.
const WARNING: u64 = 2;
const ERROR: u64 = 3;
/// actionbar.css: a 16px codicon in a 3px padded `.action-label`.
const PIN_BOX: f32 = 22.;
/// The hover's 19px line; the pin's box overflows it (ui-pair: rows 20px apart with the border in the reference).
const ROW_HEIGHT: f32 = super::hover::LINE_HEIGHT;
/// `.right:not(:empty)`.
const RIGHT_PADDING: f32 = 16.;

#[derive(Default)]
pub(super) struct LanguageStatus {
    /// Its own registry: `$setLanguageStatus` handles count apart from the language feature handles.
    registry: LanguageFeatures,
    /// The view model of the last `_update`.
    view: LanguageStatusView,
    /// Bumped on each view model change; keys the wiggle and flash animations.
    serial: usize,
    wiggle: bool,
    flash: bool,
    /// The serial whose hover already counted as an interaction.
    counted: usize,
    bounds: Rc<Cell<Bounds<Pixels>>>,
}

/// `ILanguageStatus`.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Status {
    pub(super) id: String,
    name: String,
    pub(super) severity: u64,
    label: String,
    short_label: String,
    detail: String,
    pub(super) busy: bool,
    source: String,
    /// The internal `Command`: `id`, `title`, `tooltip`, `arguments`.
    command: Option<Value>,
}

impl Status {
    fn from_dto(status: &Value) -> Status {
        let text = |value: &Value| value.as_str().unwrap_or_default().to_owned();
        let (label, short_label) = match &status["label"] {
            Value::String(label) => (label.clone(), label.clone()),
            label => (text(&label["value"]), text(&label["shortValue"])),
        };
        Status {
            id: text(&status["id"]),
            name: text(&status["name"]),
            severity: status["severity"].as_u64().unwrap_or(0),
            label,
            short_label,
            detail: text(&status["detail"]),
            busy: status["busy"] == true,
            source: text(&status["source"]),
            command: Some(status["command"].clone()).filter(Value::is_object),
        }
    }
}

/// `LanguageStatusViewModel`.
#[derive(Debug, Default, PartialEq)]
pub(super) struct LanguageStatusView {
    pub(super) combined: Vec<Status>,
    pub(super) dedicated: Vec<Status>,
}

impl LanguageStatusView {
    fn show_severity(&self) -> bool {
        self.combined.first().is_some_and(|first| first.severity >= WARNING)
    }

    /// Unpinned statuses drive the combined entry's busy indicator.
    fn is_busy(&self) -> bool {
        self.combined.iter().any(|status| status.busy && !self.dedicated.contains(status))
    }

    /// The combined entry's text: the spinner while busy, else `_severityToComboCodicon` of the first status.
    pub(super) fn combined_text(&self) -> Option<&'static str> {
        let first = self.combined.first()?;
        if self.is_busy() {
            return Some(BUSY);
        }
        Some(match first.severity {
            ERROR => "$(bracket-error)",
            WARNING => "$(bracket-dot)",
            _ => "$(bracket)",
        })
    }
}

impl Workbench {
    /// mainThreadLanguages.ts `$setLanguageStatus`: replaces the status of `handle`.
    pub(crate) fn set_language_status(&mut self, handle: u64, status: Value, cx: &mut Context<Self>) {
        self.language_status.registry.unregister(handle);
        self.language_status.registry.register(SET, vec![json!(handle), status["selector"].clone(), status]);
        self.refresh_language_status(cx);
    }

    /// `$removeLanguageStatus`.
    pub(crate) fn remove_language_status(&mut self, handle: u64, cx: &mut Context<Self>) {
        if self.language_status.registry.unregister(handle).is_some() {
            self.refresh_language_status(cx);
        }
    }

    pub(super) fn register_language_status(&mut self) {
        self.commands.register(PIN, |workbench, args, _, cx| {
            let Some(id) = args.first().and_then(Value::as_str) else { return Outcome::done() };
            workbench.pin_language_status(id, true, cx);
            Outcome::done()
        });
        self.commands.register(UNPIN, |workbench, args, _, cx| {
            let Some(id) = args.first().and_then(Value::as_str) else { return Outcome::done() };
            workbench.pin_language_status(id, false, cx);
            Outcome::done()
        });
        // ResetAction keeps upstream's id.
        self.commands.register("editor.inlayHints.Reset", |workbench, _, _, _| {
            workbench.profile_storage.lock().unwrap().delete(KEY_INTERACT_COUNT);
            Outcome::done()
        });
    }

    /// `getLanguageStatus` of the active editor's model: the matching statuses by severity, source and id, the
    /// selector score breaking ties.
    pub(super) fn language_statuses(&self) -> Vec<Status> {
        let Some(editor) = self.editors.active().filter(|editor| !self.shows_webview(editor) && editor.document().scheme != super::extension_editor::EXTENSION_EDITOR_SCHEME) else {
            return Vec::new();
        };
        let uri = editor.document().uri();
        let candidate = Candidate { uri: &uri, language: &editor.document().language_id };
        let mut statuses: Vec<Status> = self.language_status.registry.ordered(SET, &candidate).into_iter().map(|registration| Status::from_dto(&registration.args[2])).collect();
        statuses.extend(self.formatter_conflict().map(formatter_conflict_status));
        statuses.extend(self.limit_indicator_statuses(editor));
        statuses.sort_by(|a, b| b.severity.cmp(&a.severity).then_with(|| a.source.cmp(&b.source)).then_with(|| a.id.cmp(&b.id)));
        statuses
    }

    /// `_createViewModel`.
    pub(super) fn language_status_view(&self) -> LanguageStatusView {
        let pinned = self.dedicated_language_status_ids();
        let combined = self.language_statuses();
        let dedicated = combined.iter().filter(|status| pinned.contains(&status.id)).cloned().collect();
        LanguageStatusView { combined, dedicated }
    }

    /// `_update`: a changed view model wiggles the icon when severe or new to the user and flashes it when
    /// severe, unless busy.
    pub(crate) fn refresh_language_status(&mut self, cx: &mut Context<Self>) {
        let view = self.language_status_view();
        if view == self.language_status.view {
            return;
        }
        let busy = view.is_busy();
        let show_severity = view.show_severity();
        let interacted = self.language_status_interactions() >= 3;
        let state = &mut self.language_status;
        state.wiggle = !busy && (show_severity || !interacted);
        state.flash = !busy && show_severity;
        state.serial += 1;
        state.view = view;
        cx.notify();
    }

    /// The combined entry, which goes left of the language entry, and the pinned entries, which go right of it.
    pub(super) fn language_status_items(&self, foreground: Rgba, cx: &mut Context<Self>) -> (Option<AnyElement>, Vec<AnyElement>) {
        let view = self.language_status_view();
        let dedicated = view.dedicated.iter().map(|status| self.extension_status_item(&dedicated_entry(status), foreground, cx).into_any_element()).collect();
        (self.combined_language_status_item(&view, foreground, cx), dedicated)
    }

    fn combined_language_status_item(&self, view: &LanguageStatusView, foreground: Rgba, cx: &mut Context<Self>) -> Option<AnyElement> {
        let text = view.combined_text()?;
        let state = &self.language_status;
        let serial = state.serial;
        let path = self.app_root.join("node_modules/@vscode/codicons/src/icons").join(format!("{}.svg", codicon_file(text)));
        let icon = svg().external_path(path.to_string_lossy().into_owned()).size(px(16.)).flex_none().text_color(foreground);
        let icon = if text == BUSY {
            // codicon-animations.css: `codicon-modifier-spin` turns once in 1.5s in 30 steps.
            icon.with_animation("language-status-spin", Animation::new(Duration::from_millis(1500)).repeat(), |icon, delta| icon.with_transformation(Transformation::rotate(percentage((delta * 30.).floor() / 30.))))
                .into_any_element()
        } else if state.wiggle {
            icon.with_animation(("language-status-wiggle", serial), Animation::new(Duration::from_millis(800)), |icon, delta| icon.with_transformation(wiggle(delta))).into_any_element()
        } else {
            icon.into_any_element()
        };
        let content = HoverContent::LanguageStatus;
        let bounds = state.bounds.clone();
        let anchor = state.bounds.clone();
        let item = div()
            .id(ID)
            .relative()
            .h_full()
            .flex_none()
            // statusbarpart.css `.compact-right`: `compact: true` next to the language entry.
            .ml(px(5.))
            .px(px(3.))
            .flex()
            .items_center()
            .cursor_pointer()
            .child(icon)
            .child(canvas(move |area, _, _| bounds.set(area), |_, _, _, _| {}).absolute().inset_0())
            .child(self.hover_target(ID, content.clone(), HoverOptions::status_bar()))
            // shortcut: counts the pointer entering the entry, not the hover showing; count the hover once hover.rs reports it.
            .on_hover(cx.listener(|workbench, hovered: &bool, _, _| {
                if *hovered {
                    workbench.count_language_status_interaction();
                }
            }))
            // statusbar.ts `ShowTooltipCommand`.
            .on_click(cx.listener(move |workbench, _, window, cx| {
                workbench.count_language_status_interaction();
                workbench.show_managed_hover(ID, content.clone(), HoverOptions::status_bar(), anchor.get(), window, cx);
            }));
        if !state.flash {
            return Some(item.into_any_element());
        }
        let prominent = self.color("statusBarItem.prominentBackground");
        // languageStatus.css `flash`: the background goes to prominentBackground and back in 0.8s.
        let item = item.with_animation(("language-status-flash", serial), Animation::new(Duration::from_millis(800)), move |item, delta| {
            item.bg(Rgba { a: prominent.a * (1. - (2. * delta - 1.).abs()), ..prominent })
        });
        Some(item.into_any_element())
    }

    /// The hover's text, for the empty check: the labels.
    pub(super) fn language_status_hover_text(&self) -> String {
        self.language_status.view.combined.iter().map(|status| status.label.as_str()).collect::<Vec<_>>().join("\n")
    }

    /// The rows of `_renderStatus` (languageStatus.css `.hover-language-status`) and their content size.
    pub(super) fn language_status_hover(&self, window: &Window, cx: &mut Context<Self>) -> (Size<f32>, AnyElement) {
        let rows = hover_rows(&self.language_status.view);
        let foreground = self.color("editorHoverWidget.foreground");
        let link = self.color("textLink.foreground");
        let border = self.color("notifications.border");
        let toolbar_hover = self.color("toolbar.hoverBackground");
        let (mut width, count) = (0f32, rows.len());
        let mut column = div().flex().flex_col();
        for (index, row) in rows.into_iter().enumerate() {
            let mut row_width = label_width(&row.label, window) + PIN_BOX + RIGHT_PADDING;
            let mut element = div().h(px(ROW_HEIGHT)).flex().items_center().when(index + 1 < count, |element| element.border_b_1().border_color(border));
            if let Some((icon, color)) = row.severity {
                let color = color.map_or(foreground, |key| self.color(key));
                element = element.child(div().pr(px(8.)).flex_none().child(self.icon(icon, 16., 16., color)));
                row_width += 16. + 8.;
            }
            let mut left = div().flex().items_center().min_w_0().child(self.highlighted_label(&row.label, &[], foreground, 16., foreground));
            if !row.detail.is_empty() {
                left = left.child(div().px(px(2.)).opacity(0.6).child("\u{2013}")).child(self.highlighted_label(&row.detail, &[], foreground, 16., foreground));
                row_width += text_width("\u{2013}", false, window) + 4. + label_width(&row.detail, window);
            }
            let mut right = div().pl(px(RIGHT_PADDING)).flex().flex_none().items_center();
            let has_command = row.command.is_some();
            // shortcut: the link's and the pin's native title tooltips are not drawn; hover.rs shows one hover at a time.
            if let Some((title, id, arguments)) = row.command {
                row_width += text_width(&title, false, window) + 8.;
                right = right.child(div().id(("language-status-command", index)).cursor_pointer().text_color(link).child(title).on_click(cx.listener(move |workbench, _, window, cx| {
                    workbench.dismiss_hover(cx);
                    workbench.execute_command(&id, arguments.clone(), window, cx);
                })));
            }
            let (icon, command) = if row.pinned { ("pinned", UNPIN) } else { ("pin", PIN) };
            let id = row.id;
            let pin = div()
                .id(("language-status-pin", index))
                .when(has_command, |pin| pin.ml(px(8.)))
                .p(px(3.))
                .rounded(px(5.))
                .cursor_pointer()
                .hover(|pin| pin.bg(toolbar_hover))
                .child(self.icon(icon, 16., 16., foreground))
                .on_click(cx.listener(move |workbench, _, window, cx| _ = workbench.execute_command(command, vec![json!(id)], window, cx)));
            right = right.child(pin);
            element = element.child(div().flex_1().min_w_0().flex().justify_between().items_center().child(left).child(right));
            column = column.child(element);
            width = width.max(row_width);
        }
        let height = count as f32 * ROW_HEIGHT + count.saturating_sub(1) as f32;
        (Size { width, height }, column.into_any_element())
    }

    /// The pin and unpin actions of the hover.
    fn pin_language_status(&mut self, id: &str, pinned: bool, cx: &mut Context<Self>) {
        let mut ids = self.dedicated_language_status_ids();
        if pinned && !ids.iter().any(|other| other == id) {
            ids.push(id.to_owned());
        }
        if !pinned {
            ids.retain(|other| other != id);
        }
        let mut storage = self.profile_storage.lock().unwrap();
        match ids.is_empty() {
            true => storage.delete(KEY_DEDICATED_ITEMS),
            false => storage.set(KEY_DEDICATED_ITEMS, json!(ids).to_string()),
        }
        drop(storage);
        self.refresh_language_status(cx);
    }

    /// `_restoreState`: unreadable storage pins nothing.
    fn dedicated_language_status_ids(&self) -> Vec<String> {
        let storage = self.profile_storage.lock().unwrap();
        storage.get(KEY_DEDICATED_ITEMS).and_then(|raw| serde_json::from_str(raw).ok()).unwrap_or_default()
    }

    fn language_status_interactions(&self) -> u64 {
        self.profile_storage.lock().unwrap().get(KEY_INTERACT_COUNT).and_then(|raw| raw.parse().ok()).unwrap_or(0)
    }

    /// `StoredCounter.increment`, once per view model while the user has not interacted three times.
    fn count_language_status_interaction(&mut self) {
        let count = self.language_status_interactions();
        if count >= 3 || self.language_status.counted == self.language_status.serial {
            return;
        }
        self.language_status.counted = self.language_status.serial;
        self.profile_storage.lock().unwrap().set(KEY_INTERACT_COUNT, (count + 1).to_string());
    }
}

impl Workbench {
    /// limitIndicator.contribution.ts: a Warning for each feature of the active editor its limit cut short.
    fn limit_indicator_statuses(&self, editor: &crate::editor::OpenEditor) -> Vec<Status> {
        let only_shown = |limit: Option<usize>| limit.map(|limit| format!("only {limit} shown for performance reasons"));
        let colors = only_shown(self.color_detector.limited.filter(|_| self.color_detector.model_id == Some(editor.model_id)));
        let folding = only_shown(editor.folding_limited);
        let source_truncated = self.diagnostics.service.is_source_truncated(&editor.document().uri_string());
        let shown = !self.filters_validation_decorations();
        let problems = match (shown && source_truncated, shown && editor.text_model().markers_truncated) {
            (true, true) => Some(format!("Some problems are not shown in the problems view, and only {MAX_MARKER_DECORATIONS} problems are rendered in the editor for performance reasons.")),
            (true, false) => Some("Some problems are not shown in the problems view for performance reasons.".to_string()),
            (false, true) => Some(format!("Only {MAX_MARKER_DECORATIONS} problems are rendered in the editor for performance reasons.")),
            (false, false) => None,
        };
        let entries = [
            ("decoratorsLimitInfo", "Color Decorator Status", "Color decorators", "Color Decorators", Some("editor.colorDecoratorsLimit"), colors),
            ("foldingLimitInfo", "Folding Status", "Folding ranges", "Folding", Some("editor.foldingMaximumRegions"), folding),
            ("diagnosticsLimitInfo", "Problems Status", "Problems", "Problems", None, problems),
        ];
        let mut statuses = Vec::new();
        for (id, name, label, source, setting, detail) in entries {
            let Some(detail) = detail else { continue };
            statuses.push(Status {
                id: id.into(),
                name: name.into(),
                severity: WARNING,
                label: label.into(),
                short_label: label.into(),
                detail,
                busy: false,
                source: source.into(),
                command: setting.map(|setting| json!({ "id": "workbench.action.openSettings", "title": "Configure", "arguments": [setting] })),
            });
        }
        statuses
    }
}

/// formatActionsMultiple.ts `_updateStatus`: the `formatter.conflict` status.
fn formatter_conflict_status(detail: String) -> Status {
    Status {
        id: "formatter.conflict".into(),
        name: "Formatter Conflicts".into(),
        severity: ERROR,
        label: "Formatting".into(),
        short_label: "Formatting".into(),
        detail,
        busy: false,
        source: String::new(),
        command: Some(json!({ "id": super::format::CONFIGURE_DEFAULT_FORMATTER, "title": "Configure..." })),
    }
}

/// `_asStatusbarEntry`: a pinned status as its own entry.
pub(super) fn dedicated_entry(status: &Status) -> StatusEntry {
    let tooltip = status.command.as_ref().and_then(|command| command["tooltip"].as_str()).filter(|tooltip| !tooltip.is_empty()).map(str::to_owned);
    let tooltip = tooltip.or_else(|| Some(linked_text_label(&status.detail)).filter(|detail| !detail.is_empty()));
    StatusEntry {
        entry_id: format!("{ID}.{}", status.id),
        id: status.id.clone(),
        name: format!("{} (Language Status)", status.name),
        text: compute_text(&status.short_label, status.busy),
        tooltip,
        command: status.command.as_ref().map(|command| json!({ "id": command["id"], "arguments": command["arguments"] })),
        color: None,
        background_color: None,
        kind: match status.severity {
            ERROR => Some("error"),
            WARNING => Some("warning"),
            _ => None,
        },
        align_left: false,
        priority: EDITOR_STATUS_PRIORITY,
        secondary: 0,
        show_progress: false,
    }
}

/// One `_renderStatus` row of the combined entry's hover.
#[derive(Debug, PartialEq)]
pub(super) struct HoverRow {
    /// `_severityToSingleCodicon` and its color key, shown when the first status is a warning or an error.
    pub(super) severity: Option<(&'static str, Option<&'static str>)>,
    pub(super) label: String,
    pub(super) detail: String,
    /// The command link: title, id and arguments.
    pub(super) command: Option<(String, String, Vec<Value>)>,
    pub(super) pinned: bool,
    pub(super) id: String,
}

/// The rows of the combined entry's hover, in view model order.
pub(super) fn hover_rows(view: &LanguageStatusView) -> Vec<HoverRow> {
    let show_severity = view.show_severity();
    let text = |value: &Value| value.as_str().unwrap_or_default().to_owned();
    let mut rows = Vec::new();
    for status in &view.combined {
        let severity = match status.severity {
            ERROR => ("error", Some("notificationsErrorIcon.foreground")),
            WARNING => ("info", Some("notificationsInfoIcon.foreground")),
            _ => ("check", None),
        };
        let command = status.command.as_ref().map(|command| (text(&command["title"]), text(&command["id"]), command["arguments"].as_array().cloned().unwrap_or_default()));
        rows.push(HoverRow {
            severity: show_severity.then_some(severity),
            label: compute_text(&status.label, status.busy),
            detail: status.detail.clone(),
            command,
            pinned: view.dedicated.contains(status),
            id: status.id.clone(),
        });
    }
    rows
}

/// `_renderTextPlus` as plain text: links show their label.
fn linked_text_label(text: &str) -> String {
    let mut label = String::new();
    for node in parse_linked_text(text) {
        match node {
            Node::Text(text) => label.push_str(&text),
            Node::Link { label: link, .. } => label.push_str(&link),
        }
    }
    label
}

/// The width of a `$(icon)` label: its text plus 16px per icon.
fn label_width(label: &str, window: &Window) -> f32 {
    let chars: Vec<char> = label.chars().collect();
    let (mut text, mut icons, mut index) = (String::new(), 0, 0);
    while index < chars.len() {
        if let Some((_, length)) = icon_at(&chars[index..]) {
            icons += 1;
            index += length;
            continue;
        }
        text.push(chars[index]);
        index += 1;
    }
    text_width(&text, false, window) + 16. * icons as f32
}

/// `computeText`.
fn compute_text(text: &str, busy: bool) -> String {
    match (text.is_empty(), busy) {
        (false, true) => format!("{text}\u{a0}\u{a0}{BUSY}"),
        (true, true) => BUSY.to_owned(),
        _ => text.to_owned(),
    }
}

/// The codicon SVG of the combined entry's text; codiconsLibrary.ts makes `bracket` an alias of `json`.
fn codicon_file(text: &str) -> &'static str {
    match text {
        BUSY => "loading",
        "$(bracket-error)" => "bracket-error",
        "$(bracket-dot)" => "bracket-dot",
        _ => "json",
    }
}

/// languageStatus.css `wiggle`: turns 0.04 and back while growing to 1.2x, over 0.8s.
// shortcut: linear between keyframes where CSS eases each one; ease them if the motion looks off next to VS Code.
fn wiggle(delta: f32) -> Transformation {
    const FRAMES: [(f32, f32, f32); 6] = [(0., 0., 1.), (0.15, 0.04, 1.1), (0.3, -0.04, 1.2), (0.45, 0.04, 1.1), (0.6, -0.04, 1.2), (1., 0., 1.)];
    let mut turn = 0.;
    let mut scale = 1.;
    for pair in FRAMES.windows(2) {
        let ((start, from_turn, from_scale), (end, to_turn, to_scale)) = (pair[0], pair[1]);
        if delta <= end {
            let t = (delta - start) / (end - start);
            turn = from_turn + (to_turn - from_turn) * t;
            scale = from_scale + (to_scale - from_scale) * t;
            break;
        }
    }
    Transformation::rotate(percentage(turn)).with_scaling(size(scale, scale))
}
