//! The quick input widget: platform/quickinput/browser/quickInputController.ts with its list
//! (quickInputList.ts), laid out and colored as media/quickInput.css draws it. The store keeps every quick
//! input with its owner (main_thread_quick_open.rs); the widget shows one of them at a time.

use std::cell::Cell;
use std::ops::Range;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    Animation, AnimationExt, AnyElement, App, Bounds, BoxShadow, ClickEvent, DispatchPhase, Context, Div, FocusHandle, FontWeight, HighlightStyle, ListAlignment, ListState,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Rgba, StyledText, Subscription, Task, Window, canvas, div, fill, list, point, prelude::*, px, rgba,
};
use serde_json::Value;

use super::Workbench;
use super::input_box_view::{InputId, InputLook, InputView};
use crate::commands::Outcome;
use crate::main_thread_quick_open::Owner;
use crate::quick_input::{Button, Event, Focus, Kind, QuickInput, Row, Severity, drag_layout};

/// `QuickInputController.MAX_WIDTH`.
const MAX_WIDTH: f32 = 600.;
/// `quickPickTop` in layout.ts while the title bar shows the command center.
const TOP: f32 = 6.;
/// The title bar part's height, `activeContainerOffset.top` for the drag.
const TITLE_BAR_HEIGHT: f32 = 35.;
const ROW_HEIGHT: f32 = 22.;
const DETAIL_ROW_HEIGHT: f32 = 44.;
const RADIUS: f32 = 8.;
/// quickInputController.ts, in StorageScope.APPLICATION.
const VIEWSTATE_STORAGE_KEY: &str = "workbench.quickInput.viewState";

pub struct QuickInputs {
    inputs: Vec<(QuickInput, Owner)>,
    next_id: i64,
    visible: Option<i64>,
    pub focus: FocusHandle,
    /// The element focused before the widget showed, focused again when it hides (`previousFocusElement`).
    previous_focus: Option<FocusHandle>,
    focus_out: Option<Subscription>,
    list: ListState,
    /// The input and the row order the list was last reset to, and the row it last revealed.
    listed: (Option<i64>, Vec<usize>),
    revealed: Option<usize>,
    hovered: Option<usize>,
    /// The hidden input, drawn while the close animation runs.
    closing: Option<(QuickInput, Task<()>)>,
    /// Counts the times the widget opened; a new count restarts the open animation.
    opened: usize,
    /// A busy input shows its progress bar after 800ms.
    busy: Option<(i64, Task<()>)>,
    busy_shown: bool,
    pub input: InputView,
    /// `QuickInputViewState`: the dragged position, top and center as fractions of the window.
    view_state: Option<(Option<f32>, Option<f32>)>,
    /// The pointer's offset from the widget's top left while the title or header is dragged.
    drag: Option<Point<Pixels>>,
    /// The widget's padding box as last painted.
    bounds: Rc<Cell<Bounds<Pixels>>>,
}

impl QuickInputs {
    pub fn new(cx: &mut App) -> Self {
        QuickInputs {
            inputs: Vec::new(),
            next_id: 1,
            visible: None,
            focus: cx.focus_handle(),
            previous_focus: None,
            focus_out: None,
            list: ListState::new(0, ListAlignment::Top, px(200.)),
            listed: (None, Vec::new()),
            revealed: None,
            hovered: None,
            closing: None,
            opened: 0,
            busy: None,
            busy_shown: false,
            input: InputView::default(),
            view_state: None,
            drag: None,
            bounds: Rc::default(),
        }
    }

    pub fn add(&mut self, mut input: QuickInput, owner: Owner) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        input.id = id;
        self.inputs.push((input, owner));
        id
    }

    pub fn remove(&mut self, id: i64) {
        self.inputs.retain(|(input, _)| input.id != id);
    }

    pub fn get(&self, id: i64) -> Option<&QuickInput> {
        self.inputs.iter().find(|(input, _)| input.id == id).map(|(input, _)| input)
    }

    pub fn get_mut(&mut self, id: i64) -> Option<&mut QuickInput> {
        self.inputs.iter_mut().find(|(input, _)| input.id == id).map(|(input, _)| input)
    }

    pub fn owner(&self, id: i64) -> Option<&Owner> {
        self.inputs.iter().find(|(input, _)| input.id == id).map(|(_, owner)| owner)
    }

    pub fn owner_mut(&mut self, id: i64) -> Option<&mut Owner> {
        self.inputs.iter_mut().find(|(input, _)| input.id == id).map(|(_, owner)| owner)
    }

    pub fn find_id(&self, matches: impl Fn(&Owner) -> bool) -> Option<i64> {
        self.inputs.iter().find(|(_, owner)| matches(owner)).map(|(input, _)| input.id)
    }

    pub fn find_mut(&mut self, matches: impl Fn(&Owner) -> bool) -> Option<&mut QuickInput> {
        self.inputs.iter_mut().find(|(_, owner)| matches(owner)).map(|(input, _)| input)
    }

    /// `currentQuickInput`: the shown input.
    pub fn current(&self) -> Option<&QuickInput> {
        self.get(self.visible?)
    }

    fn current_mut(&mut self) -> Option<&mut QuickInput> {
        self.get_mut(self.visible?)
    }
}

