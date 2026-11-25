//! base/browser/ui/splitview/splitview.ts: views side by side with sashes between them. Views may snap (hide when
//! a sash drags them below half their minimum) and take a layout priority; a split view lays out by proportions
//! (`proportionalLayout`, the default) or gives size changes to its views by priority, as the workbench grid does.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Priority {
    Low,
    #[default]
    Normal,
    High,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub size: f32,
    pub minimum: f32,
    pub maximum: f32,
    pub snap: bool,
    pub priority: Priority,
    pub visible: bool,
    /// The size to come back to once a hidden view shows (`_cachedVisibleSize`).
    cached_visible_size: Option<f32>,
}

impl View {
    pub fn new(size: f32, minimum: f32, maximum: f32) -> Self {
        View { size, minimum, maximum, snap: false, priority: Priority::Normal, visible: true, cached_visible_size: None }
    }

    pub fn snap(mut self) -> Self {
        self.snap = true;
        self
    }

    pub fn priority(mut self, priority: Priority) -> Self {
        self.priority = priority;
        self
    }

    /// A view added with `visible: false` in its descriptor: no room until it shows at `size`.
    pub fn visible(mut self, visible: bool) -> Self {
        self.set_visible(visible, None);
        self
    }

    /// `ViewItem.minimumSize`: a hidden view takes no room.
    fn min(&self) -> f32 {
        if self.visible { self.minimum } else { 0. }
    }

    fn max(&self) -> f32 {
        if self.visible { self.maximum } else { 0. }
    }

    /// `ViewItem.setVisible`.
    fn set_visible(&mut self, visible: bool, size: Option<f32>) {
        if visible == self.visible {
            return;
        }
        self.visible = visible;
        if visible {
            self.size = self.cached_visible_size.take().unwrap_or(self.size).clamp(self.minimum, self.maximum);
        } else {
            self.cached_visible_size = Some(size.unwrap_or(self.size));
            self.size = 0.;
        }
    }
}

/// `SashState`: which ways a sash can move, read for its cursor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SashState {
    Disabled,
    AtMinimum,
    AtMaximum,
    Enabled,
}

#[derive(Clone, Copy, Debug)]
struct SnapState {
    index: usize,
    limit_delta: f32,
    size: f32,
}

struct SashDrag {
    index: usize,
    start: f32,
    current: f32,
    sizes: Vec<f32>,
    min_delta: f32,
    max_delta: f32,
    alt: bool,
    snap_before: Option<SnapState>,
    snap_after: Option<SnapState>,
}

pub struct SplitView {
    size: f32,
    content_size: f32,
    views: Vec<View>,
    /// `None` without `proportionalLayout`.
    proportions: Option<Vec<Option<f32>>>,
    proportional: bool,
    drag: Option<SashDrag>,
}

impl SplitView {
    /// `options.descriptor`: the views at their sizes out of `size`, laid out by those proportions.
    pub fn new(size: f32, views: Vec<View>) -> Self {
        let mut split_view = SplitView { size, content_size: views.iter().map(|view| view.size).sum(), views, proportions: None, proportional: true, drag: None };
        split_view.save_proportions();
        split_view
    }

    /// `proportionalLayout: false`: a new size goes to the views by priority, high first and low last.
    pub fn with_priorities(size: f32, views: Vec<View>) -> Self {
        SplitView { size, content_size: views.iter().map(|view| view.size).sum(), views, proportions: None, proportional: false, drag: None }
    }

    pub fn sizes(&self) -> Vec<f32> {
        self.views.iter().map(|view| view.size).collect()
    }

    pub fn size(&self) -> f32 {
        self.size
    }

    /// The size a view had before it hid, or its size when it shows.
    pub fn visible_size(&self, index: usize) -> f32 {
        let view = &self.views[index];
        if view.visible { view.size } else { view.cached_visible_size.unwrap_or(view.size) }
    }

    /// A view's bounds changed, as a branch's do when its children show or hide.
    pub fn set_limits(&mut self, index: usize, minimum: f32, maximum: f32) {
        let view = &mut self.views[index];
        view.minimum = minimum;
        view.maximum = maximum;
    }

