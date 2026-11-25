//! platform/accessibilitySignal: the signals of accessibilitySignalService.ts with their sounds and settings,
//! `checkEnabledState`, the volume, and playback of the upstream mp3 files through NSSound.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_app_kit::NSSound;
use objc2_foundation::NSString;
use serde_json::Value;

pub struct Signal {
    pub name: &'static str,
    /// `SoundSource.randomOneOf`: file names under accessibilitySignal/browser/media.
    pub sounds: &'static [&'static str],
    pub settings_key: &'static str,
    pub announcement: Option<&'static str>,
    /// Signals with a `legacyAnnouncementSettingsKey` are the ones Help: List Signal Announcements lists.
    pub has_legacy_announcement: bool,
}

const fn signal(name: &'static str, sounds: &'static [&'static str], key: &'static str, announcement: Option<&'static str>, has_legacy_announcement: bool) -> Signal {
    Signal { name, sounds, settings_key: key, announcement, has_legacy_announcement }
}

/// `AccessibilitySignal.allAccessibilitySignals`, in registration order.
pub const SIGNALS: &[Signal] = &[
    signal("Error at Position", &["error.mp3"], "accessibility.signals.positionHasError", Some("Error"), false),
    signal("Warning at Position", &["warning.mp3"], "accessibility.signals.positionHasWarning", Some("Warning"), false),
    signal("Error on Line", &["error.mp3"], "accessibility.signals.lineHasError", Some("Error on Line"), true),
    signal("Warning on Line", &["warning.mp3"], "accessibility.signals.lineHasWarning", Some("Warning on Line"), true),
    signal("Folded Area on Line", &["foldedAreas.mp3"], "accessibility.signals.lineHasFoldedArea", Some("Folded"), true),
    signal("Breakpoint on Line", &["break.mp3"], "accessibility.signals.lineHasBreakpoint", Some("Breakpoint"), true),
    signal("Inline Suggestion on Line", &["quickFixes.mp3"], "accessibility.signals.lineHasInlineSuggestion", None, false),
    signal("Next Edit Suggestion on Line", &["nextEditSuggestion.mp3"], "accessibility.signals.nextEditSuggestion", Some("Next Edit Suggestion"), false),
    signal("Terminal Quick Fix", &["quickFixes.mp3"], "accessibility.signals.terminalQuickFix", Some("Quick Fix"), true),
    signal("Debugger Stopped on Breakpoint", &["break.mp3"], "accessibility.signals.onDebugBreak", Some("Breakpoint"), true),
    signal("No Inlay Hints on Line", &["error.mp3"], "accessibility.signals.noInlayHints", Some("No Inlay Hints"), true),
    signal("Task Completed", &["taskCompleted.mp3"], "accessibility.signals.taskCompleted", Some("Task Completed"), true),
    signal("Task Failed", &["taskFailed.mp3"], "accessibility.signals.taskFailed", Some("Task Failed"), true),
    signal("Terminal Command Failed", &["error.mp3"], "accessibility.signals.terminalCommandFailed", Some("Command Failed"), true),
    signal("Terminal Command Succeeded", &["terminalCommandSucceeded.mp3"], "accessibility.signals.terminalCommandSucceeded", Some("Command Succeeded"), false),
    signal("Terminal Bell", &["terminalBell.mp3"], "accessibility.signals.terminalBell", Some("Terminal Bell"), true),
    signal("Notebook Cell Completed", &["taskCompleted.mp3"], "accessibility.signals.notebookCellCompleted", Some("Notebook Cell Completed"), true),
    signal("Notebook Cell Failed", &["taskFailed.mp3"], "accessibility.signals.notebookCellFailed", Some("Notebook Cell Failed"), true),
    signal("Diff Line Inserted", &["diffLineInserted.mp3"], "accessibility.signals.diffLineInserted", None, false),
    signal("Diff Line Deleted", &["diffLineDeleted.mp3"], "accessibility.signals.diffLineDeleted", None, false),
    signal("Diff Line Modified", &["diffLineModified.mp3"], "accessibility.signals.diffLineModified", None, false),
    signal("Chat Edit Modified File", &["chatEditModifiedFile.mp3"], "accessibility.signals.chatEditModifiedFile", Some("File Modified from Chat Edits"), false),
    signal("Chat Request Sent", &["requestSent.mp3"], "accessibility.signals.chatRequestSent", Some("Chat Request Sent"), true),
    signal("Chat Response Received", &["responseReceived1.mp3", "responseReceived2.mp3", "responseReceived3.mp3", "responseReceived4.mp3"], "accessibility.signals.chatResponseReceived", None, false),
    signal("Confetti", &["confetti.mp3"], "accessibility.signals.confetti", None, false),
    signal("Code Action Request Triggered", &["codeActionTriggered.mp3"], "accessibility.signals.codeActionTriggered", Some("Code Action Request Triggered"), true),
    signal("Code Action Applied", &["codeActionApplied.mp3"], "accessibility.signals.codeActionApplied", None, false),
    signal("Progress", &["progress.mp3"], "accessibility.signals.progress", Some("Progress"), true),
    signal("Clear", &["clear.mp3"], "accessibility.signals.clear", Some("Clear"), true),
    signal("Save", &["save.mp3"], "accessibility.signals.save", Some("Save"), true),
    signal("Format", &["format.mp3"], "accessibility.signals.format", Some("Format"), true),
    signal("Voice Recording Started", &["voiceRecordingStarted.mp3"], "accessibility.signals.voiceRecordingStarted", None, false),
    signal("Voice Mode Started", &["voiceRecordingStarted.mp3"], "accessibility.signals.voiceModeStarted", Some("Voice Mode Started"), false),
    signal("Voice Recording Stopped", &["voiceRecordingStopped.mp3"], "accessibility.signals.voiceRecordingStopped", None, false),
    signal("Voice Mode Stopped", &["voiceRecordingStopped.mp3"], "accessibility.signals.voiceModeStopped", Some("Voice Mode Stopped"), false),
    signal("Edits Kept", &["editsKept.mp3"], "accessibility.signals.editsKept", Some("Edits Kept"), false),
    signal("Undo Edits", &["editsUndone.mp3"], "accessibility.signals.editsUndone", Some("Edits Undone"), false),
    signal("Chat User Action Required", &["chatUserActionRequired.mp3"], "accessibility.signals.chatUserActionRequired", Some("Chat User Action Required"), false),
];

pub fn signal_by_key(settings_key: &str) -> &'static Signal {
    SIGNALS.iter().find(|signal| signal.settings_key == settings_key).unwrap_or_else(|| panic!("unknown signal {settings_key}"))
}

