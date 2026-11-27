//! layoutActions.ts `CustomizeLayoutAction` (the Customize Layout quick pick), `AlignQuickInputTopAction` and
//! `AlignQuickInputCenterAction`, and sidebarPart.ts `workbench.action.toggleActivityBarVisibility`.

use gpui::{Context, Window};
use serde_json::json;

use super::Workbench;
use crate::commands::Outcome;
use crate::contextkey::Expr;
use crate::main_thread_quick_open::Owner;
use crate::quick_input::{Button, Entry, Event, Item, Kind, QuickInput};

/// A `CustomizeLayoutItem`: a visibility toggle with an eye button (`CreateToggleLayoutItem`) or an option checked
/// while `active` holds (`CreateOptionLayoutItem`).
struct LayoutItem {
    command: &'static str,
    active: &'static str,
    label: &'static str,
    icon: VisualIcon,
    toggle: bool,
}

/// `LayoutVisualIcon`: one codicon, or one per side of the primary side bar (`ContextualLayoutVisualIcon`).
enum VisualIcon {
    Fixed(&'static str),
    BySideBar { left: &'static str, right: &'static str },
}

const SIDE_BAR_LEFT: &str = "config.workbench.sideBar.location == 'left'";
const MODERN_UI: &str = "config.workbench.experimental.modernUI == true";

const fn toggle(command: &'static str, active: &'static str, label: &'static str, icon: VisualIcon) -> LayoutItem {
    LayoutItem { command, active, label, icon, toggle: true }
}

const fn option(command: &'static str, active: &'static str, label: &'static str, icon: &'static str) -> LayoutItem {
    LayoutItem { command, active, label, icon: VisualIcon::Fixed(icon), toggle: false }
}

/// The sections in `getItems` order; the Menu Bar toggle is left out on native macOS, as upstream does.
const SECTIONS: &[(&str, &[LayoutItem])] = &[
    ("Visibility", &[
        toggle("workbench.action.toggleActivityBarVisibility", "config.workbench.activityBar.location != 'hidden'", "Activity Bar", VisualIcon::BySideBar { left: "layout-activitybar-left", right: "layout-activitybar-right" }),
        toggle("workbench.action.toggleSidebarVisibility", "sideBarVisible", "Primary Side Bar", VisualIcon::BySideBar { left: "layout-sidebar-left", right: "layout-sidebar-right" }),
        toggle("workbench.action.toggleAuxiliaryBar", "auxiliaryBarVisible", "Secondary Side Bar", VisualIcon::BySideBar { left: "layout-sidebar-right", right: "layout-sidebar-left" }),
        toggle("workbench.action.togglePanel", "panelVisible", "Panel", VisualIcon::Fixed("layout-panel")),
        toggle("workbench.action.toggleStatusbarVisibility", "config.workbench.statusBar.visible == true", "Status Bar", VisualIcon::Fixed("layout-statusbar")),
    ]),
    ("Primary Side Bar Position", &[
        option("workbench.action.moveSideBarLeft", SIDE_BAR_LEFT, "Left", "layout-sidebar-left"),
        option("workbench.action.moveSideBarRight", "config.workbench.sideBar.location == 'right'", "Right", "layout-sidebar-right"),
    ]),
    ("Panel Alignment", &[
        option("workbench.action.alignPanelLeft", "panelAlignment == 'left'", "Left", "layout-panel-left"),
        option("workbench.action.alignPanelRight", "panelAlignment == 'right'", "Right", "layout-panel-right"),
        option("workbench.action.alignPanelCenter", "panelAlignment == 'center'", "Center", "layout-panel-center"),
        option("workbench.action.alignPanelJustify", "panelAlignment == 'justify'", "Justify", "layout-panel-justify"),
    ]),
    ("Quick Input Position", &[
        option("workbench.action.alignQuickInputTop", "quickInputAlignment == 'top'", "Top", "arrow-up"),
        option("workbench.action.alignQuickInputCenter", "quickInputAlignment == 'center'", "Center", "circle"),
    ]),
    ("Layout Density", &[
        option("workbench.action.setLayoutDensity.default", "config.window.density.layout == 'default'", "Default", "layout-density-default"),
        option("workbench.action.setLayoutDensity.compact", "config.window.density.layout == 'compact'", "Compact", "layout-density-compact"),
    ]),
    ("Modes", &[
        option("workbench.action.toggleFullScreen", "isFullscreen", "Full Screen", "screen-full"),
        option("workbench.action.toggleZenMode", "inZenMode", "Zen Mode", "target"),
        option("workbench.action.toggleCenteredLayout", "isCenteredLayout", "Centered Layout", "layout-centered"),
    ]),
];

/// The title bar buttons, by handle.
const RESET_BUTTON: i64 = 0;
const CLOSE_BUTTON: i64 = 1;

/// The open picker: the item last picked, kept active when the items change.
pub struct CustomizeLayoutPicker {
    selected: Option<i64>,
}

impl Workbench {
    pub(super) fn register_customize_layout_commands(&mut self) {
        self.commands.register("workbench.action.customizeLayout", |workbench, _, window, cx| {
            workbench.toggle_customize_layout(window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.alignQuickInputTop", |workbench, _, window, cx| {
            workbench.set_quick_input_alignment(false, window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.alignQuickInputCenter", |workbench, _, window, cx| {
            workbench.set_quick_input_alignment(true, window, cx);
            Outcome::done()
        });
        // sidebarPart.ts: hiding remembers the location in profile storage (`rememberActivityBarVisiblePosition`),
        // showing goes back to it.
        self.commands.register("workbench.action.toggleActivityBarVisibility", |workbench, _, _, cx| {
            let location = workbench.setting("workbench.activityBar.location");
            let next = match location.as_str() {
                Some("hidden") => {
                    let remembered = workbench.profile_storage.lock().unwrap().get("workbench.activityBar.location").map(str::to_owned);
                    match remembered.as_deref() {
                        Some(position @ ("top" | "bottom")) => position.to_owned(),
                        _ => "default".to_owned(),
                    }
                }
                _ => {
                    workbench.profile_storage.lock().unwrap().set("workbench.activityBar.location", location.as_str().unwrap_or("default").to_owned());
                    "hidden".to_owned()
                }
            };
            workbench.update_setting("workbench.activityBar.location", json!(next));
            cx.notify();
            Outcome::done()
        });
    }

    /// `run`: a second run while the picker shows hides it.
    fn toggle_customize_layout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.quick_inputs.find_id(|owner| matches!(owner, Owner::CustomizeLayout(_))) {
            return self.dispose_quick_input(id, window, cx);
        }
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.title = Some("Customize Layout".into());
        input.ignore_focus_out = true;
        input.hide_input = true;
        input.buttons = vec![
            Button { handle: RESET_BUTTON, codicon: Some("discard".into()), tooltip: Some("Restore Defaults".into()), always_visible: true, ..Button::default() },
            Button { handle: CLOSE_BUTTON, codicon: Some("close".into()), tooltip: Some("Close".into()), always_visible: true, ..Button::default() },
        ];
        input.set_items(self.customize_layout_entries(window));
        let id = self.quick_inputs.add(input, Owner::CustomizeLayout(CustomizeLayoutPicker { selected: None }));
        self.show_quick_input(id, window, cx);
        self.flush_quick_input_events(window, cx);
    }

    /// `getItems` against the current context keys.
    fn customize_layout_entries(&self, window: &Window) -> Vec<Entry> {
        let context = self.key_context(window);
        let holds = |expression: &str| Expr::deserialize(expression).is_some_and(|expression| expression.evaluate(&context));
        let mut entries = Vec::new();
        let mut handle = 0;
        for (section, items) in SECTIONS {
            if *section == "Layout Density" && !holds(MODERN_UI) {
                handle += items.len() as i64;
                continue;
            }
            entries.push(Entry::Separator(Some((*section).into())));
            for item in *items {
                let active = holds(item.active);
                let icon = match item.icon {
                    VisualIcon::Fixed(icon) => icon,
                    VisualIcon::BySideBar { left, right } => if holds(SIDE_BAR_LEFT) { left } else { right },
                };
                let (label, buttons) = match (item.toggle, active) {
                    (true, _) => {
                        let (eye, aria) = if active { ("eye", "Select to Hide") } else { ("eye-closed", "Select to Show") };
                        (format!("$({icon}) {}", item.label), vec![Button { handle: 0, codicon: Some(eye.into()), tooltip: Some(format!("{} ({aria})", item.label)), ..Button::default() }])
                    }
                    (false, true) => (format!("$({icon}) {} $(check)", item.label), Vec::new()),
                    (false, false) => (format!("$({icon}) {}", item.label), Vec::new()),
                };
                let keybinding = self.keybindings.resolver.lookup_primary_keybinding(item.command, &context);
                let keybinding = keybinding.map(|item| item.keybinding.iter().map(|chord| chord.key_caps()).collect()).unwrap_or_default();
                entries.push(Entry::Item(Item { handle, label, buttons, keybinding, ..Item::default() }));
                handle += 1;
            }
        }
        entries
    }

    /// `onDidChangeContext` for the layout keys: the items follow every change while the picker shows, the picked
    /// item staying active.
    pub(super) fn follow_customize_layout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.quick_inputs.find_id(|owner| matches!(owner, Owner::CustomizeLayout(_))) else { return };
        let entries = self.customize_layout_entries(window);
        if self.quick_inputs.get(id).is_some_and(|input| input.items() == entries.as_slice()) {
            return;
        }
        let Some(Owner::CustomizeLayout(picker)) = self.quick_inputs.owner(id) else { return };
        let selected = picker.selected;
        let input = self.quick_inputs.get_mut(id).expect("the picker is open");
        input.set_items(entries);
        if let Some(handle) = selected {
            input.set_active_items(&[handle]);
        }
        window.focus(&self.quick_inputs.focus, cx);
        self.flush_quick_input_events(window, cx);
    }

