//! The view containers extensions contribute (viewsExtensionPoint.ts `viewsContainers.activitybar`) and the tree
//! views of every container (viewsService.ts): the activity bar items of containers with an active view, their side
//! bar, the contributed views appended to the Source Control, Run and Debug and Testing side bars, and the
//! `<view id>.focus` and `workbench.view.extension.<id>` commands.
// Not ported yet: `viewsContainers.panel`, moving views between containers, hiding views.

use std::collections::HashMap;
use std::path::PathBuf;

use gpui::{AnyElement, ClickEvent, Context, Div, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder, px, svg};
use serde_json::{Value, json};

use super::Workbench;
use super::hover::HoverOptions;
use super::layout::Part;
use super::search_view::SideBarView;
use super::tree_view::static_id;
use super::view_panes::{FILE_VIEW, HEADER_SIZE, OUTLINE_VIEW, PaneContainer, TIMELINE_VIEW};
use crate::commands::Outcome;

pub(super) struct ViewContainer {
    /// `workbench.view.extension.<contributed id>`.
    pub id: &'static str,
    /// The contributed id, the `contributes.views` key of its views.
    pub key: String,
    pub title: String,
    pub icon: ContainerIcon,
}

pub(super) enum ContainerIcon {
    Codicon(String),
    File(PathBuf),
}

/// `registerCustomViewContainers` for the activity bar, in extension order.
pub(super) fn contributed_view_containers(extensions: &[exthost::scan::ScannedExtension]) -> Vec<ViewContainer> {
    let mut containers = Vec::new();
    for extension in extensions {
        for descriptor in extension.description["contributes"]["viewsContainers"]["activitybar"].as_array().into_iter().flatten() {
            let (Some(key), Some(title), Some(icon)) = (descriptor["id"].as_str(), descriptor["title"].as_str(), descriptor["icon"].as_str()) else { continue };
            let icon = match icon.strip_prefix("$(").and_then(|name| name.strip_suffix(')')) {
                Some(name) => ContainerIcon::Codicon(name.to_owned()),
                None => ContainerIcon::File(extension.location.join(icon)),
            };
            containers.push(ViewContainer { id: static_id(&format!("workbench.view.extension.{key}")), key: key.to_owned(), title: title.to_owned(), icon });
        }
    }
    containers
}

impl Workbench {
    pub(super) fn register_view_container_commands(&mut self) {
        for container in &self.tree_views.containers {
            let id = container.id;
            if self.commands.get(id).is_none() {
                self.commands.register(id, move |workbench, _, window, cx| {
                    workbench.open_view_container(id, window, cx);
                    Outcome::done()
                });
            }
        }
        for view in &self.tree_views.views {
            let id = view.id;
            let command = format!("{id}.focus");
            if self.commands.get(&command).is_none() {
                self.commands.register(command, move |workbench, _, window, cx| {
                    workbench.open_tree_view(id, true, window, cx);
                    Outcome::done()
                });
            }
        }
    }

    /// viewsService.ts `registerFocusViewAction` of the built-in views: `<view id>.focus` runs `openView(id, true)`.
    /// The panel's views register theirs in `register_panel_view`, the contributed ones above.
    pub(super) fn register_view_focus_commands(&mut self) {
        for id in [FILE_VIEW, OUTLINE_VIEW, TIMELINE_VIEW] {
            self.commands.register(format!("{id}.focus"), move |workbench, _, window, cx| {
                if !workbench.open_moved_view(id, cx) {
                    drop(workbench.execute_command("workbench.view.explorer", Vec::new(), window, cx));
                    workbench.expand_pane(PaneContainer::Explorer, id, cx);
                }
                let focus = match id {
                    OUTLINE_VIEW => workbench.outline_pane.focus.clone(),
                    TIMELINE_VIEW => workbench.timeline.focus.clone(),
                    _ => workbench.explorer.focus.clone(),
                };
                window.focus(&focus, cx);
                Outcome::done()
            });
        }
        let containers = [
            ("workbench.view.search", "workbench.view.search"),
            ("workbench.scm", "workbench.view.scm"),
            ("workbench.scm.repositories", "workbench.view.scm"),
            ("workbench.scm.history", "workbench.view.scm"),
            ("workbench.debug.welcome", "workbench.view.debug"),
            ("workbench.view.testing", "workbench.view.testing"),
        ];
        for (view, container) in containers {
            self.commands.register(format!("{view}.focus"), move |workbench, _, window, cx| match workbench.open_moved_view(view, cx) {
                true => Outcome::done(),
                false => workbench.execute_command(container, Vec::new(), window, cx),
            });
        }
        // markers.contribution.ts: "Focus Problems (Errors, Warnings, Infos)" opens the Problems view focused.
        self.commands.register("workbench.action.problems.focus", |workbench, _, window, cx| workbench.execute_command("workbench.panel.markers.view.focus", Vec::new(), window, cx));
        for view in super::extensions_view::VIEW_IDS {
            self.commands.register(format!("{view}.focus"), move |workbench, _, window, cx| {
                workbench.open_extensions_viewlet(window, cx);
                if workbench.extensions_viewlet.panes.iter().any(|pane| pane.descriptor.id == view) {
                    window.focus(&workbench.extensions_viewlet.list_focus.clone(), cx);
                }
                Outcome::done()
            });
        }
    }