    pub fn layout(&mut self, size: f32) {
        let previous = self.size.max(self.content_size);
        self.size = size;
        match self.proportions.clone() {
            None => {
                let (low, high) = self.priority_indexes();
                let sizes = self.sizes();
                self.resize(self.views.len() - 1, size - previous, &sizes, &low, &high, f32::NEG_INFINITY, f32::INFINITY, None, None);
            }
            Some(proportions) => {
                let mut size = size;
                let mut total = 0.;
                for (view, proportion) in self.views.iter_mut().zip(&proportions) {
                    match proportion {
                        Some(proportion) => total += proportion,
                        None => {
                            view.size = view.size.clamp(view.min(), view.max());
                            size -= view.size;
                        }
                    }
                }
                for (view, proportion) in self.views.iter_mut().zip(&proportions) {
                    if let Some(proportion) = proportion.filter(|_| total > 0.) {
                        view.size = (proportion * size / total).round().clamp(view.min(), view.max());
                    }
                }
            }
        }
        self.distribute_empty_space(None);
        self.update_content_size();
    }

    /// `resizeView`: the view takes `size` within its bounds and the others absorb the change, it last.
    pub fn resize_view(&mut self, index: usize, size: f32) {
        let Some(view) = self.views.get_mut(index) else { return };
        view.size = size.round().clamp(view.min(), view.max().min(self.size));
        let others = (0..self.views.len()).filter(|&other| other != index);
        let mut low: Vec<usize> = others.clone().filter(|&other| self.views[other].priority == Priority::Low).collect();
        low.push(index);
        let high: Vec<usize> = others.filter(|&other| self.views[other].priority == Priority::High).collect();
        self.relayout(&low, &high);
    }

    /// `setViewVisible`: the shown or hidden view's room comes from or goes to the others, it last.
    pub fn set_view_visible(&mut self, index: usize, visible: bool) {
        self.views[index].set_visible(visible, None);
        self.distribute_empty_space(Some(index));
        self.update_content_size();
        self.save_proportions();
    }

    /// `moveView`: the view keeps its size at its new index.
    pub fn move_view(&mut self, from: usize, to: usize) {
        let view = self.views.remove(from);
        self.views.insert(to, view);
        self.save_proportions();
    }

    pub fn is_view_visible(&self, index: usize) -> bool {
        self.views[index].visible
    }

    /// `updateSashEnablement`; sash `index` sits after view `index`.
    pub fn sash_states(&self) -> Vec<SashState> {
        let running = |flags: Vec<bool>| flags.into_iter().scan(false, |any, flag| { *any |= flag; Some(*any) }).collect::<Vec<_>>();
        let reversed = |flags: Vec<bool>| { let mut flags = running(flags.into_iter().rev().collect()); flags.reverse(); flags };
        let collapses: Vec<bool> = self.views.iter().map(|view| view.size - view.min() > 0.).collect();
        let expands: Vec<bool> = self.views.iter().map(|view| view.max() - view.size > 0.).collect();
        let (collapses_down, expands_down) = (running(collapses.clone()), running(expands.clone()));
        let (collapses_up, expands_up) = (reversed(collapses), reversed(expands));
        let mut position = 0.;
        (0..self.views.len().saturating_sub(1))
            .map(|index| {
                position += self.views[index].size;
                let min = !(collapses_down[index] && expands_up[index + 1]);
                let max = !(expands_down[index] && collapses_up[index + 1]);
                match (min, max) {
                    (true, true) => {
                        let snapped = |snap: Option<usize>| snap.is_some_and(|snap| !self.views[snap].visible);
                        if snapped(self.first_snap_index((0..=index).rev())) && collapses_up[index] && position > 0. {
                            SashState::AtMinimum
                        } else if snapped(self.first_snap_index(index + 1..self.views.len())) && collapses_down[index] && position < self.content_size {
                            SashState::AtMaximum
                        } else {
                            SashState::Disabled
                        }
                    }
                    (true, false) => SashState::AtMinimum,
                    (false, true) => SashState::AtMaximum,
                    (false, false) => SashState::Enabled,
                }
            })
            .collect()
    }

