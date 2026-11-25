//! base/browser/ui/splitview/splitview.ts for views side by side that never snap, hide or take layout
//! priorities, as the Keyboard Shortcuts table's column headers are: proportional layout, `resizeView` and the
//! sash drags with their Alt variant.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub size: f32,
    pub minimum: f32,
    pub maximum: f32,
}

/// `SashState`: which ways a sash can move, read for its cursor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SashState {
    Disabled,
    AtMinimum,
    AtMaximum,
    Enabled,
}

struct SashDrag {
    index: usize,
    start: f32,
    current: f32,
    sizes: Vec<f32>,
    min_delta: f32,
    max_delta: f32,
    alt: bool,
}

pub struct SplitView {
    size: f32,
    content_size: f32,
    views: Vec<View>,
    proportions: Vec<f32>,
    drag: Option<SashDrag>,
}

impl SplitView {
    /// `options.descriptor`: the views at their sizes out of `size`, laid out by those proportions.
    pub fn new(size: f32, views: Vec<View>) -> Self {
        let mut split_view = SplitView { size, content_size: views.iter().map(|view| view.size).sum(), views, proportions: Vec::new(), drag: None };
        split_view.save_proportions();
        split_view
    }

    pub fn sizes(&self) -> Vec<f32> {
        self.views.iter().map(|view| view.size).collect()
    }

    pub fn size(&self) -> f32 {
        self.size
    }

    pub fn layout(&mut self, size: f32) {
        self.size = size;
        let total: f32 = self.proportions.iter().sum();
        if total > 0. {
            for (view, proportion) in self.views.iter_mut().zip(&self.proportions) {
                view.size = (proportion * size / total).round().clamp(view.minimum, view.maximum);
            }
        }
        self.distribute_empty_space();
        self.update_content_size();
    }

    /// `resizeView`: the view takes `size` within its bounds and the others absorb the change, it last.
    pub fn resize_view(&mut self, index: usize, size: f32) {
        let Some(view) = self.views.get_mut(index) else { return };
        view.size = size.round().clamp(view.minimum, view.maximum.min(self.size));
        self.relayout(Some(index));
    }

    /// `updateSashEnablement`; sash `index` sits after view `index`.
    pub fn sash_states(&self) -> Vec<SashState> {
        let collapses: Vec<bool> = self.views.iter().map(|view| view.size - view.minimum > 0.).collect();
        let expands: Vec<bool> = self.views.iter().map(|view| view.maximum - view.size > 0.).collect();
        let down = |flags: &[bool]| flags.iter().scan(false, |any, &flag| { *any |= flag; Some(*any) }).collect::<Vec<_>>();
        let up = |flags: &[bool]| { let mut any: Vec<bool> = flags.iter().rev().scan(false, |any, &flag| { *any |= flag; Some(*any) }).collect(); any.reverse(); any };
        let (collapses_down, expands_down, collapses_up, expands_up) = (down(&collapses), down(&expands), up(&collapses), up(&expands));
        (0..self.views.len().saturating_sub(1))
            .map(|index| {
                let min = !(collapses_down[index] && expands_up[index + 1]);
                let max = !(expands_down[index] && collapses_up[index + 1]);
                match (min, max) {
                    (true, true) => SashState::Disabled,
                    (true, false) => SashState::AtMinimum,
                    (false, true) => SashState::AtMaximum,
                    (false, false) => SashState::Enabled,
                }
            })
            .collect()
    }

