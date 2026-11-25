//! Layout of the workbench trees as abstractTree.ts computes it over the shown rows, one row height for all:
//! indent guides (`TreeRenderer._renderIndentGuides`, `_onDidChangeActiveNodes`), sticky scroll
//! (`StickyScrollController.findStickyState`), and the wheel and smooth scrolling of scrollableElement.ts and
//! scrollable.ts that the lists share. Rows are given by their depth, 1 for the top rows.

#[cfg(test)]
mod tests;

/// The row's parent: the closest row above it with a smaller depth. Top rows have none.
pub fn parent(depths: &[usize], index: usize) -> Option<usize> {
    depths[..index].iter().rposition(|depth| *depth < depths[index])
}

/// The last row under `index`, `index` itself without shown children (`getListRenderCount` - 1 past it).
pub fn last_descendant(depths: &[usize], index: usize) -> usize {
    let depth = depths[index];
    index + depths[index + 1..].iter().take_while(|child| **child > depth).count()
}

/// `_onDidChangeActiveNodes`: the rows whose guides draw active. An active row (focused or selected) that shows
/// children makes its own guides active, any other its parent's.
pub fn active_guide_parents(depths: &[usize], active: &[usize]) -> Vec<usize> {
    let mut parents = Vec::new();
    for &index in active {
        let shows_children = last_descendant(depths, index) > index;
        let owner = if shows_children { Some(index) } else { parent(depths, index) };
        if let Some(owner) = owner.filter(|owner| !parents.contains(owner)) {
            parents.push(owner);
        }
    }
    parents
}

/// `_renderIndentGuides`: one guide per ancestor of the row, the outermost first, each active when its ancestor is
/// one of `active_parents`.
pub fn indent_guides(depths: &[usize], index: usize, active_parents: &[usize]) -> Vec<bool> {
    let mut guides = Vec::new();
    let mut row = index;
    while let Some(ancestor) = parent(depths, row) {
        guides.push(active_parents.contains(&ancestor));
        row = ancestor;
    }
    guides.reverse();
    guides
}

/// A row drawn in the sticky scroll widget: its index and its top inside the widget.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StickyRow {
    pub index: usize,
    pub position: f32,
}

/// abstractTree.ts `StickyScrollController.maxWidgetViewRatio`.
const MAX_WIDGET_VIEW_RATIO: f32 = 0.4;

/// `StickyScrollController.update` and `findStickyState` with the default delegate and source ranges: the
/// ancestors of the first rows under the widget that scrolled out of view, each pushed up by the end of its
/// subtree, at most `max_count` (`workbench.tree.stickyScrollMaxItemCount`, at least 1) and 40% of the view.
pub fn sticky_rows(depths: &[usize], row_height: f32, scroll_top: f32, render_height: f32, max_count: usize) -> Vec<StickyRow> {
    let max_count = max_count.max(1);
    let index_at = |height: f32| -> Option<usize> {
        let index = ((scroll_top + height) / row_height).floor();
        (index >= 0. && (index as usize) < depths.len()).then_some(index as usize)
    };
    let Some(first_visible) = index_at(0.) else { return Vec::new() };
    if scroll_top <= 0. || render_height == 0. {
        return Vec::new();
    }
    let mut sticky: Vec<StickyRow> = Vec::new();
    let mut under_widget = first_visible;
    let mut sticky_height = 0.;
    while let Some(next) = next_sticky_row(depths, under_widget, sticky.last().map(|row| row.index), sticky_height, row_height, scroll_top) {
        sticky.push(next);
        sticky_height += row_height;
        if sticky.len() <= max_count {
            match index_at(next.position + row_height) {
                Some(index) => under_widget = index,
                None => break,
            }
        }
    }
    // `constrainStickyNodes` with DefaultStickyScrollDelegate.
    let max_height = render_height * MAX_WIDGET_VIEW_RATIO;
    let fits = sticky.iter().enumerate().position(|(index, row)| row.position + row_height > max_height || index >= max_count);
    sticky.truncate(fits.unwrap_or(sticky.len()));
    sticky
}

