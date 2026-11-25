//! editorZoom.ts and mouseHandler.ts `_setupMouseWheelZoomListener`: one zoom level for every editor, from -5
//! to 20, each level adding 10% to the font size and line height (fontInfo.ts).

use std::time::{Duration, Instant};

#[derive(Default)]
pub struct EditorZoom {
    level: f32,
    gesture: Option<WheelGesture>,
}

/// Wheel events less than 50ms apart are one trackpad gesture, zooming from where it started.
struct WheelGesture {
    last_event: Instant,
    start_level: f32,
    has_zoom_modifiers: bool,
    accumulated_delta: f32,
}

impl EditorZoom {
    pub fn level(&self) -> f32 {
        self.level
    }

    /// Returns whether the level changed.
    pub fn set_level(&mut self, level: f32) -> bool {
        let level = level.clamp(-5., 20.);
        let changed = level != self.level;
        self.level = level;
        changed
    }

    /// A wheel event with `editor.mouseWheelZoom` on, `delta_y` in `StandardWheelEvent` units (positive up).
    /// A mouse notch with Cmd or Ctrl zooms one level; a trackpad gesture that began with them zooms by a fifth
    /// of its accumulated delta. Returns whether the event zoomed, so the editor does not scroll.
    pub fn wheel(&mut self, delta_y: f32, physical_wheel: bool, has_zoom_modifiers: bool, now: Instant) -> bool {
        if physical_wheel {
            if !has_zoom_modifiers {
                return false;
            }
            self.set_level(self.level + if delta_y > 0. { 1. } else { -1. });
            return true;
        }
        let level = self.level;
        let gesture = match self.gesture.as_mut() {
            Some(gesture) if now.duration_since(gesture.last_event) <= Duration::from_millis(50) => gesture,
            _ => self.gesture.insert(WheelGesture { last_event: now, start_level: level, has_zoom_modifiers, accumulated_delta: 0. }),
        };
        gesture.last_event = now;
        gesture.accumulated_delta += delta_y;
        if !gesture.has_zoom_modifiers {
            return false;
        }
        let level = gesture.start_level + gesture.accumulated_delta / 5.;
        self.set_level(level);
        true
    }

    /// `editorZoomLevelMultiplier` for the font size and line height.
    pub fn multiplier(&self) -> f32 {
        1. + self.level * 0.1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_level_stays_between_minus_five_and_twenty() {
        let mut zoom = EditorZoom::default();
        assert!(zoom.set_level(1.));
        assert_eq!(zoom.multiplier(), 1.1);
        assert!(!zoom.set_level(1.));
        zoom.set_level(-9.);
        assert_eq!(zoom.level(), -5.);
        zoom.set_level(30.);
        assert_eq!(zoom.level(), 20.);
    }

    #[test]
    fn a_mouse_notch_zooms_one_level_only_with_the_modifier() {
        let mut zoom = EditorZoom::default();
        let now = Instant::now();
        assert!(!zoom.wheel(1., true, false, now));
        assert!(zoom.wheel(1., true, true, now));
        assert!(zoom.wheel(3., true, true, now));
        assert!(zoom.wheel(-0.5, true, true, now));
        assert_eq!(zoom.level(), 1.);
    }

    #[test]
    fn a_trackpad_gesture_zooms_by_a_fifth_of_its_delta_from_where_it_started() {
        let mut zoom = EditorZoom::default();
        let start = Instant::now();
        assert!(zoom.wheel(2., false, true, start));
        assert!(zoom.wheel(3., false, false, start + Duration::from_millis(40)));
        assert_eq!(zoom.level(), 1.);
        // A gesture that began without the modifier keeps scrolling even once it is held.
        let later = start + Duration::from_millis(200);
        assert!(!zoom.wheel(5., false, false, later));
        assert!(!zoom.wheel(5., false, true, later + Duration::from_millis(10)));
        assert_eq!(zoom.level(), 1.);
    }
}
