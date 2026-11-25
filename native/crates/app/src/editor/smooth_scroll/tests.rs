use super::*;

const VIEWPORT_HEIGHT: f32 = 800.0;
const LINE_HEIGHT: f32 = 20.0;

fn simulate_smooth_scroll(from: f32, to: f32) -> Vec<(i32, i32)> {
    let now = Instant::now();
    let scroll = SmoothScroll::start((from, 0.), (to, 0.), (VIEWPORT_HEIGHT, 0.), now);
    let mut lines = Vec::new();
    for ms in [0, 25, 50, 75, 100, 125] {
        let (top, _, _) = scroll.tick(now + Duration::from_millis(ms));
        let start_line = (top / LINE_HEIGHT).floor() as i32;
        let end_line = ((top + VIEWPORT_HEIGHT) / LINE_HEIGHT).ceil() as i32;
        lines.push((start_line, end_line));
    }
    lines
}

#[test]
fn scroll_25_lines_40_fit() {
    assert_eq!(
        simulate_smooth_scroll(0.0, 500.0),
        [(5, 46), (14, 55), (20, 61), (23, 64), (24, 65), (25, 65)]
    );
}

#[test]
fn scroll_75_lines_40_fit() {
    assert_eq!(
        simulate_smooth_scroll(0.0, 1500.0),
        [
            (15, 56),
            (44, 85),
            (62, 103),
            (71, 112),
            (74, 115),
            (75, 115)
        ]
    );
}

#[test]
fn scroll_100_lines_40_fit() {
    assert_eq!(
        simulate_smooth_scroll(0.0, 2000.0),
        [
            (20, 61),
            (59, 100),
            (82, 123),
            (94, 135),
            (99, 140),
            (100, 140)
        ]
    );
}

#[test]
fn scroll_125_lines_40_fit() {
    assert_eq!(
        simulate_smooth_scroll(0.0, 2500.0),
        [
            (16, 57),
            (29, 70),
            (107, 148),
            (119, 160),
            (124, 165),
            (125, 165)
        ]
    );
}

#[test]
fn scroll_500_lines_40_fit() {
    assert_eq!(
        simulate_smooth_scroll(0.0, 10000.0),
        [
            (16, 57),
            (29, 70),
            (482, 523),
            (494, 535),
            (499, 540),
            (500, 540)
        ]
    );
}

#[test]
fn long_jump_reaches_the_first_stop_at_the_cut_then_starts_from_the_second() {
    let before_cut = animate(0.0, 10000.0, VIEWPORT_HEIGHT, 0.33 - 1e-4);
    assert!((before_cut - 600.0).abs() < 0.1, "{before_cut}");
    assert_eq!(animate(0.0, 10000.0, VIEWPORT_HEIGHT, 0.33), 9400.0);
    assert_eq!(animate(10000.0, 0.0, VIEWPORT_HEIGHT, 0.33), 600.0);
}

#[test]
fn short_jump_eases_straight_to_the_target() {
    assert_eq!(animate(0.0, 2000.0, VIEWPORT_HEIGHT, 0.5), 1750.0);
}

#[test]
fn tick_returns_the_exact_target_and_done_once_the_duration_passes() {
    let now = Instant::now();
    let scroll = SmoothScroll::start((0., 0.), (1234.5, 67.25), (VIEWPORT_HEIGHT, 300.), now);
    assert!(!scroll.tick(now + Duration::from_millis(124)).2);
    assert_eq!(
        scroll.tick(now + Duration::from_millis(125)),
        (1234.5, 67.25, true)
    );
    assert_eq!(
        scroll.tick(now + Duration::from_secs(5)),
        (1234.5, 67.25, true)
    );
}