    /// `openViewContainer(id, true)` of a contributed container.
    fn open_view_container(&mut self, id: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        self.side_bar_view = SideBarView::Contributed(id);
        self.set_part_hidden(Part::SideBar, false, cx);
        if let Some(first) = self.container_views(id).first().copied() {
            self.focus_tree_view(first, window, cx);
        }
        self.sync_tree_views(cx);
        cx.notify();
    }

    /// `viewsService.openView(id, focus)`: shows the view's container and expands the view.
    pub(super) fn open_tree_view(&mut self, id: &str, focus: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.tree_views.views.iter().find(|view| view.id == id) else { return };
        let (id, container) = (view.id, view.container.clone());
        if self.open_moved_view(id, cx) {
            if focus {
                self.focus_tree_view(id, window, cx);
            }
            self.sync_tree_views(cx);
            return;
        }
        match container.as_str() {
            "explorer" => {
                drop(self.execute_command("workbench.view.explorer", Vec::new(), window, cx));
                self.expand_pane(PaneContainer::Explorer, id, cx);
            }
            "scm" => {
                drop(self.execute_command("workbench.view.scm", Vec::new(), window, cx));
                self.expand_pane(PaneContainer::Scm, id, cx);
            }
            "debug" => {
                drop(self.execute_command("workbench.view.debug", Vec::new(), window, cx));
                self.expand_pane(PaneContainer::Debug, id, cx);
            }
            "test" => drop(self.execute_command("workbench.view.testing", Vec::new(), window, cx)),
            key => {
                let Some(container) = self.tree_views.containers.iter().find(|container| container.key == key) else { return };
                self.side_bar_view = SideBarView::Contributed(container.id);
                self.set_part_hidden(Part::SideBar, false, cx);
            }
        }
        self.tree_views.collapsed.remove(id);
        if focus {
            self.focus_tree_view(id, window, cx);
        }
        self.sync_tree_views(cx);
        cx.notify();
    }

    fn focus_tree_view(&mut self, id: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let focus = self.tree_views.focus.entry(id.to_owned()).or_insert_with(|| cx.focus_handle()).clone();
        window.focus(&focus, cx);
    }