impl Workbench {
    /// `QuickInputController.show`: the widget shows the input, hiding the one it showed before.
    pub fn show_quick_input(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        let inputs = &mut self.quick_inputs;
        if inputs.visible == Some(id) {
            return;
        }
        match inputs.visible.take() {
            Some(previous) => {
                if let Some(input) = inputs.get_mut(previous) {
                    input.did_hide();
                }
            }
            None => {
                inputs.previous_focus = window.focused(cx).filter(|focused| *focused != inputs.focus);
                inputs.closing = None;
                inputs.drag = None;
                inputs.opened += 1;
            }
        }
        let Some(input) = inputs.get_mut(id) else { return };
        input.show();
        inputs.visible = Some(id);
        inputs.input.reset();
        inputs.hovered = None;
        inputs.revealed = None;
        window.focus(&self.quick_inputs.focus, cx);
        if self.quick_inputs.focus_out.is_none() {
            let focus = self.quick_inputs.focus.clone();
            self.quick_inputs.focus_out = Some(cx.on_focus_out(&focus, window, |workbench, _, window, cx| workbench.quick_input_focus_lost(window, cx)));
        }
        self.blink_caret(InputId::QuickInput, window, cx);
        cx.notify();
    }

    pub fn hide_quick_input_by_id(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        if self.quick_inputs.visible == Some(id) {
            self.hide_quick_input(true, window, cx);
        }
    }

    /// `QuickInputController.hide`: focus goes back where it was, unless the widget hid because it lost it.
    fn hide_quick_input(&mut self, restore_focus: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.quick_inputs.visible.take() else { return };
        let Some(input) = self.quick_inputs.get_mut(id) else { return };
        let closing = input.clone();
        input.did_hide();
        let close = cx.spawn(async move |workbench, cx| {
            cx.background_executor().timer(Duration::from_millis(150)).await;
            let _ = workbench.update(cx, |workbench, cx| {
                workbench.quick_inputs.closing = None;
                cx.notify();
            });
        });
        self.quick_inputs.closing = Some((closing, close));
        self.quick_inputs.input.reset();
        let previous = self.quick_inputs.previous_focus.take();
        if restore_focus && self.quick_inputs.focus.is_focused(window) {
            match previous {
                Some(previous) => window.focus(&previous, cx),
                None => window.blur(cx),
            }
        }
        cx.notify();
    }

    /// `QuickInput.dispose`: hides it, tells its owner, and forgets it.
    pub fn dispose_quick_input(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        self.hide_quick_input_by_id(id, window, cx);
        self.flush_quick_input_events(window, cx);
        self.quick_inputs.remove(id);
    }

    /// Hands the events of every quick input to its owner, oldest first, until none is left.
    pub fn flush_quick_input_events(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        while let Some((id, events)) = self.quick_inputs.inputs.iter_mut().find(|(input, _)| !input.events.is_empty()).map(|(input, _)| (input.id, std::mem::take(&mut input.events))) {
            for event in events {
                self.deliver_quick_input_event(id, event, window, cx);
            }
        }
        let busy = self.quick_inputs.current().filter(|input| input.busy).map(|input| input.id);
        if busy != self.quick_inputs.busy.as_ref().map(|(id, _)| *id) {
            self.quick_inputs.busy_shown = false;
            self.quick_inputs.busy = busy.map(|id| {
                let show = cx.spawn(async move |workbench, cx| {
                    cx.background_executor().timer(Duration::from_millis(800)).await;
                    let _ = workbench.update(cx, |workbench, cx| {
                        workbench.quick_inputs.busy_shown = true;
                        cx.notify();
                    });
                });
                (id, show)
            });
        }
        cx.notify();
    }

    /// The `blur` of the widget: it hides unless it ignores focus out.
    fn quick_input_focus_lost(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.quick_inputs.current().is_some_and(|input| !input.ignore_focus_out) {
            self.hide_quick_input(false, window, cx);
            self.flush_quick_input_events(window, cx);
        }
    }

    /// Runs `change` on the shown input, then delivers what it fired.
    pub(super) fn with_current_quick_input(&mut self, window: &mut Window, cx: &mut Context<Self>, change: impl FnOnce(&mut QuickInput)) -> Outcome {
        let Some(input) = self.quick_inputs.current_mut() else { return Outcome::done() };
        change(input);
        self.flush_quick_input_events(window, cx);
        Outcome::done()
    }

    /// The context keys of quickInputController.ts and quickInputService.ts.
    pub(super) fn quick_input_context(&self, window: &Window, context: &mut std::collections::HashMap<String, Value>) {
        let Some(input) = self.quick_inputs.current() else { return };
        context.insert("inQuickOpen".into(), true.into());
        // `contextKey` of the quick access provider: Quick Open's is `inFilesPicker`.
        if let Some(Owner::QuickAccess(_)) = self.quick_inputs.owner(input.id)
            && super::quick_access::is_files_picker(&input.input_box.value)
        {
            context.insert("inFilesPicker".into(), true.into());
        }
        if !self.quick_inputs.focus.is_focused(window) {
            return;
        }
        context.insert("inQuickInput".into(), true.into());
        context.insert("inputFocus".into(), true.into());
        let kind = match input.kind {
            Kind::QuickPick => "quickPick",
            Kind::InputBox => "inputBox",
        };
        context.insert("quickInputType".into(), kind.into());
        context.insert("cursorAtEndOfQuickInputBox".into(), input.input_box.caret_at_end().into());
    }

