//! languageStatus.ts and languageStatusService.ts: the statuses extensions set for a language (`$setLanguageStatus`)
//! show as one `{}` entry next to the language entry, whose hover lists them; a pinned status gets its own entry.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{Animation, AnimationExt, AnyElement, Bounds, Context, Pixels, Rgba, Transformation, canvas, div, percentage, prelude::*, px, size, svg};
use serde_json::{Value, json};

use super::Workbench;
use super::hover::{HoverContent, HoverOptions};
use super::status_entries::EDITOR_STATUS_PRIORITY;
use crate::command_palette::strip_icons;
use crate::commands::Outcome;
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
        let content = HoverContent::Markdown(hover_markdown(view));
        let bounds = state.bounds.clone();
        let anchor = state.bounds.clone();
        let item = div()
            .id(ID)
            .relative()
            .h_full()
            .flex_none()
            .mx(px(3.))
            .px(px(5.))
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

/// `_renderStatus` of each status as one markdown paragraph: label, detail, command link, pin link.
// shortcut: the hover is markdown text, so `$(icon)`s, the severity icon and the busy spinner are dropped and pin is
// a text link; draw them once HoverContent carries icons.
pub(super) fn hover_markdown(view: &LanguageStatusView) -> String {
    let mut rows = Vec::new();
    for status in &view.combined {
        let mut row = strip_icons(&status.label).trim().to_owned();
        if !status.detail.is_empty() {
            row.push_str(" \u{2013} ");
            row.push_str(&strip_icons(&status.detail));
        }
        if let Some(command) = &status.command {
            let title = command["title"].as_str().unwrap_or_default();
            row.push_str(&format!("   {}", command_link(title, command["id"].as_str().unwrap_or_default(), &command["arguments"])));
        }
        let pinned = view.dedicated.contains(status);
        let (title, id) = if pinned { ("Remove from Status Bar", UNPIN) } else { ("Add to Status Bar", PIN) };
        row.push_str(&format!("   {}", command_link(title, id, &json!([status.id]))));
        rows.push(row);
    }
    rows.join("\n\n")
}

/// A linked text `[title](command:id?args)` with the arguments URI-encoded, as `URI.from({ scheme: 'command' })`.
fn command_link(title: &str, id: &str, arguments: &Value) -> String {
    let query = match arguments {
        Value::Null => String::new(),
        arguments => format!("?{}", encode_uri_component(&arguments.to_string())),
    };
    format!("[{title}](command:{id}{query})")
}

/// `encodeURIComponent`, also escaping the parentheses that end a linked text's href.
fn encode_uri_component(text: &str) -> String {
    let mut encoded = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' => encoded.push(byte as char),
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
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