    /// `onSashStart`: with Alt the view past the sash grows or shrinks on both sides, macOS style.
    pub fn start_sash_drag(&mut self, index: usize, start: f32, alt: bool) {
        let sizes = self.sizes();
        let (mut min_delta, mut max_delta) = (f32::NEG_INFINITY, f32::INFINITY);
        if alt {
            let is_last_sash = index == self.views.len() - 2;
            if is_last_sash {
                let view = self.views[index];
                min_delta = (view.minimum - view.size) / 2.;
                max_delta = (view.maximum - view.size) / 2.;
            } else {
                let view = self.views[index + 1];
                min_delta = (view.size - view.maximum) / 2.;
                max_delta = (view.size - view.minimum) / 2.;
            }
        }
        self.drag = Some(SashDrag { index, start, current: start, sizes, min_delta, max_delta, alt });
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
        let (index, start, min_delta, max_delta, alt, sizes) = (drag.index, drag.start, drag.min_delta, drag.max_delta, drag.alt, drag.sizes.clone());
        let delta = self.resize(index, current - start, &sizes, None, min_delta, max_delta);
        if alt {
            let is_last_sash = index == self.views.len() - 2;
            let view = self.views[if is_last_sash { index } else { index + 1 }];
            let resize_index = if is_last_sash { index - 1 } else { index + 1 };
            self.resize(resize_index, -delta, &self.sizes(), None, view.size - view.maximum, view.size - view.minimum);
        }
        self.distribute_empty_space();
        self.update_content_size();
    }

    pub fn end_sash_drag(&mut self) {
        self.drag = None;
        self.save_proportions();
    }

    pub fn dragged_sash(&self) -> Option<usize> {
        self.drag.as_ref().map(|drag| drag.index)
    }

    fn save_proportions(&mut self) {
        if self.content_size > 0. {
            self.proportions = self.views.iter().map(|view| view.size / self.content_size).collect();
        }
    }

    fn relayout(&mut self, low_priority: Option<usize>) {
        let content_size: f32 = self.views.iter().map(|view| view.size).sum();
        let sizes = self.sizes();
        self.resize(self.views.len() - 1, self.size - content_size, &sizes, low_priority, f32::NEG_INFINITY, f32::INFINITY);
        self.distribute_empty_space();
        self.update_content_size();
        self.save_proportions();
    }

    /// Moves the boundary after view `index` by `delta` from `sizes`: the views before it take the delta, the
    /// nearest first, the views after it give it, each within its bounds. Returns the delta applied.
    fn resize(&mut self, index: usize, delta: f32, sizes: &[f32], low_priority: Option<usize>, min_delta: f32, max_delta: f32) -> f32 {
        let mut up: Vec<usize> = (0..=index).rev().collect();
        let mut down: Vec<usize> = (index + 1..self.views.len()).collect();
        if let Some(low) = low_priority {
            for indexes in [&mut up, &mut down] {
                if let Some(position) = indexes.iter().position(|&index| index == low) {
                    indexes.remove(position);
                    indexes.push(low);
                }
            }
        }
        let views = &mut self.views;
        let min_delta_up: f32 = up.iter().map(|&i| views[i].minimum - sizes[i]).sum();
        let max_delta_up: f32 = up.iter().map(|&i| views[i].maximum - sizes[i]).sum();
        let max_delta_down = if down.is_empty() { f32::INFINITY } else { down.iter().map(|&i| sizes[i] - views[i].minimum).sum() };
        let min_delta_down = if down.is_empty() { f32::NEG_INFINITY } else { down.iter().map(|&i| sizes[i] - views[i].maximum).sum() };
        let delta = delta.clamp(min_delta_up.max(min_delta_down).max(min_delta), max_delta_down.min(max_delta_up).min(max_delta));
        let mut delta_up = delta;
        for &i in &up {
            let size = (sizes[i] + delta_up).clamp(views[i].minimum, views[i].maximum);
            delta_up -= size - sizes[i];
            views[i].size = size;
        }
        let mut delta_down = delta;
        for &i in &down {
            let size = (sizes[i] - delta_down).clamp(views[i].minimum, views[i].maximum);
            delta_down += size - sizes[i];
            views[i].size = size;
        }
        delta
    }

    /// `distributeEmptySpace`: what the sizes miss of the whole goes to the last view that can take it.
    fn distribute_empty_space(&mut self) {
        let mut empty = self.size - self.views.iter().map(|view| view.size).sum::<f32>();
        for view in self.views.iter_mut().rev() {
            if empty == 0. {
                break;
            }
            let size = (view.size + empty).clamp(view.minimum, view.maximum);
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