    /// quickInputActions.ts and quickAccessActions.ts.
    pub(super) fn register_quick_input_commands(&mut self) {
        let focus_commands = [
            ("quickInput.next", Focus::Next),
            ("quickInput.previous", Focus::Previous),
            ("quickInput.first", Focus::First),
            ("quickInput.last", Focus::Last),
            ("quickInput.pageNext", Focus::NextPage),
            ("quickInput.pagePrevious", Focus::PreviousPage),
            ("quickInput.nextSeparator", Focus::NextSeparator),
            ("quickInput.previousSeparator", Focus::PreviousSeparator),
            // The quick access fallback applies to quick navigation (Ctrl+Tab style pickers), which the port has none of.
            ("quickInput.nextSeparatorWithQuickAccessFallback", Focus::NextSeparator),
            ("quickInput.previousSeparatorWithQuickAccessFallback", Focus::PreviousSeparator),
            ("workbench.action.quickOpenSelectNext", Focus::Next),
            ("workbench.action.quickOpenSelectPrevious", Focus::Previous),
            ("workbench.action.quickOpenNavigateNext", Focus::Next),
            ("workbench.action.quickOpenNavigatePrevious", Focus::Previous),
            ("workbench.action.quickOpenNavigateNextInFilePicker", Focus::Next),
            ("workbench.action.quickOpenNavigatePreviousInFilePicker", Focus::Previous),
        ];
        for (id, focus) in focus_commands {
            self.commands.register(id, move |workbench, _, window, cx| {
                let page = workbench.quick_input_page_size(window);
                workbench.with_current_quick_input(window, cx, |input| input.focus(focus, page))
            });
        }
        self.commands.register("quickInput.accept", |workbench, _, window, cx| workbench.with_current_quick_input(window, cx, |input| input.accept(false)));
        self.commands.register("quickInput.acceptInBackground", |workbench, _, window, cx| workbench.with_current_quick_input(window, cx, |input| input.accept(true)));
        for id in ["workbench.action.acceptSelectedQuickOpenItem", "workbench.action.alternativeAcceptSelectedQuickOpenItem"] {
            self.commands.register(id, |workbench, _, window, cx| workbench.with_current_quick_input(window, cx, QuickInput::accept_from_ui));
        }
        for id in ["quickInput.toggleCheckbox", "workbench.action.quickPickManyToggle"] {
            self.commands.register(id, |workbench, _, window, cx| workbench.with_current_quick_input(window, cx, QuickInput::toggle));
        }
        for id in ["quickInput.hide", "workbench.action.closeQuickOpen"] {
            self.commands.register(id, |workbench, _, window, cx| {
                workbench.hide_quick_input(true, window, cx);
                workbench.flush_quick_input_events(window, cx);
                Outcome::done()
            });
        }
        self.commands.register("workbench.action.focusQuickOpen", |workbench, _, window, cx| {
            if workbench.quick_inputs.current().is_some() {
                window.focus(&workbench.quick_inputs.focus, cx);
            }
            Outcome::done()
        });
        self.commands.register("workbench.action.quickInputBack", |workbench, _, window, cx| {
            workbench.with_current_quick_input(window, cx, |input| {
                if input.buttons.iter().any(|button| button.handle == -1) {
                    input.events.push(Event::TriggerButton(-1, None));
                }
            })
        });
    }

    fn quick_input_page_size(&self, window: &Window) -> usize {
        (self.quick_input_list_max_height(window) / ROW_HEIGHT).floor().max(1.) as usize
    }

    /// `QuickInputList.layout(height * 0.4)`: whole 44px rows plus 6px to show there is more.
    fn quick_input_list_max_height(&self, window: &Window) -> f32 {
        let height = f32::from(window.viewport_size().height) * 0.4;
        (height / DETAIL_ROW_HEIGHT).floor() * DETAIL_ROW_HEIGHT + 6.
    }

    /// The widget, drawn over the workbench at the top center or where it was dragged.
    pub(super) fn quick_input_widget(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (input, interactive) = match (self.quick_inputs.current(), self.quick_inputs.closing.as_ref()) {
            (Some(input), _) => (input.clone(), true),
            (None, Some((input, _))) => (input.clone(), false),
            (None, None) => return None,
        };
        // Modern UI frosted glass (frostedGlass.css), on outside high contrast themes, drops the widget's animations.
        let frosted = !self.theme.kind.is_high_contrast();
        if frosted && !interactive {
            return None;
        }
        let viewport = window.viewport_size();
        let width = (f32::from(viewport.width) * 0.62).min(MAX_WIDTH);
        let (top, left) = match self.quick_inputs.view_state {
            Some((top, left)) => (top.map_or(TOP, |top| (f32::from(viewport.height) * top).round()), (f32::from(viewport.width) * left.unwrap_or(0.5) - width / 2.).round()),
            None => (TOP, ((f32::from(viewport.width) - width) / 2.).round()),
        };
        let has_title = !input.title_text().is_empty() || !input.buttons.iter().all(|button| button.location.is_some_and(|location| location > 1));
        let widget = div()
            .id("quick-input-widget")
            .absolute()
            .top(px(top))
            .left(px(left))
            // The width is the content box; the 1px border goes outside it.
            .w(px(width + if self.theme.color("widget.border").is_some() { 2. } else { 0. }))
            .flex()
            .flex_col()
            // cornerRadius.large under Modern UI (roundedCorners.css).
            .rounded(px(RADIUS))
            .when(!frosted, |widget| widget.bg(self.color("quickInput.background")))
            .when(frosted, |widget| widget.child(self.frosted_glass("quickInput.background", RADIUS)))
            .when(interactive, |widget| widget.child(self.quick_input_drag_tracker(cx)))
            .text_color(self.foreground_or("quickInput.foreground"))
            .when_some(self.theme.color("widget.border"), |widget, border| widget.border_1().border_color(border))
            // --vscode-shadow-xl.
            .shadow(vec![BoxShadow { color: rgba(0x00000026).into(), offset: point(px(0.), px(0.)), blur_radius: px(20.), spread_radius: px(0.), inset: false }])
            .when(interactive, |widget| widget.on_mouse_down_out(cx.listener(|workbench, _, window, cx| workbench.quick_input_focus_lost(window, cx))))
            .when(has_title, |widget| widget.child(self.quick_input_title(&input, cx)))
            .when(input.description.as_ref().is_some_and(|description| !description.is_empty()), |widget| {
                widget.child(div().mx(px(11.)).my(px(6.)).mr(px(6.)).child(input.description.clone().unwrap_or_default()))
            })
            .child(self.quick_input_header(&input, interactive, window, cx))
            .children(input.message().map(|message| self.quick_input_message(&input, message)))
            .child(self.quick_input_progress(&input))
            .when(input.kind == Kind::QuickPick, |widget| widget.child(self.quick_input_list(&input, interactive, window, cx)));
        if frosted {
            return Some(widget.into_any_element());
        }
        // shortcut: GPUI has no element transforms, so the open (250ms) and close (150ms) animations fade without their 0.97/0.99 scale.
        let ease = |t: f32| 1. - (1. - t).powi(4);
        let element = if interactive {
            widget.with_animation(("quick-input-open", self.quick_inputs.opened), Animation::new(Duration::from_millis(250)).with_easing(ease), |widget, delta| widget.opacity(delta)).into_any_element()
        } else {
            widget.with_animation(("quick-input-close", self.quick_inputs.opened), Animation::new(Duration::from_millis(150)).with_easing(ease), |widget, delta| widget.opacity(1. - delta)).into_any_element()
        };
        Some(element)
    }