    /// The shown views of a contributed container, by its `workbench.view.extension.<id>` id.
    pub(super) fn container_views(&self, id: &str) -> Vec<&'static str> {
        let Some(container) = self.tree_views.containers.iter().find(|container| container.id == id) else { return Vec::new() };
        self.shown_views_of(&container.key).into_iter().filter(|view| self.stays_in(view, id)).collect()
    }

    /// Whether a contributed view's tree shows: its container in the visible side bar and the view expanded.
    pub(super) fn tree_view_body_visible(&self, id: &'static str) -> bool {
        let Some(view) = self.tree_views.views.iter().find(|view| view.id == id) else { return false };
        if self.movable_view(id).is_some_and(|movable| self.view_container(&movable) != movable.default_container) {
            return self.is_view_visible(id);
        }
        if !self.is_part_visible(Part::SideBar) || !self.shown_views_of(&view.container).contains(&id) {
            return false;
        }
        let shown_container = match view.container.as_str() {
            "explorer" => return self.side_bar_view == SideBarView::Explorer && self.view_pane_expanded(PaneContainer::Explorer, id),
            "scm" => return self.side_bar_view == SideBarView::Scm && self.view_pane_expanded(PaneContainer::Scm, id),
            "debug" => return self.side_bar_view == SideBarView::Debug && self.view_pane_expanded(PaneContainer::Debug, id),
            "test" => SideBarView::Testing,
            key => match self.tree_views.containers.iter().find(|container| container.key == key) {
                Some(container) => SideBarView::Contributed(container.id),
                None => return false,
            },
        };
        self.side_bar_view == shown_container && !self.tree_views.collapsed.contains(id)
    }

    /// viewDescriptorService.ts `<view id>.active` of the contributed views.
    pub(super) fn view_containers_context(&self, context: &mut HashMap<String, Value>) {
        for view in &self.tree_views.views {
            if view.when.as_ref().is_none_or(|when| when.evaluate(&self.context_keys)) {
                context.insert(format!("{}.active", view.id), json!(true));
            }
        }
    }

    /// The activity bar items of contributed containers with an active view.
    pub(super) fn contributed_activity_items(&self, hover: HoverOptions, inactive: gpui::Rgba, cx: &mut Context<Self>) -> Vec<gpui::Stateful<Div>> {
        let mut items = Vec::new();
        for container in &self.tree_views.containers {
            if self.shown_views_of(&container.key).is_empty() {
                continue;
            }
            let id = container.id;
            let active = self.side_bar_view == SideBarView::Contributed(id) && self.is_part_visible(Part::SideBar);
            let icon = match (&container.icon, active) {
                (ContainerIcon::Codicon(name), true) => self.active_view_icon(name),
                (ContainerIcon::Codicon(name), false) => self.icon(name, 36., 24., inactive),
                (ContainerIcon::File(path), _) => {
                    let color = if active { self.color("modernActivityBarItem.activeForeground") } else { inactive };
                    div().w(px(36.)).h(px(36.)).flex().items_center().justify_center().child(svg().external_path(path.to_string_lossy().into_owned()).size(px(24.)).text_color(color))
                }
            };
            let title: &'static str = static_id(&container.title);
            items.push(icon.id(id).relative().children(self.container_activity_badge(id)).cursor_pointer().child(self.action_hover(title, id, hover)).on_click(cx.listener(move |workbench, event: &ClickEvent, window, cx| workbench.activity_item_clicked(active, id, event, window, cx))));
        }
        items
    }

    /// A contributed container's side bar: with one view the view merged into the container (no header, the title
    /// "<container>: <view>"), else a section per view.
    pub(super) fn contributed_side_bar(&self, id: &'static str, window: &Window, cx: &mut Context<Self>) -> Div {
        let foreground = self.foreground_or("sideBar.foreground");
        let container_title = self.tree_views.containers.iter().find(|container| container.id == id).map(|container| container.title.clone()).unwrap_or_default();
        let views = self.container_views(id);
        let frame = self.side_bar_frame().text_color(foreground);
        if let [view] = views.as_slice() {
            let pane = self.tree_view_pane(view, window, cx);
            let title = if pane.title == container_title || pane.title.is_empty() { container_title } else { format!("{container_title}: {}", pane.title) };
            let title = self.side_bar_title_text(title).children(pane.actions.map(|actions| div().flex_none().pl(px(8.)).child(actions)));
            return frame.child(title).child(div().flex_1().min_h_0().flex().flex_col().child(pane.body));
        }
        frame.child(self.side_bar_title_text(container_title)).child(div().flex_1().min_h_0().flex().flex_col().children(self.contributed_view_sections(&views, window, cx)))
    }

    /// The contributed views of Testing as sections below its own view.
    pub(super) fn testing_contributed_sections(&self, window: &Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let views: Vec<_> = self.shown_views_of("test").into_iter().filter(|view| self.stays_in(view, super::view_locations::TESTING_CONTAINER)).collect();
        self.contributed_view_sections(&views, window, cx)
    }

    /// paneview.ts panes: a 28px header (chevron, title, description, actions while expanded) over the body.
    fn contributed_view_sections(&self, views: &[&'static str], window: &Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let foreground = self.foreground_or("sideBarSectionHeader.foreground");
        let border = self.theme.color("sideBarSectionHeader.border");
        let mut sections = Vec::new();
        for &id in views {
            let pane = self.tree_view_pane(id, window, cx);
            let expanded = !self.tree_views.collapsed.contains(id);
            let header = div()
                .id(gpui::SharedString::from(format!("view-header-{id}")))
                .h(px(HEADER_SIZE))
                .flex_none()
                .flex()
                .items_center()
                .cursor_pointer()
                .text_color(foreground)
                .when_some(self.theme.color("sideBarSectionHeader.background"), |header, background| header.bg(background))
                .when_some(border, |header, border| header.border_t_1().border_color(border))
                .child(self.icon(if expanded { "chevron-down" } else { "chevron-right" }, 20., 16., foreground))
                .child(div().text_size(px(11.)).font_weight(gpui::FontWeight::BOLD).child(pane.title.to_uppercase()))
                .children(pane.description.map(|description| div().ml(px(4.)).text_size(px(11.)).opacity(0.6).truncate().child(description)))
                .child(div().ml_auto().when(expanded, |actions| actions.children(pane.actions)))
                .on_click(cx.listener(move |workbench, _, _, cx| {
                    if !workbench.tree_views.collapsed.remove(id) {
                        workbench.tree_views.collapsed.insert(id);
                    }
                    workbench.sync_tree_views(cx);
                    cx.notify();
                }));
            sections.push(div().flex().flex_col().when(expanded, |section| section.flex_1().min_h_0()).child(header).when(expanded, |section| section.child(div().flex_1().min_h_0().flex().flex_col().child(pane.body))).into_any_element());
        }
        sections
    }
}

#[cfg(test)]
mod tests;
