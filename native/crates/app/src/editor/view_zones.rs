//! View zones: whitespace the view inserts below a model line, as viewZones.ts and linesLayout.ts lay them out for
//! one line height. A zone widget (zoneWidget.ts) draws over the whitespace of its zone.

use super::{OpenEditor, Position, Viewport, end_of, folding::tracked};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewZone {
    pub id: u32,
    /// The model position the zone follows (`afterLineNumber`, `afterColumn`): the zone sits below its line.
    pub anchor: Position,
    pub height: f32,
    /// The zone sits above its anchor's line instead (`afterLineNumber` of the line before, 0 above the first).
    pub before: bool,
}

impl OpenEditor {
    pub fn add_view_zone(&mut self, anchor: Position, height: f32) -> u32 {
        let id = self.view_zones.iter().map(|zone| zone.id + 1).max().unwrap_or(1);
        self.view_zones.push(ViewZone { id, anchor, height, before: false });
        id
    }

    /// A zone above model line `line`, which follows the line's start.
    pub fn add_view_zone_before(&mut self, line: usize, height: f32) -> u32 {
        let id = self.add_view_zone(Position::new(line, 0), height);
        self.view_zones.last_mut().expect("just added").before = true;
        id
    }

    pub fn remove_view_zone(&mut self, id: u32) {
        self.view_zones.retain(|zone| zone.id != id);
    }

    pub fn view_zone(&self, id: u32) -> Option<&ViewZone> {
        self.view_zones.iter().find(|zone| zone.id == id)
    }

    /// viewZones.ts `_computeWhitespaceProps`: the view lines above the zone; a zone in a folded range is not shown
    /// (`showInHiddenAreas` false), `None`.
    fn zone_view_position(&self, zone: &ViewZone) -> Option<usize> {
        let view_line = self.view_line(zone.anchor.line);
        (self.model_line(view_line) == zone.anchor.line).then_some(view_line + usize::from(!zone.before))
    }

    /// The shown zones as (view lines above, height), in the order linesLayout.ts sorts them.
    fn shown_zones(&self) -> Vec<(usize, f32)> {
        let mut zones: Vec<(usize, f32)> = self.view_zones.iter().filter_map(|zone| Some((self.zone_view_position(zone)?, zone.height))).collect();
        zones.sort_by_key(|zone| zone.0);
        zones
    }

    /// `getWhitespaceAccumulatedHeightBeforeLineNumber` for a 0-based view line.
    fn zones_height_before(&self, view_line: usize) -> f32 {
        self.shown_zones().iter().filter(|zone| zone.0 <= view_line).map(|zone| zone.1).sum()
    }

    /// `getVerticalOffsetForLineNumber`: the whitespace above, the lines before and the zones before.
    pub fn line_top(&self, view_line: usize, viewport: Viewport) -> f32 {
        viewport.line_top(view_line) + self.zones_height_before(view_line)
    }

    /// The top of a zone's whitespace (`getVerticalOffsetForWhitespaceIndex`), `None` while it is folded away.
    pub fn view_zone_top(&self, id: u32, viewport: Viewport) -> Option<f32> {
        let zone = self.view_zone(id)?;
        let position = self.zone_view_position(zone)?;
        // The zones at the same position lie in their order above the line: this one and the ones after it.
        let below: f32 = self.view_zones.iter().skip_while(|other| other.id != id).filter(|other| self.zone_view_position(other) == Some(position)).map(|other| other.height).sum();
        Some(self.line_top(position, viewport) - below)
    }

    /// `getLineNumberAtOrAfterVerticalOffset` as a 0-based view line, not clamped to the line count: an offset in
    /// a zone is the line after it.
    pub fn view_line_at(&self, offset: f32, viewport: Viewport) -> usize {
        let offset = (offset - viewport.top_whitespace).max(0.);
        let mut above = 0.;
        for (position, height) in self.shown_zones() {
            let zone_top = position as f32 * viewport.line_height + above;
            if offset < zone_top {
                break;
            }
            if offset < zone_top + height {
                return position;
            }
            above += height;
        }
        ((offset - above) / viewport.line_height).floor() as usize
    }

    /// The zones and where they go between the lines `lines`: the zones under each view line before the last.
    pub fn zones_between(&self, lines: std::ops::Range<usize>) -> Vec<(usize, f32)> {
        self.shown_zones().into_iter().filter(|zone| zone.0 > lines.start && zone.0 < lines.end).collect()
    }

    /// A zone follows its anchor like a decoration with `NeverGrowsWhenTypingAtEdges`.
    pub(super) fn track_view_zones(&mut self, start: Position, end: Position, text: &str) {
        let new_end = end_of(start, text);
        for zone in &mut self.view_zones {
            zone.anchor = tracked(zone.anchor, start, end, new_end, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::LINE_HEIGHT;
    use crate::editor::edit::tests::editor;

    const VIEWPORT: Viewport = Viewport { width: 400., height: 360., char_width: 7.2, line_height: LINE_HEIGHT, sticky_scroll_lines: 0, top_whitespace: 0., stop_rendering_line_after: None };

    #[test]
    fn a_zone_pushes_the_lines_below_it_down() {
        let mut editor = editor("a\nb\nc\nd", "\n");
        let id = editor.add_view_zone(Position::new(1, 0), 50.);
        assert_eq!(editor.line_top(1, VIEWPORT), LINE_HEIGHT);
        assert_eq!(editor.line_top(2, VIEWPORT), 2. * LINE_HEIGHT + 50.);
        assert_eq!(editor.view_zone_top(id, VIEWPORT), Some(2. * LINE_HEIGHT));
        assert_eq!(editor.view_line_at(2. * LINE_HEIGHT + 10., VIEWPORT), 2);
        assert_eq!(editor.view_line_at(2. * LINE_HEIGHT + 50. + LINE_HEIGHT + 1., VIEWPORT), 3);
        assert_eq!(editor.view_line_at(LINE_HEIGHT + 1., VIEWPORT), 1);
        assert_eq!(editor.zones_between(0..4), vec![(2, 50.)]);
        assert_eq!(editor.zones_between(2..4), vec![]);
    }

    #[test]
    fn a_zone_before_the_first_line_pushes_every_line_down() {
        let mut editor = editor("a\nb", "\n");
        let id = editor.add_view_zone_before(0, 20.);
        assert_eq!(editor.view_zone_top(id, VIEWPORT), Some(0.));
        assert_eq!(editor.line_top(0, VIEWPORT), 20.);
        let below = editor.add_view_zone_before(1, 10.);
        assert_eq!(editor.view_zone_top(below, VIEWPORT), Some(20. + LINE_HEIGHT));
        assert_eq!(editor.line_top(1, VIEWPORT), 30. + LINE_HEIGHT);
    }

    #[test]
    fn a_zone_follows_its_line_through_edits() {
        let mut editor = editor("a\nb\nc", "\n");
        let id = editor.add_view_zone(Position::new(1, 0), 50.);
        editor.track_view_zones(Position::new(0, 0), Position::new(0, 0), "x\ny\n");
        assert_eq!(editor.view_zone(id).unwrap().anchor, Position::new(3, 0));
        editor.remove_view_zone(id);
        assert!(editor.view_zone(id).is_none());
    }
}
