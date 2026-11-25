use gpui::TestAppContext;
use serde_json::json;

use super::*;
use crate::workbench::peek_view::PeekSash;
use crate::workbench::goto_symbol::tests::{answer, link, open, run};

/// zoneWidget.ts `_initSash`: the zone grows by whole lines while dragged and stays between 5 and 35 lines; the
/// split view sash keeps the preview at least 200px wide; both are stored as `peekViewLayout` on close.
#[gpui::test]
fn the_sashes_resize_the_zone_and_the_preview_and_the_layout_is_stored(cx: &mut TestAppContext) {
    let (workbench, cx, receiver, dir) = open(cx, "peek-sashes", "foo\nbar foo\nfoo\n");
    run(&workbench, cx, "editor.action.revealDefinition");
    answer(&receiver, "$provideDefinition", json!([link(&dir, 2, 5, 8), link(&dir, 3, 1, 4)]), cx);
    workbench.update(cx, |workbench, cx| {
        let line_height = workbench.editors.viewport.line_height;
        let widget = workbench.references_peek.widget.as_mut().unwrap();
        let start = widget.layout.height_in_lines;
        widget.layout.sash_drag = Some((PeekSash::Zone, 100., start, 800.));
        workbench.drag_peek_sash(PeekKind::References, 0., 100. + 2.5 * line_height, cx);
        let widget = workbench.references_peek.widget.as_ref().unwrap();
        assert_eq!(widget.layout.height_in_lines, start + 2.);
        let zone = workbench.editors.active().unwrap().view_zone(widget.layout.zone_id).unwrap();
        assert_eq!(zone.height, (start + 2.) * line_height);
        workbench.drag_peek_sash(PeekKind::References, 0., 100. + 40. * line_height, cx);
        assert_eq!(workbench.references_peek.widget.as_ref().unwrap().layout.height_in_lines, start + 2.);
        workbench.references_peek.widget.as_mut().unwrap().layout.sash_drag = Some((PeekSash::Split, 560., 560., 800.));
        workbench.drag_peek_sash(PeekKind::References, 0., 0., cx);
        assert_eq!(workbench.references_peek.widget.as_ref().unwrap().layout.ratio, 200. / 800.);
    });
    run(&workbench, cx, "closeReferenceSearch");
    workbench.read_with(cx, |workbench, _| {
        let layout: serde_json::Value = serde_json::from_str(&workbench.profile_storage.lock().unwrap().get(LAYOUT.key).unwrap()).unwrap();
        assert_eq!(layout["ratio"], json!(0.25));
    });
}
