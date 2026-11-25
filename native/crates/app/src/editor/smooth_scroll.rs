//! Smooth scrolling animation, as scrollable.ts `SmoothScrollingOperation` runs it. Plain data: the
//! caller passes `Instant`s in and applies the returned scroll position.

use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SmoothScroll {
    pub from: (f32, f32),
    pub to: (f32, f32),
    pub viewport: (f32, f32),
    pub start: Instant,
    pub duration: Duration,
}

/// viewLayout.ts `SMOOTH_SCROLLING_TIME`.
pub const SMOOTH_SCROLLING_TIME: Duration = Duration::from_millis(125);

impl SmoothScroll {
    /// `SmoothScrollingOperation.start`, also its `combine` from the current position: (top, left) pairs, 10ms
    /// longer and started 10ms ago for a quicker response.
    pub fn start(from: (f32, f32), to: (f32, f32), viewport: (f32, f32), now: Instant) -> Self {
        let pretend_started = Duration::from_millis(10);
        SmoothScroll { from, to, viewport, start: now - pretend_started, duration: SMOOTH_SCROLLING_TIME + pretend_started }
    }

    /// `SmoothScrollingOperation._tick`: returns `(top, left, done)`.
    pub fn tick(&self, now: Instant) -> (f32, f32, bool) {
        let elapsed = now.saturating_duration_since(self.start);
        let completion = elapsed.as_secs_f32() / self.duration.as_secs_f32();
        if completion < 1.0 {
            let top = animate(self.from.0, self.to.0, self.viewport.0, completion);
            let left = animate(self.from.1, self.to.1, self.viewport.1, completion);
            return (top, left, false);
        }
        (self.to.0, self.to.1, true)
    }
}

/// `SmoothScrollingOperation._initAnimation`: over 2.5 viewports, skip the middle between two stops.
fn animate(from: f32, to: f32, viewport_size: f32, completion: f32) -> f32 {
    if (from - to).abs() <= 2.5 * viewport_size {
        return ease_out_cubic_between(from, to, completion);
    }
    let edge = 0.75 * viewport_size * (to - from).signum();
    composed(from, from + edge, to - edge, to, 0.33, completion)
}

/// `createComposed` of two `createEaseOutCubic` animations.
fn composed(from: f32, stop1: f32, stop2: f32, to: f32, cut: f32, completion: f32) -> f32 {
    if completion < cut {
        return ease_out_cubic_between(from, stop1, completion / cut);
    }
    ease_out_cubic_between(stop2, to, (completion - cut) / (1.0 - cut))
}

/// `createEaseOutCubic`.
fn ease_out_cubic_between(from: f32, to: f32, completion: f32) -> f32 {
    from + (to - from) * ease_out_cubic(completion)
}

fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

#[cfg(test)]
mod tests;