    /// `onSashStart`: with Alt the view past the sash grows or shrinks on both sides, macOS style; without it a
    /// snapping view nearest the sash hides past half its minimum and shows again past it.
    pub fn start_sash_drag(&mut self, index: usize, start: f32, alt: bool) {
        let sizes = self.sizes();
        let (mut min_delta, mut max_delta) = (f32::NEG_INFINITY, f32::INFINITY);
        let (mut snap_before, mut snap_after) = (None, None);
        if alt {
            let is_last_sash = index == self.views.len() - 2;
            if is_last_sash {
                let view = self.views[index];
                min_delta = (view.min() - view.size) / 2.;
                max_delta = (view.max() - view.size) / 2.;
            } else {
                let view = self.views[index + 1];
                min_delta = (view.size - view.max()) / 2.;
                max_delta = (view.size - view.min()) / 2.;
            }
        } else {
            let up: Vec<usize> = (0..=index).rev().collect();
            let down: Vec<usize> = (index + 1..self.views.len()).collect();
            let views = &self.views;
            let min_delta_up: f32 = up.iter().map(|&i| views[i].min() - sizes[i]).sum();
            let max_delta_up: f32 = up.iter().map(|&i| views[i].maximum - sizes[i]).sum();
            let max_delta_down = if down.is_empty() { f32::INFINITY } else { down.iter().map(|&i| sizes[i] - views[i].min()).sum() };
            let min_delta_down = if down.is_empty() { f32::NEG_INFINITY } else { down.iter().map(|&i| sizes[i] - views[i].maximum).sum() };
            let limit_min = min_delta_up.max(min_delta_down);
            let limit_max = max_delta_down.min(max_delta_up);
            snap_before = self.first_snap_index(up.iter().copied()).map(|snap| {
                let view = &views[snap];
                let half = (view.minimum / 2.).floor();
                SnapState { index: snap, limit_delta: if view.visible { limit_min - half } else { limit_min + half }, size: view.size }
            });
            snap_after = self.first_snap_index(down.iter().copied()).map(|snap| {
                let view = &views[snap];
                let half = (view.minimum / 2.).floor();
                SnapState { index: snap, limit_delta: if view.visible { limit_max + half } else { limit_max - half }, size: view.size }
            });
        }
        self.drag = Some(SashDrag { index, start, current: start, sizes, min_delta, max_delta, alt, snap_before, snap_after });
    }

    /// `onSashChange`; pressing or releasing Alt mid-drag starts the drag over from where the sash is.
    pub fn drag_sash(&mut self, current: f32, alt: bool) {
        let Some(drag) = &self.drag else { return };
        if drag.alt != alt {
            let (index, from) = (drag.index, drag.current);
            self.start_sash_drag(index, from, alt);
        }
        let drag = self.drag.as_mut().expect("a sash drag runs");
        drag.current = current;
        let (index, start, min_delta, max_delta, alt, snap_before, snap_after) = (drag.index, drag.start, drag.min_delta, drag.max_delta, drag.alt, drag.snap_before, drag.snap_after);
        let sizes = drag.sizes.clone();
        let delta = self.resize(index, current - start, &sizes, &[], &[], min_delta, max_delta, snap_before, snap_after);
        if alt {
            let is_last_sash = index == self.views.len() - 2;
            let view = self.views[if is_last_sash { index } else { index + 1 }];
            let resize_index = if is_last_sash { index - 1 } else { index + 1 };
            let sizes = self.sizes();
            self.resize(resize_index, -delta, &sizes, &[], &[], view.size - view.max(), view.size - view.min(), None, None);
        }
        self.distribute_empty_space(None);
        self.update_content_size();
    }

    pub fn end_sash_drag(&mut self) {
        self.drag = None;
        self.save_proportions();
    }

    pub fn dragged_sash(&self) -> Option<usize> {
        self.drag.as_ref().map(|drag| drag.index)
    }

    fn priority_indexes(&self) -> (Vec<usize>, Vec<usize>) {
        let of = |priority| (0..self.views.len()).filter(|&i| self.views[i].priority == priority).collect();
        (of(Priority::Low), of(Priority::High))
    }

    /// `findFirstSnapIndex`: the first visible snapping view, else the first hidden one before any resizable view.
    fn first_snap_index(&self, indexes: impl Iterator<Item = usize> + Clone) -> Option<usize> {
        if let Some(index) = indexes.clone().find(|&i| self.views[i].visible && self.views[i].snap) {
            return Some(index);
        }
        for index in indexes {
            let view = &self.views[index];
            if view.visible && view.max() - view.min() > 0. {
                return None;
            }
            if !view.visible && view.snap {
                return Some(index);
            }
        }
        None
    }

    fn save_proportions(&mut self) {
        if self.proportional && self.content_size > 0. {
            self.proportions = Some(self.views.iter().map(|view| view.visible.then(|| view.size / self.content_size)).collect());
        }
    }

