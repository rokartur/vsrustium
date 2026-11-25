//! terminalNotificationHandler.ts: OSC 99 desktop notifications (kitty's protocol) as workbench notifications. The
//! handler keeps the protocol state and answers with effects; the workbench shows, updates and closes the
//! notifications and calls back when one of them is clicked or closed.

use std::collections::HashMap;

use base64::Engine;

use crate::notifications::Priority;

/// What the terminal's host knows when a sequence arrives (`IOsc99NotificationHost`).
pub struct Env {
    pub enabled: bool,
    pub window_focused: bool,
    pub terminal_visible: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    /// The handler's key of the notification, given back with `button`, `dismissed`, `auto_closed` and `closed`.
    pub key: u64,
    pub id: Option<String>,
    pub message: String,
    /// `u=2`: Warning, else Info.
    pub warning: bool,
    pub priority: Option<Priority>,
    pub buttons: Vec<String>,
    pub auto_close_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    Show(Shown),
    /// An active notification with the same id gets the new message, severity, buttons and auto close.
    Update(Shown),
    Close(u64),
    Focus,
    /// Text for the process (`sendText(data, false)`).
    Write(String),
}

#[derive(Clone, Copy, PartialEq)]
enum Occasion {
    Always,
    Unfocused,
    Invisible,
}

#[derive(Clone, Copy, PartialEq)]
enum CloseReason {
    Button,
    Secondary,
    Auto,
    Protocol,
}

struct State {
    id: Option<String>,
    title: String,
    body: String,
    buttons: String,
    focus_on_activate: bool,
    report_on_activate: bool,
    report_on_close: bool,
    urgency: Option<i64>,
    auto_close_ms: Option<i64>,
    occasion: Option<Occasion>,
}

struct Active {
    key: u64,
    id: Option<String>,
    focus_on_activate: bool,
    report_on_activate: bool,
    report_on_close: bool,
    close_reason: Option<CloseReason>,
}

#[derive(Default)]
pub struct Osc99 {
    pending: HashMap<String, State>,
    pending_anonymous: Option<State>,
    active: Vec<Active>,
    next_key: u64,
}

impl Osc99 {
    /// `handleSequence`: the data after `99;`.
    pub fn handle(&mut self, data: &str, env: &Env) -> Vec<Effect> {
        let (metadata, payload) = data.split_once(';').unwrap_or((data, ""));
        let entries = parse_metadata(metadata);
        let last = |key: &str| entries.get(key).and_then(|values| values.last()).map(String::as_str);
        let payload_type = last("p").filter(|kind| !kind.is_empty()).unwrap_or("title");
        let id = entries.get("i").and_then(|values| values.first()).and_then(|id| sanitize_id(id));
        if !env.enabled {
            return Vec::new();
        }
        match payload_type {
            "?" => return vec![query_response(id.as_deref())],
            "alive" => {
                let alive: Vec<&str> = self.active.iter().filter_map(|active| active.id.as_deref()).collect();
                return vec![response(&[&format!("i={}", id.as_deref().unwrap_or("0")), "p=alive"], &alive.join(","))];
            }
            "close" => return self.close_by_id(id),
            _ => {}
        }
        let state = match &id {
            Some(id) => self.pending.entry(id.clone()).or_insert_with(|| State::new(Some(id.clone()))),
            None => self.pending_anonymous.get_or_insert_with(|| State::new(None)),
        };
        state.update(&entries);
        let text = match last("e") == Some("1") {
            true => base64::engine::general_purpose::STANDARD.decode(payload).ok().and_then(|bytes| String::from_utf8(bytes).ok()).unwrap_or_default(),
            false => payload.to_owned(),
        };
        match payload_type {
            "title" => state.title.push_str(&text),
            "body" => state.body.push_str(&text),
            "buttons" => state.buttons.push_str(&text),
            _ => return Vec::new(),
        }
        if entries.get("d").and_then(|values| values.first()).map(String::as_str) == Some("0") {
            return Vec::new();
        }
        let honored = match state.occasion {
            None | Some(Occasion::Always) => true,
            Some(Occasion::Unfocused) => !env.window_focused,
            Some(Occasion::Invisible) => !env.window_focused && !env.terminal_visible,
        };
        let Some(message) = honored.then(|| state.message()).flatten() else {
            if !honored {
                self.clear_pending(&id);
            }
            return Vec::new();
        };
        let mut shown = Shown {
            key: 0,
            id: state.id.clone(),
            message,
            warning: state.urgency == Some(2),
            priority: match state.urgency {
                Some(0) => Some(Priority::Silent),
                Some(1) => Some(Priority::Default),
                Some(2) => Some(Priority::Urgent),
                _ => None,
            },
            buttons: if state.buttons.is_empty() { Vec::new() } else { state.buttons.split('\u{2028}').map(str::to_owned).collect() },
            auto_close_ms: state.auto_close_ms.filter(|ms| *ms > 0).map(|ms| ms as u64),
        };
        let flags = (state.focus_on_activate, state.report_on_activate, state.report_on_close);
        self.clear_pending(&id);
        if let Some(active) = id.as_ref().and_then(|id| self.active.iter_mut().find(|active| active.id.as_ref() == Some(id))) {
            (active.focus_on_activate, active.report_on_activate, active.report_on_close) = flags;
            shown.key = active.key;
            return vec![Effect::Update(shown)];
        }
        self.next_key += 1;
        shown.key = self.next_key;
        let (focus_on_activate, report_on_activate, report_on_close) = flags;
        self.active.push(Active { key: shown.key, id, focus_on_activate, report_on_activate, report_on_close, close_reason: None });
        vec![Effect::Show(shown)]
    }