/// `getNextStickyNode`: the ancestor of the first row under the widget that sits right under the previous sticky
/// row, if it has scrolled under the widget.
fn next_sticky_row(depths: &[usize], under_widget: usize, previous: Option<usize>, sticky_height: f32, row_height: f32, scroll_top: f32) -> Option<StickyRow> {
    let next = ancestor_under(depths, under_widget, previous)?;
    let last = last_descendant(depths, next);
    // `nodeIsUncollapsedParent`: the row itself sticks only while it shows children.
    if next == under_widget && last == next {
        return None;
    }
    if scroll_top + sticky_height <= next as f32 * row_height {
        return None;
    }
    // `calculateStickyNodePosition`: pushed up once the bottom of its last row comes into the widget.
    let bottom_of_last = (last + 1) as f32 * row_height - scroll_top;
    let position = match sticky_height + row_height > bottom_of_last && sticky_height <= bottom_of_last {
        true => bottom_of_last - row_height,
        false => sticky_height,
    };
    Some(StickyRow { index: next, position })
}

/// `getAncestorUnderPrevious`: the ancestor of `row` (or `row`) whose parent is `previous`; the top ancestor
/// without a previous sticky row.
fn ancestor_under(depths: &[usize], row: usize, previous: Option<usize>) -> Option<usize> {
    let mut current = row;
    while let Some(parent) = parent(depths, current) {
        if Some(parent) == previous {
            return Some(current);
        }
        current = parent;
    }
    previous.is_none().then_some(current)
}

/// `nodePositionTopBelowWidget`: the height the sticky rows of the row's ancestors take, for a sticky row click
/// that scrolls its row right under them.
pub fn position_below_widget(depths: &[usize], index: usize, row_height: f32, max_count: usize) -> f32 {
    let mut ancestors = 0;
    let mut row = index;
    while let Some(ancestor) = parent(depths, row) {
        ancestors += 1;
        row = ancestor;
    }
    ancestors.min(max_count.max(1)) as f32 * row_height
}

/// scrollableElement.ts `SCROLL_WHEEL_SENSITIVITY`.
const SCROLL_WHEEL_SENSITIVITY: f32 = 50.;

/// `_onMouseWheel` with `scrollPredominantAxis`: the scroll position change of one `StandardWheelEvent` (deltas
/// positive towards the start), `workbench.list.mouseWheelScrollSensitivity` times, and
/// `workbench.list.fastScrollSensitivity` times more with Alt. Returns (left, top) deltas in pixels.
pub fn wheel_scroll_delta(delta_x: f32, delta_y: f32, alt: bool, sensitivity: f32, fast_sensitivity: f32) -> (f32, f32) {
    let (mut delta_x, mut delta_y) = (delta_x * sensitivity, delta_y * sensitivity);
    if delta_y.abs() >= delta_x.abs() {
        delta_x = 0.;
    } else {
        delta_y = 0.;
    }
    if alt {
        delta_x *= fast_sensitivity;
        delta_y *= fast_sensitivity;
    }
    // -0.3 becomes -1 and 0.3 becomes 1, or slow scrolling would never move.
    let step = |delta: f32| {
        let pixels = SCROLL_WHEEL_SENSITIVITY * delta;
        if pixels < 0. { pixels.floor() } else { pixels.ceil() }
    };
    (-step(delta_x), -step(delta_y))
}

/// `getDesiredScrollPositionFromOffsetPaged` (`workbench.list.scrollByPage`): a press on the track before the
/// slider goes one view back, after it one view on.
pub fn paged_scroll_position(offset: f32, slider_position: f32, scroll_position: f32, visible: f32) -> f32 {
    match offset < slider_position {
        true => scroll_position - visible,
        false => scroll_position + visible,
    }
}

/// listView.ts `smoothScrollDuration` with `workbench.list.smoothScrolling`, in ms.
pub const SMOOTH_SCROLL_DURATION: f32 = 125.;

/// scrollable.ts `SmoothScrollingOperation` on one axis: ease-out cubic from `from` to `to`; a jump of more
/// than 2.5 views runs the first and last 75% of a view and skips the middle, a third of the time in the first.
pub fn smooth_scroll_position(from: f32, to: f32, viewport: f32, completion: f32) -> f32 {
    if completion >= 1. {
        return to;
    }
    let ease_out = |from: f32, to: f32, completion: f32| from + (to - from) * (1. - (1. - completion).powi(3));
    if (from - to).abs() <= 2.5 * viewport {
        return ease_out(from, to, completion);
    }
    let (stop1, stop2) = match from < to {
        true => (from + 0.75 * viewport, to - 0.75 * viewport),
        false => (from - 0.75 * viewport, to + 0.75 * viewport),
    };
    const CUT: f32 = 0.33;
    match completion < CUT {
        true => ease_out(from, stop1, completion / CUT),
        false => ease_out(stop2, to, (completion - CUT) / (1. - CUT)),
    }
}