    fn relayout(&mut self, low: &[usize], high: &[usize]) {
        let content_size: f32 = self.views.iter().map(|view| view.size).sum();
        let sizes = self.sizes();
        self.resize(self.views.len() - 1, self.size - content_size, &sizes, low, high, f32::NEG_INFINITY, f32::INFINITY, None, None);
        self.distribute_empty_space(None);
        self.update_content_size();
        self.save_proportions();
    }

    /// Moves the boundary after view `index` by `delta` from `sizes`: the views before it take the delta, the
    /// nearest first, the views after it give it, each within its bounds. Returns the delta applied.
    #[allow(clippy::too_many_arguments)]
    fn resize(&mut self, index: usize, delta: f32, sizes: &[f32], low: &[usize], high: &[usize], min_delta: f32, max_delta: f32, snap_before: Option<SnapState>, snap_after: Option<SnapState>) -> f32 {
        let mut up: Vec<usize> = (0..=index).rev().collect();
        let mut down: Vec<usize> = (index + 1..self.views.len()).collect();
        for indexes in [&mut up, &mut down] {
            for &i in high {
                if let Some(position) = indexes.iter().position(|&index| index == i) {
                    indexes.remove(position);
                    indexes.insert(0, i);
                }
            }
            for &i in low {
                if let Some(position) = indexes.iter().position(|&index| index == i) {
                    indexes.remove(position);
                    indexes.push(i);
                }
            }
        }
        let mut snapped = false;
        if let Some(snap) = snap_before {
            let visible = delta >= snap.limit_delta;
            snapped = visible != self.views[snap.index].visible;
            self.views[snap.index].set_visible(visible, Some(snap.size));
        }
        if let Some(snap) = snap_after.filter(|_| !snapped) {
            let visible = delta < snap.limit_delta;
            snapped = visible != self.views[snap.index].visible;
            self.views[snap.index].set_visible(visible, Some(snap.size));
        }
        if snapped {
            return self.resize(index, delta, sizes, low, high, min_delta, max_delta, snap_before, snap_after);
        }
        let views = &mut self.views;
        let min_delta_up: f32 = up.iter().map(|&i| views[i].min() - sizes[i]).sum();
        let max_delta_up: f32 = up.iter().map(|&i| views[i].max() - sizes[i]).sum();
        let max_delta_down = if down.is_empty() { f32::INFINITY } else { down.iter().map(|&i| sizes[i] - views[i].min()).sum() };
        let min_delta_down = if down.is_empty() { f32::NEG_INFINITY } else { down.iter().map(|&i| sizes[i] - views[i].max()).sum() };
        let delta = delta.max(min_delta_up.max(min_delta_down).max(min_delta)).min(max_delta_down.min(max_delta_up).min(max_delta));
        let mut delta_up = delta;
        for &i in &up {
            let size = (sizes[i] + delta_up).clamp(views[i].min(), views[i].max());
            delta_up -= size - sizes[i];
            views[i].size = size;
        }
        let mut delta_down = delta;
        for &i in &down {
            let size = (sizes[i] - delta_down).clamp(views[i].min(), views[i].max());
            delta_down += size - sizes[i];
            views[i].size = size;
        }
        delta
    }

    /// `distributeEmptySpace`: what the sizes miss of the whole goes to the views from the last, high priority
    /// first, low priority and then `last` at the end.
    fn distribute_empty_space(&mut self, last: Option<usize>) {
        let mut empty = self.size - self.views.iter().map(|view| view.size).sum::<f32>();
        let mut indexes: Vec<usize> = (0..self.views.len()).rev().collect();
        let (low, high) = self.priority_indexes();
        for i in high.into_iter().rev() {
            indexes.retain(|&index| index != i);
            indexes.insert(0, i);
        }
        for i in low.into_iter().rev().chain(last) {
            indexes.retain(|&index| index != i);
            indexes.push(i);
        }
        for i in indexes {
            if empty == 0. {
                break;
            }
            let view = &mut self.views[i];
            let size = (view.size + empty).clamp(view.min(), view.max());
            empty -= size - view.size;
            view.size = size;
        }
    }

    /// `layoutViews` keeps the content size the proportions are saved from.
    fn update_content_size(&mut self) {
        self.content_size = self.views.iter().map(|view| view.size).sum();
    }
}

#[cfg(test)]
mod tests;