    /// A button (1-based, as reported) was clicked: the activation, then the notification closes.
    pub fn button(&mut self, key: u64, index: usize) -> Vec<Effect> {
        let Some(active) = self.active.iter_mut().find(|active| active.key == key) else { return Vec::new() };
        active.close_reason = Some(CloseReason::Button);
        let mut effects = Vec::new();
        // `reportActivation` reads the pending state's flags, which match the active ones once shown.
        if active.focus_on_activate {
            effects.push(Effect::Focus);
        }
        if active.report_on_activate {
            effects.push(response(&[&format!("i={}", active.id.as_deref().unwrap_or("0"))], &index.to_string()));
        }
        effects.push(Effect::Close(key));
        effects
    }

    /// Dismiss or Disable Terminal Notifications: closes without an activation.
    pub fn dismissed(&mut self, key: u64) -> Vec<Effect> {
        self.set_reason(key, CloseReason::Secondary);
        vec![Effect::Close(key)]
    }

    /// The `w=` timeout passed.
    pub fn auto_closed(&mut self, key: u64) -> Vec<Effect> {
        self.set_reason(key, CloseReason::Auto);
        vec![Effect::Close(key)]
    }

    /// `onDidClose`: a close without a reason (the close button) counts as an activation when one is reported.
    pub fn closed(&mut self, key: u64) -> Vec<Effect> {
        let Some(index) = self.active.iter().position(|active| active.key == key) else { return Vec::new() };
        let active = self.active.remove(index);
        let id = active.id.as_deref().unwrap_or("0");
        let mut effects = Vec::new();
        if active.report_on_activate && active.close_reason.is_none() {
            if active.focus_on_activate {
                effects.push(Effect::Focus);
            }
            effects.push(response(&[&format!("i={id}")], ""));
        }
        if active.report_on_close {
            effects.push(response(&[&format!("i={id}"), "p=close"], ""));
        }
        effects
    }

    fn close_by_id(&mut self, id: Option<String>) -> Vec<Effect> {
        let Some(id) = id else { return Vec::new() };
        self.pending.remove(&id);
        let Some(active) = self.active.iter_mut().find(|active| active.id.as_ref() == Some(&id)) else { return Vec::new() };
        active.close_reason = Some(CloseReason::Protocol);
        vec![Effect::Close(active.key)]
    }

    fn set_reason(&mut self, key: u64, reason: CloseReason) {
        if let Some(active) = self.active.iter_mut().find(|active| active.key == key) {
            active.close_reason = Some(reason);
        }
    }

    fn clear_pending(&mut self, id: &Option<String>) {
        match id {
            Some(id) => _ = self.pending.remove(id),
            None => self.pending_anonymous = None,
        }
    }
}

impl State {
    fn new(id: Option<String>) -> State {
        State { id, title: String::new(), body: String::new(), buttons: String::new(), focus_on_activate: true, report_on_activate: false, report_on_close: false, urgency: None, auto_close_ms: None, occasion: None }
    }