#[derive(Clone, Copy, PartialEq)]
pub enum Modality {
    Sound,
    Announcement,
}

/// `checkEnabledState` on the signal's setting (`{ sound, announcement }`, both `off` when unset).
pub fn is_enabled(setting: &Value, modality: Modality, screen_reader_attached: bool, user_gesture: bool) -> bool {
    let field = match modality {
        Modality::Sound => "sound",
        Modality::Announcement => "announcement",
    };
    match setting[field].as_str().unwrap_or("off") {
        "on" | "always" => true,
        "auto" => screen_reader_attached,
        "userGesture" => user_gesture,
        _ => false,
    }
}

/// `getVolumeInPercent`: `accessibility.signalOptions.volume` clamped to 0..100, 50 when not a number.
pub fn volume_percent(setting: &Value) -> f64 {
    setting.as_f64().map_or(50., |volume| volume.clamp(0., 100.))
}

/// `AccessibilitySignalService.playSound`: one NSSound per file, kept and replayed from the start as the service
/// keeps its `HTMLAudioElement`s.
pub struct SoundPlayer {
    media: PathBuf,
    sounds: HashMap<&'static str, Retained<NSSound>>,
}

impl SoundPlayer {
    pub fn new(app_root: &Path) -> Self {
        SoundPlayer { media: app_root.join("src/vs/platform/accessibilitySignal/browser/media"), sounds: HashMap::new() }
    }

    pub fn play(&mut self, file_name: &'static str, volume_percent: f64) {
        if !self.sounds.contains_key(file_name) {
            let path = NSString::from_str(&self.media.join(file_name).to_string_lossy());
            let Some(sound) = NSSound::initWithContentsOfFile_byReference(NSSound::alloc(), &path, true) else {
                eprintln!("Error while playing sound: cannot read {file_name}");
                return;
            };
            self.sounds.insert(file_name, sound);
        }
        let sound = &self.sounds[file_name];
        sound.stop();
        sound.setCurrentTime(0.);
        sound.setVolume((volume_percent / 100.) as f32);
        sound.play();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn enabled_states_follow_check_enabled_state() {
        let setting = json!({ "sound": "auto", "announcement": "userGesture" });
        assert!(!is_enabled(&setting, Modality::Sound, false, false));
        assert!(is_enabled(&setting, Modality::Sound, true, false));
        assert!(!is_enabled(&setting, Modality::Announcement, true, false));
        assert!(is_enabled(&setting, Modality::Announcement, false, true));
        assert!(is_enabled(&json!({ "sound": "always" }), Modality::Sound, false, false));
        assert!(is_enabled(&json!({ "sound": "on" }), Modality::Sound, false, false));
        assert!(!is_enabled(&json!({ "sound": "never" }), Modality::Sound, true, true));
        assert!(!is_enabled(&json!({}), Modality::Announcement, true, true));
    }

    #[test]
    fn volume_is_clamped_with_a_default() {
        assert_eq!(volume_percent(&json!(70)), 70.);
        assert_eq!(volume_percent(&json!(140)), 100.);
        assert_eq!(volume_percent(&json!(-3)), 0.);
        assert_eq!(volume_percent(&Value::Null), 50.);
    }

    #[test]
    fn every_sound_file_exists_upstream() {
        let media = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../src/vs/platform/accessibilitySignal/browser/media");
        for signal in SIGNALS {
            for sound in signal.sounds {
                assert!(media.join(sound).is_file(), "{sound}");
            }
        }
    }
}
