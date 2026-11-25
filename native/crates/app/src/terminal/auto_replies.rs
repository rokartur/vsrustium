//! terminalAutoResponder.ts: `terminal.integrated.autoReplies` answers a matched word in the process output.

use std::time::{Duration, Instant};

use serde_json::Value;

/// Each reply is throttled by a second (`timeout(1000)`).
const THROTTLE: Duration = Duration::from_secs(1);

pub struct AutoResponder {
    match_word: Vec<char>,
    response: String,
    pointer: usize,
    throttled_until: Option<Instant>,
}

impl AutoResponder {
    /// `onProcessData`: the response once the data completes the match word; a mismatched character restarts
    /// the match without being matched again, as upstream.
    pub fn feed(&mut self, data: &str, now: Instant) -> Option<String> {
        if self.throttled_until.is_some_and(|until| now < until) {
            return None;
        }
        let mut reply = None;
        for char in data.chars() {
            if char == self.match_word[self.pointer] {
                self.pointer += 1;
            } else {
                self.pointer = 0;
            }
            if self.pointer == self.match_word.len() {
                reply = Some(self.response.clone());
                self.throttled_until = Some(now + THROTTLE);
                self.pointer = 0;
            }
        }
        reply
    }
}

/// The responders of the setting's object: a `null` reply or an empty word removes the entry.
pub fn responders(setting: &Value) -> Vec<AutoResponder> {
    let Some(replies) = setting.as_object() else { return Vec::new() };
    let mut responders = Vec::new();
    for (word, reply) in replies {
        let Some(response) = reply.as_str() else { continue };
        if word.is_empty() {
            continue;
        }
        responders.push(AutoResponder { match_word: word.chars().collect(), response: response.to_owned(), pointer: 0, throttled_until: None });
    }
    responders
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_split_match_replies_once_per_second() {
        let mut responders = responders(&json!({ "Terminate batch job (Y/N)": "Y\r", "ignored": null }));
        assert_eq!(responders.len(), 1);
        let responder = &mut responders[0];
        let start = Instant::now();
        assert_eq!(responder.feed("^CTerminate batch", start), None);
        assert_eq!(responder.feed(" job (Y/N)", start).as_deref(), Some("Y\r"));
        assert_eq!(responder.feed("Terminate batch job (Y/N)", start + Duration::from_millis(500)), None);
        assert_eq!(responder.feed("Terminate batch job (Y/N)", start + Duration::from_secs(2)).as_deref(), Some("Y\r"));
    }
}