    /// `_updateOsc99StateFromMetadata`: the last value of each key wins.
    fn update(&mut self, entries: &HashMap<String, Vec<String>>) {
        let last = |key: &str| entries.get(key).and_then(|values| values.last()).map(String::as_str);
        if let Some(actions) = last("a") {
            (self.focus_on_activate, self.report_on_activate) = (true, false);
            for token in actions.split(',') {
                match token {
                    "focus" => self.focus_on_activate = true,
                    "-focus" => self.focus_on_activate = false,
                    "report" => self.report_on_activate = true,
                    "-report" => self.report_on_activate = false,
                    _ => {}
                }
            }
        }
        if let Some(close) = last("c") {
            self.report_on_close = close == "1";
        }
        if let Some(urgency) = last("u").and_then(parse_int) {
            self.urgency = Some(urgency);
        }
        if let Some(ms) = last("w").and_then(parse_int) {
            self.auto_close_ms = Some(ms);
        }
        match last("o") {
            Some("always") => self.occasion = Some(Occasion::Always),
            Some("unfocused") => self.occasion = Some(Occasion::Unfocused),
            Some("invisible") => self.occasion = Some(Occasion::Invisible),
            _ => {}
        }
    }

    /// `_getOsc99NotificationMessage`.
    fn message(&self) -> Option<String> {
        let (title, body) = (sanitize_text(&self.title), sanitize_text(&self.body));
        match (title.trim().is_empty(), body.trim().is_empty()) {
            (false, false) => Some(format!("{title}: {body}")),
            (false, true) => Some(title),
            (true, false) => Some(body),
            (true, true) => None,
        }
    }
}

fn parse_metadata(metadata: &str) -> HashMap<String, Vec<String>> {
    let mut result: HashMap<String, Vec<String>> = HashMap::new();
    for entry in metadata.split(':') {
        let Some((key, value)) = entry.split_once('=') else { continue };
        if !key.is_empty() {
            result.entry(key.to_owned()).or_default().push(value.to_owned());
        }
    }
    result
}

/// `Number.parseInt(value, 10)`: the leading integer, if any.
fn parse_int(value: &str) -> Option<i64> {
    let value = value.trim_start();
    let sign_length = usize::from(value.starts_with(['-', '+']));
    let digits = value[sign_length..].chars().take_while(char::is_ascii_digit).count();
    value[..sign_length + digits].parse().ok()
}

/// `_sanitizeOsc99Id`: `[a-zA-Z0-9_\-+.]` only.
fn sanitize_id(raw: &str) -> Option<String> {
    let sanitized: String = raw.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+' | '.')).collect();
    (!sanitized.is_empty()).then_some(sanitized)
}

/// `_sanitizeOsc99MessageText`: `[label](target)` becomes `label`, repeated until no link is left, as nested links
/// reassemble after one pass.
fn sanitize_text(text: &str) -> String {
    let mut sanitized = text.to_owned();
    loop {
        let next = strip_links_once(&sanitized);
        if next == sanitized {
            return sanitized;
        }
        sanitized = next;
    }
}

/// One `replace(/\[([^\]]+)\]\(([^)]+)\)/g, '$1')`.
fn strip_links_once(text: &str) -> String {
    let mut result = String::new();
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let after = &rest[open + 1..];
        let link = after.find(']').filter(|close| *close > 0).and_then(|close| {
            let target = after[close + 1..].strip_prefix('(')?;
            let end = target.find(')').filter(|end| *end > 0)?;
            Some((close, close + 2 + end + 1))
        });
        match link {
            Some((close, consumed)) => {
                result.push_str(&rest[..open]);
                result.push_str(&after[..close]);
                rest = &after[consumed..];
            }
            None => {
                result.push_str(&rest[..open + 1]);
                rest = after;
            }
        }
    }
    result.push_str(rest);
    result
}

fn query_response(id: Option<&str>) -> Effect {
    let id = format!("i={}", id.unwrap_or("0"));
    response(&[&id, "p=?", "a=report,focus", "c=1", "o=always,unfocused,invisible", "p=title,body,buttons,close,alive,?", "u=0,1,2", "w=1"], "")
}

fn response(metadata: &[&str], payload: &str) -> Effect {
    Effect::Write(format!("\x1b]99;{};{payload}\x1b\\", metadata.join(":")))
}

#[cfg(test)]
mod tests;