    /// The `::before` layer of frostedGlass.css: the surface color at 92% over a 12px blur of what is behind it,
    /// inside a 1px border with `radius` corners.
    pub(super) fn frosted_glass(&self, surface: &str, radius: f32) -> impl IntoElement {
        let mut color = self.color(surface);
        color.a *= 0.92;
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                // Inside the 1px border, so the corners follow the padding box.
                let radii = gpui::Corners::all(px(radius - 1.));
                window.paint_backdrop_blur(bounds, radii, px(12.));
                window.paint_quad(fill(bounds, color).corner_radii(radii));
            },
        )
        .absolute()
        .inset_0()
    }

    /// Records the widget's bounds and, while it is dragged, follows the pointer anywhere in the window.
    fn quick_input_drag_tracker(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let last_bounds = self.quick_inputs.bounds.clone();
        let dragging = self.quick_inputs.drag.is_some();
        let workbench = cx.entity();
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                last_bounds.set(bounds);
                if !dragging {
                    return;
                }
                let moved = workbench.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble {
                        moved.update(cx, |workbench, cx| workbench.quick_input_drag_to(event.position, window, cx));
                    }
                });
                window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble {
                        workbench.update(cx, |workbench, cx| {
                            workbench.quick_inputs.drag = None;
                            workbench.save_quick_input_view_state();
                            cx.notify();
                        });
                    }
                });
            },
        )
        .absolute()
        .inset_0()
    }

    /// `loadViewState`: the position stored by the last drag, if any.
    pub(super) fn load_quick_input_view_state(&mut self) {
        let storage = self.profile_storage.lock().unwrap();
        let data: Value = storage.get(VIEWSTATE_STORAGE_KEY).and_then(|raw| serde_json::from_str(raw).ok()).unwrap_or_default();
        self.quick_inputs.view_state = match (data["top"].as_f64(), data["left"].as_f64()) {
            (None, None) => None,
            (top, left) => Some((top.map(|top| top as f32), left.map(|left| left as f32))),
        };
    }

    /// `saveViewState` in StorageScope.APPLICATION; the default position removes the key.
    fn save_quick_input_view_state(&self) {
        let mut storage = self.profile_storage.lock().unwrap();
        match self.quick_inputs.view_state {
            Some((top, left)) => {
                // JSON.stringify leaves out an undefined side.
                let mut state = serde_json::Map::new();
                if let Some(top) = top {
                    state.insert("top".into(), top.into());
                }
                if let Some(left) = left {
                    state.insert("left".into(), left.into());
                }
                storage.set(VIEWSTATE_STORAGE_KEY, Value::Object(state).to_string());
            }
            None => storage.delete(VIEWSTATE_STORAGE_KEY),
        }
    }

    /// A press on the title or header starts a drag; a double click puts the widget back at the top center.
    fn quick_input_drag_start(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if event.click_count == 2 {
            self.quick_inputs.view_state = None;
            self.quick_inputs.drag = None;
            self.save_quick_input_view_state();
            cx.notify();
            return;
        }
        let border = if self.theme.color("widget.border").is_some() { px(1.) } else { px(0.) };
        self.quick_inputs.drag = Some(event.position - self.quick_inputs.bounds.get().origin + point(border, border));
        cx.notify();
    }

    fn quick_input_drag_to(&mut self, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(offset) = self.quick_inputs.drag else { return };
        let viewport = window.viewport_size();
        let widget = self.quick_inputs.bounds.get().size;
        let top_left = position - offset;
        let window_size = (f32::from(viewport.width), f32::from(viewport.height));
        let widget_size = (f32::from(widget.width), f32::from(widget.height));
        let (top, left) = drag_layout(f32::from(top_left.y), f32::from(top_left.x), window_size, widget_size, TITLE_BAR_HEIGHT, TOP);
        self.quick_inputs.view_state = Some((Some(top), Some(left)));
        cx.notify();
    }

    /// `.quick-input-titlebar`: the back button, the title, and the title buttons.
    fn quick_input_title(&self, input: &QuickInput, cx: &mut Context<Self>) -> Div {
        let (left, right): (Vec<&Button>, Vec<&Button>) = input.buttons.iter().filter(|button| button.location.is_none_or(|location| location <= 1)).partition(|button| button.handle == -1);
        let title = input.title_text();
        div()
            .relative()
            .flex()
            .items_center()
            .rounded_t(px(RADIUS))
            .pt(px(2.))
            .bg(self.color("quickInputTitle.background"))
            .cursor_grab()
            .on_mouse_down(MouseButton::Left, cx.listener(Self::quick_input_drag_start))
            // The action bars are not drag areas (excludeNodes).
            .child(div().absolute().left(px(4.)).occlude().flex().children(left.into_iter().map(|button| self.quick_input_button(button, 20., cx))))
            .child(div().flex_1().py(px(3.)).text_center().child(if title.is_empty() { "\u{a0}".to_owned() } else { title }))
            .child(div().absolute().right(px(4.)).occlude().flex().gap(px(4.)).children(right.into_iter().map(|button| self.quick_input_button(button, 20., cx))))
    }

    /// A title, inline or input button: a 16px codicon in a `size` box, or the toggle's checked look.
    fn quick_input_button(&self, button: &Button, size: f32, cx: &mut Context<Self>) -> AnyElement {
        let codicon = match (&button.codicon, button.handle) {
            (Some(codicon), _) => codicon.clone(),
            (None, -1) => "arrow-left".to_owned(),
            (None, _) => "blank".to_owned(),
        };
        let handle = button.handle;
        let toggle = button.toggle;
        let foreground = match toggle {
            Some(true) => self.color("inputOption.activeForeground"),
            _ => self.foreground_or("icon.foreground"),
        };
        let element = match &button.icon_path {
            Some(icon) if button.codicon.is_none() => {
                let path = if self.theme.kind.is_light() { &icon.light } else { &icon.dark };
                div().size(px(size)).flex().items_center().justify_center().child(gpui::img(path.clone()).size(px(16.)))
            }
            _ => self.icon(&codicon, size, 16., foreground),
        };
        element
            .id(("quick-input-button", handle as u64))
            .cursor_pointer()
            .rounded(px(5.))
            .when(toggle == Some(true), |button| {
                button.bg(self.color("inputOption.activeBackground")).border_1().border_color(self.color("inputOption.activeBorder"))
            })
            .hover(|button| button.bg(self.color("toolbar.hoverBackground")))
            .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                workbench.with_current_quick_input(window, cx, |input| {
                    if let Some(button) = input.buttons.iter_mut().find(|button| button.handle == handle) {
                        button.toggle = button.toggle.map(|checked| !checked);
                        input.events.push(Event::TriggerButton(handle, button.toggle));
                    }
                });
            }))
            .into_any_element()
    }

    /// `.quick-input-header`: the check-all box, the input box with its count, inline buttons and OK.
    fn quick_input_header(&self, input: &QuickInput, interactive: bool, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let checked = input.checked_count();
        let pickable = input.rows.iter().filter(|row| row.item.pickable && !row.hidden).count();
        let all_checked = pickable > 0 && input.order.iter().filter(|row| input.rows[**row].item.pickable).all(|row| input.rows[*row].checked);
        let inline: Vec<&Button> = input.buttons.iter().filter(|button| button.location == Some(2)).collect();
        div()
            .flex()
            .px(px(6.))
            .pt(px(6.))
            .pb(px(4.))
            .when(input.hide_input, |header| header.p(px(0.)))
            // Only the header itself is a drag area, so its children occlude it.
            .cursor_grab()
            .on_mouse_down(MouseButton::Left, cx.listener(Self::quick_input_drag_start))
            .when(input.can_select_many && !input.hide_input, |header| {
                header.child(
                    self.checkbox(all_checked, checked > 0 && !all_checked)
                        .id("quick-input-check-all")
                        .occlude()
                        .cursor_pointer()
                        .mt(px(4.))
                        .on_click(cx.listener(|workbench, _: &ClickEvent, window, cx| {
                            workbench.with_current_quick_input(window, cx, QuickInput::toggle_all);
                        })),
                )
            })
            .when(!input.hide_input, |header| header.child(self.quick_input_box(input, interactive, window, cx).occlude()))
            // quickInput.css: 5px before the first inline action, 4px between, 2px from the top; actionbar.css pads the label 3px.
            .children(inline.into_iter().enumerate().map(|(i, button)| div().ml(px(if i == 0 { 5. } else { 4. })).mt(px(2.)).occlude().child(self.quick_input_button(button, 22., cx))))
            .when(input.can_select_many, |header| {
                header.child(
                    div()
                        .id("quick-input-ok")
                        .occlude()
                        .cursor_pointer()
                        .ml(px(6.))
                        .h(px(26.))
                        .px(px(10.))
                        .flex()
                        .items_center()
                        .rounded(px(4.))
                        .bg(self.color("button.background"))
                        .text_color(self.color("button.foreground"))
                        .when_some(self.theme.color("button.border"), |button, border| button.border_1().border_color(border))
                        .hover(|button| button.bg(self.color("button.hoverBackground")))
                        .child("OK")
                        .on_click(cx.listener(|workbench, _: &ClickEvent, window, cx| {
                            workbench.with_current_quick_input(window, cx, QuickInput::accept_from_ui);
                        })),
                )
            })
    }

    /// The `.monaco-inputbox`: 26px, `input.*` colors, the focus border, the validation border, the count badge.
    fn quick_input_box(&self, input: &QuickInput, interactive: bool, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let focused = interactive && self.quick_inputs.focus.is_focused(window);
        let validation = match input.severity {
            Severity::Info => self.theme.color("inputValidation.infoBorder"),
            Severity::Warning => self.theme.color("inputValidation.warningBorder"),
            Severity::Error => self.theme.color("inputValidation.errorBorder"),
            Severity::Ignore => None,
        };
        let border = validation.or(focused.then(|| self.theme.color("focusBorder")).flatten()).or(self.theme.color("input.border"));
        let input_buttons: Vec<&Button> = input.buttons.iter().filter(|button| button.location == Some(3)).collect();
        let count = input.can_select_many.then(|| {
            div()
                .flex_none()
                // .quick-input-count inherits the header's grab cursor.
                .cursor_grab()
                .mr(px(4.))
                .px(px(4.))
                .py(px(2.))
                .rounded(px(2.))
                .text_size(px(11.))
                .bg(self.color("badge.background"))
                .text_color(self.color("badge.foreground"))
                .when_some(self.theme.color("contrastBorder"), |badge, border| badge.border_1().border_color(border))
                .child(input.checked_count().to_string())
        });
        div()
            .flex_1()
            .cursor_text()
            // 1px borders, 4px padding and the 15px normal line height of 13px text, measured on 1.147.
            .h(px(25.))
            .flex()
            .items_center()
            .rounded(px(6.))
            .border_1()
            .border_color(border.unwrap_or(rgba(0x00000000)))
            .bg(self.color("input.background"))
            .text_color(self.foreground_or("input.foreground"))
            .when(input.can_select_many, |input_box| input_box.ml(px(5.)))
            .child(div().flex_1().h_full().mx(px(6.)).flex().child(self.input_text(InputId::QuickInput, InputLook { password: input.password, placeholder: input.placeholder.clone(), focused }, cx)))
            .children(count)
            .children(input_buttons.into_iter().map(|button| div().mr(px(2.)).child(self.quick_input_button(button, 20., cx))))
    }

    /// `.quick-input-message`: the prompt, or the validation message in its severity's colors.
    fn quick_input_message(&self, input: &QuickInput, message: String) -> Div {
        let colors = match input.severity {
            Severity::Info => Some(("inputValidation.infoBackground", "inputValidation.infoForeground", "inputValidation.infoBorder")),
            Severity::Warning => Some(("inputValidation.warningBackground", "inputValidation.warningForeground", "inputValidation.warningBorder")),
            Severity::Error => Some(("inputValidation.errorBackground", "inputValidation.errorForeground", "inputValidation.errorBorder")),
            Severity::Ignore => None,
        };
        let message_box = div().mx(px(6.)).mt(px(-1.)).p(px(5.)).child(message);
        match colors {
            Some((background, foreground, border)) => message_box
                .mb(px(-2.))
                .bg(self.color(background))
                .when_some(self.theme.color(foreground), |message_box, color| message_box.text_color(color))
                .when_some(self.theme.color(border), |message_box, border| message_box.border_1().border_color(border)),
            None => message_box,
        }
    }

    /// The 2px infinite progress bar (progressbar.css: a 2% bit crossing in 4s, scaling to 3x halfway).
    fn quick_input_progress(&self, input: &QuickInput) -> AnyElement {
        let shown = self.quick_inputs.busy_shown && self.quick_inputs.busy.as_ref().is_some_and(|(id, _)| *id == input.id);
        let bar = div().relative().h(px(2.)).overflow_hidden();
        if !shown {
            return bar.into_any_element();
        }
        let color = self.color("progressBar.background");
        bar.child(
            div().absolute().top_0().h_full().w(gpui::relative(0.02)).bg(color).with_animation(
                "quick-input-progress",
                Animation::new(Duration::from_secs(4)).repeat(),
                |bit, delta| {
                    let (offset, scale) = if delta < 0.5 { (delta * 2. * 25., 1. + delta * 4.) } else { (25. + (delta - 0.5) * 2. * 24., 3. - (delta - 0.5) * 4.) };
                    bit.left(gpui::relative(offset * 0.02)).w(gpui::relative(0.02 * scale))
                },
            ),
        )
        .into_any_element()
    }

    /// `.quick-input-list`: the visible rows, at most the layout's height, with 7px below them.
    fn quick_input_list(&mut self, input: &QuickInput, interactive: bool, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let inputs = &mut self.quick_inputs;
        if inputs.listed.0 != Some(input.id) || inputs.listed.1 != input.order {
            inputs.list.reset(input.order.len());
            inputs.listed = (Some(input.id), input.order.clone());
            inputs.revealed = None;
        }
        let focused_position = input.focused.and_then(|row| input.order.iter().position(|shown| *shown == row));
        if interactive && focused_position.is_some() && inputs.revealed != focused_position {
            inputs.list.scroll_to_reveal_item(focused_position.unwrap_or_default());
            inputs.revealed = focused_position;
        }
        let content: f32 = input.order.iter().map(|row| row_height(&input.rows[*row])).sum();
        let height = content.min(self.quick_input_list_max_height(window)).min(20. * ROW_HEIGHT);
        let rows = list(self.quick_inputs.list.clone(), cx.processor(move |workbench: &mut Workbench, position: usize, window, cx| workbench.quick_input_row(position, interactive, window, cx)))
            .size_full();
        // .monaco-list: 7px bottom padding the rows scroll under; .monaco-scrollable-element: 6px on the sides.
        let rows = div().h(px(height + 7.)).px(px(6.)).child(rows);
        div().when(input.hide_input, |list| list.mt(px(4.)).pb(px(4.))).child(rows)
    }

    /// A `.quick-input-list-entry`.
    fn quick_input_row(&mut self, position: usize, interactive: bool, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let input = match (self.quick_inputs.current(), self.quick_inputs.closing.as_ref()) {
            (Some(input), _) if interactive => input,
            (_, Some((input, _))) => input,
            _ => return div().into_any_element(),
        };
        let Some(&row_index) = input.order.get(position) else { return div().into_any_element() };
        let row = &input.rows[row_index];
        let focused = input.focused == Some(row_index);
        let hovered = self.quick_inputs.hovered == Some(row_index);
        let can_select_many = input.can_select_many;
        let foreground = if focused { self.theme.color("quickInputList.focusForeground") } else { None };
        let icon_color = if focused { self.theme.color("quickInputList.focusIconForeground") } else { None }.unwrap_or(self.foreground_or("icon.foreground"));
        let highlight = self.color(if focused { "quickInputList.focusHighlightForeground" } else { "list.highlightForeground" });
        let label_box = div()
            .flex_1()
            .min_w_0()
            .ml(px(5.))
            .flex()
            .flex_col()
            .justify_center()
            .overflow_hidden()
            .child(
                div()
                    .h(px(ROW_HEIGHT))
                    .flex()
                    .items_center()
                    .whitespace_nowrap()
                    .when(row.item.icon.is_some() || row.item.icon_path.is_some(), |line| {
                        let icon = match (&row.item.icon, &row.item.icon_path) {
                            (Some(codicon), _) => self.icon(codicon, 16., 16., icon_color),
                            (None, Some(path)) => {
                                let path = if self.theme.kind.is_light() { &path.light } else { &path.dark };
                                div().size(px(16.)).flex_none().child(gpui::img(path.clone()).size(px(16.)))
                            }
                            (None, None) => div(),
                        };
                        line.child(icon.mr(px(6.)))
                    })
                    .children(row.item.file_icon.as_ref().and_then(|path| self.quick_input_file_icon(path, icon_color)))
                    .child(self.highlighted_label(&row.item.label, &row.label_highlights, highlight, 16., icon_color))
                    .children(row.item.description.as_ref().filter(|description| !description.is_empty()).map(|description| {
                        // monaco-icon-label: .label-description at 0.9em, 0.7 opacity, 0.5em of its own 11.7px after the name.
                        div().ml(px(5.85)).text_size(px(11.7)).opacity(if focused { 1. } else { 0.7 }).child(self.highlighted_label(description, &row.description_highlights, highlight, 14., icon_color))
                    })),
            )
            .children(row.item.detail.as_ref().filter(|detail| !detail.is_empty()).map(|detail| {
                div().h(px(ROW_HEIGHT)).flex().items_center().whitespace_nowrap().opacity(if focused { 1. } else { 0.7 }).child(self.highlighted_label(detail, &row.detail_highlights, highlight, 14., icon_color))
            }));
        let separator = row.separator.clone().filter(|label| !label.is_empty());
        // quickInput.css: `.quick-input-list-entry .quick-input-list-separator` outranks the controller's pickerGroup.foreground rule.
        let separator_color = if focused { foreground.unwrap_or(self.foreground_or("quickInput.foreground")) } else { self.color("descriptionForeground") };
        let buttons_shown = focused || hovered;
        let handle = row.item.handle;
        let entry = div()
            .relative()
            .size_full()
            .flex()
            // Measured on 1.147: keybinding caps and the gear end 4px from the row edge, labels start at 9px.
            .pl(px(4.))
            .pr(px(2.))
            .when(row.separator.is_some() && position > 0, |entry| entry.border_t_1().border_color(self.color("pickerGroup.border")))
            .when(can_select_many, |entry| {
                entry.child(
                    self.checkbox(row.checked, false)
                        .id(("quick-input-row-check", row_index))
                        .mt(px(2.))
                        .when(row.item.pickable, |checkbox| checkbox.cursor_pointer())
                        .when(!row.item.pickable, |checkbox| checkbox.opacity(0.4)),
                )
            })
            .child(label_box)
            .when(!row.item.keybinding.is_empty(), |entry| entry.child(self.keybinding_label(&row.item.keybinding, focused || hovered)))
            // The entry's right padding is 4px on 1.147, so the separator label ends 4px from the row edge.
            .children(separator.map(|label| div().flex().items_center().ml(px(4.)).mr(px(2.)).text_size(px(11.)).text_color(separator_color).child(label)))
            .when(row.item.buttons.iter().any(|button| buttons_shown || button.always_visible), |entry| {
                let buttons = row.item.buttons.iter().enumerate().filter(|(_, button)| buttons_shown || button.always_visible);
                entry.child(div().flex().items_center().mt(px(1.)).children(buttons.map(|(index, button)| {
                    let toggle = button.toggle;
                    self.icon(button.codicon.as_deref().unwrap_or("blank"), 20., 16., icon_color)
                        .id(("quick-input-item-button", index))
                        .mr(px(2.))
                        .rounded(px(5.))
                        .hover(|button| button.bg(self.color("toolbar.hoverBackground")))
                        .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                            cx.stop_propagation();
                            workbench.with_current_quick_input(window, cx, |input| {
                                input.events.push(Event::TriggerItemButton(handle, index as i64, toggle.map(|checked| !checked)));
                            });
                        }))
                })))
            });
        let background = if focused { self.theme.color("quickInputList.focusBackground") } else if hovered { self.theme.color("list.hoverBackground") } else { None };
        div()
            .id(("quick-input-row", row_index))
            .h(px(row_height(row)))
            .cursor_pointer()
            .rounded(px(3.))
            .when_some(background, |row, background| row.bg(background))
            .when_some(foreground, |row, color| row.text_color(color))
            .child(entry)
            .when(interactive, |row| {
                row.on_hover(cx.listener(move |workbench, hovered: &bool, _, cx| {
                    let inputs = &mut workbench.quick_inputs;
                    if *hovered {
                        inputs.hovered = Some(row_index);
                    } else if inputs.hovered == Some(row_index) {
                        inputs.hovered = None;
                    }
                    cx.notify();
                }))
                .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                    workbench.with_current_quick_input(window, cx, |input| input.click_row(row_index));
                }))
            })
            .into_any_element()
    }

    /// iconlabel.css `.monaco-icon-label::before`: 16px wide, 22px tall, 6px before the label.
    fn quick_input_file_icon(&self, path: &Path, foreground: Rgba) -> Option<Div> {
        let icon = self.file_icons.as_ref()?.file_icon(path, self.languages.language_of(path, ""), self.theme.kind, 13., ROW_HEIGHT, foreground)?;
        Some(icon.mr(px(6.)))
    }

    /// A `HighlightedLabel` with `supportIcons`: `$(name)` draws a codicon, matches are bold in the highlight color.
    fn highlighted_label(&self, text: &str, highlights: &[Range<usize>], color: Rgba, icon_size: f32, icon_color: Rgba) -> Div {
        let chars: Vec<char> = text.chars().collect();
        let mut line = div().flex().items_center().min_w_0();
        let mut segment = String::new();
        let mut segment_highlights = Vec::new();
        let mut index = 0;
        let flush = |line: Div, segment: &mut String, segment_highlights: &mut Vec<(Range<usize>, HighlightStyle)>| {
            if segment.is_empty() {
                return line;
            }
            let styled = StyledText::new(std::mem::take(segment)).with_highlights(std::mem::take(segment_highlights));
            line.child(styled)
        };
        while index < chars.len() {
            if let Some((name, length)) = icon_at(&chars[index..]) {
                line = flush(line, &mut segment, &mut segment_highlights);
                line = line.child(self.icon(&name, icon_size, icon_size, icon_color));
                index += length;
                continue;
            }
            let start = segment.len();
            segment.push(chars[index]);
            if highlights.iter().any(|range| range.contains(&index)) {
                segment_highlights.push((start..segment.len(), HighlightStyle { color: Some(color.into()), font_weight: Some(FontWeight::BOLD), ..Default::default() }));
            }
            index += 1;
        }
        flush(line, &mut segment, &mut segment_highlights)
    }

    /// The `.monaco-keybinding` of a row: a box per key, chords apart.
    fn keybinding_label(&self, chords: &[Vec<String>], row_active: bool) -> Div {
        let border = self.color("keybindingLabel.border");
        let bottom = self.color("keybindingLabel.bottomBorder");
        let shadow = self.color("keybindingLabel.shadow");
        let key = |label: &String| {
            div()
                .mx(px(2.))
                .px(px(5.))
                .h(px(18.))
                .min_w(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(3.))
                .text_size(px(11.))
                .border_1()
                .border_color(border)
                .border_b_1()
                .when(!row_active, |key| key.bg(self.color("keybindingLabel.background")))
                .text_color(self.foreground_or("keybindingLabel.foreground"))
                .shadow(vec![BoxShadow { color: shadow.into(), offset: point(px(0.), px(-1.)), blur_radius: px(0.), spread_radius: px(0.), inset: true }])
                .child(div().border_b_1().border_color(bottom))
                .child(label.clone())
        };
        div().flex().items_center().children(chords.iter().enumerate().map(|(index, chord)| div().flex().when(index > 0, |chord| chord.ml(px(5.))).children(chord.iter().map(key))))
    }

    /// The 18px `.monaco-checkbox` of the list and the header: tri-state when `mixed`.
    fn checkbox(&self, checked: bool, mixed: bool) -> Div {
        let icon = if mixed { Some("dash") } else if checked { Some("check") } else { None };
        div()
            .size(px(18.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(3.))
            .border_1()
            .border_color(self.color("checkbox.border"))
            .bg(self.color("checkbox.background"))
            .children(icon.map(|icon| self.icon(icon, 16., 16., self.color("checkbox.foreground"))))
    }
}

fn row_height(row: &Row) -> f32 {
    if row.item.detail.as_ref().is_some_and(|detail| !detail.is_empty()) { DETAIL_ROW_HEIGHT } else { ROW_HEIGHT }
}

/// `$(name)` or `$(name~spin)` at the start of `chars`: the codicon name and the chars it spans.
fn icon_at(chars: &[char]) -> Option<(String, usize)> {
    if chars.len() < 4 || chars[0] != '$' || chars[1] != '(' {
        return None;
    }
    let close = chars.iter().position(|c| *c == ')')?;
    let inner: String = chars[2..close].iter().collect();
    let name = inner.split('~').next().unwrap_or_default();
    let valid = !name.is_empty() && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    valid.then(|| (name.to_owned(), close + 1))
}

#[cfg(test)]
mod tests;