    pub fn customize_layout_event(&mut self, id: i64, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        let handle = match event {
            Event::Accept(_) => self.quick_inputs.get(id).and_then(|input| input.selected_handles().first().copied()),
            Event::TriggerItemButton(handle, _, _) => Some(handle),
            Event::TriggerButton(CLOSE_BUTTON, _) => return self.hide_quick_input_by_id(id, window, cx),
            Event::TriggerButton(RESET_BUTTON, _) => return self.reset_layout(window, cx),
            Event::Hide => return self.quick_inputs.remove(id),
            _ => None,
        };
        let Some(handle) = handle else { return };
        if let Some(Owner::CustomizeLayout(picker)) = self.quick_inputs.owner_mut(id) {
            picker.selected = Some(handle);
        }
        let command = SECTIONS.iter().flat_map(|(_, items)| items.iter()).nth(handle as usize).expect("an item of the picker").command;
        drop(self.execute_command(command, Vec::new(), window, cx));
        cx.notify();
    }

    /// The Restore Defaults button: the layout settings back to their defaults, the panel centered, the quick input
    /// on top.
    fn reset_layout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for key in ["workbench.activityBar.location", "workbench.sideBar.location", "workbench.statusBar.visible", "workbench.panel.defaultLocation", "window.density.layout"] {
            let mut settings = self.settings.lock().unwrap();
            let default = settings.configuration.inspect(key, &Default::default()).default_value;
            match settings.update_value_derived(key, default.as_ref()) {
                Ok(change) if !change.is_empty() => self.host.send_configuration_change(&settings, &change),
                Ok(_) => {}
                Err(error) => eprintln!("writing {key}: {error}"),
            }
        }
        drop(self.execute_command("workbench.action.alignPanelCenter", Vec::new(), window, cx));
        drop(self.execute_command("workbench.action.alignQuickInputTop", Vec::new(), window, cx));
        cx.notify();
    }
}
