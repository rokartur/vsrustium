//! The Source Control view (scmViewPane.ts with scmInput.ts and activity.ts): the `MainThreadSCM` requests, the
//! repository's input box and action button, the resource groups with their resources and inline actions, the
//! `scm/title` actions, the activity bar badge and the status bar entries of the active repository.

use std::collections::{HashMap, HashSet};

use gpui::{Bounds, ClickEvent, Context, Div, FocusHandle, MouseButton, MouseDownEvent, Rgba, Window, div, img, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use super::hover::{HoverContent, HoverOptions};
use super::input_box_view::{InputId, InputLook, InputView};
use crate::commands::Outcome;
use crate::editor::TextDocument;
use crate::input_box::InputBox;
use crate::main_thread_status_bar::{StatusEntry, set_or_update_entry};
use crate::menus::{CommandIcon, MenuId};
use crate::scm::{Group, Provider, Resource, Scm, ScmChange};
use crate::workbench::host_requests::uri_parts;

/// scm.css: list rows are 22px.
const ROW_HEIGHT: f32 = 22.;
/// The tree's indent per level (`workbench.tree.indent` 8) plus the twistie (16).
const INDENT: f32 = 8.;
const TWISTIE: f32 = 16.;

/// The view's state besides the model.
pub struct ScmView {
    pub model: Scm,
    pub input: InputBox,
    pub input_view: InputView,
    pub input_focus: FocusHandle,
    pub list_focus: FocusHandle,
    /// The input box models of the source controls (`inputBoxDocumentUri`), by provider handle.
    input_models: HashMap<u64, (String, std::path::PathBuf)>,
    /// Collapsed groups, by provider and group handle.
    collapsed: HashSet<(u64, u64)>,
    /// The selected row: provider, group, resource handle (`None` for the group row).
    pub selected: Option<(u64, u64, Option<u64>)>,
    /// The handle the input box shows, so a provider switch reloads it.
    shown_input: Option<u64>,
}

impl ScmView {
    pub fn new(cx: &mut gpui::App) -> Self {
        ScmView {
            model: Scm::default(),
            input: InputBox::default(),
            input_view: InputView::default(),
            input_focus: cx.focus_handle(),
            list_focus: cx.focus_handle(),
            input_models: HashMap::new(),
            collapsed: HashSet::new(),
            selected: None,
            shown_input: None,
        }
    }
}

impl Workbench {
    // shortcut: the SCM Changes and webview view headers are drawn here, not as view panes of view_panes.rs; milestone 11 moves them.
    pub(super) fn section_header(&self, title: &str, expanded: bool, separated: bool) -> Div {
        let foreground = self.foreground_or("sideBarSectionHeader.foreground");
        let border = self.theme.color("sideBarSectionHeader.border").or(self.theme.color("surface.border")).filter(|_| separated);
        div()
            .relative()
            .h(px(28.))
            // paneview.css: .pane-header is cursor: pointer.
            .cursor_pointer()
            .flex_none()
            .mx(px(4.))
            .pl(px(4.))
            .flex()
            .items_center()
            .rounded(px(4.))
            .bg(self.color("sideBar.background"))
            .hover(|header| header.bg(self.color("list.hoverBackground")))
            .text_color(foreground)
            .when_some(border, |header, border| header.child(div().absolute().top_0().left(px(4.)).right(px(4.)).h(px(1.)).bg(border)))
            .child(self.icon(if expanded { "chevron-down" } else { "chevron-right" }, 16., 16., foreground).mx(px(2.)))
            .child(div().text_size(px(12.)).font_weight(gpui::FontWeight::SEMIBOLD).child(title.to_string()))
    }

    pub(in crate::workbench) fn register_scm_commands(&mut self) {
        self.commands.register("workbench.view.scm", |workbench, _, window, cx| {
            workbench.side_bar_view = super::search_view::SideBarView::Scm;
            workbench.focus_scm_input(window, cx);
            cx.notify();
            Outcome::done()
        });
        self.commands.register("scm.acceptInput", |workbench, _, window, cx| workbench.accept_scm_input(window, cx));
        self.commands.register("scm.clearInput", |workbench, _, window, cx| {
            workbench.edit_scm_input(|input| input.set_value(String::new()), window, cx);
            Outcome::done()
        });
        self.commands.register("scm.clearValidation", |workbench, _, _, cx| {
            if let Some(provider) = workbench.active_scm_provider_mut() {
                provider.input.validation = None;
            }
            cx.notify();
            Outcome::done()
        });
    }

    /// One `MainThreadSCM` request.
    pub(crate) fn handle_scm_request(&mut self, method: &str, args: &[Value], window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let change = self.scm.model.apply(method, args);
        if !method.starts_with("$setInput") && !method.starts_with("$showValidation") {
            self.quick_diffs.generation += 1;
        }
        match change {
            ScmChange::None => return Outcome::done(),
            ScmChange::Registered(handle) => self.scm_registered(handle, args.get(7).cloned().unwrap_or(Value::Null), cx),
            ScmChange::Changed => {}
        }
        let handle = args.first().and_then(Value::as_u64);
        if method == "$setInputBoxValue"
            && let Some(handle) = handle
        {
            let value = self.scm.model.provider(handle).map(|provider| provider.input.value.clone()).unwrap_or_default();
            self.sync_scm_input_model(handle, &value, cx);
            if self.active_scm_provider().map(|provider| provider.handle) == Some(handle) {
                self.scm.input.set_value(value);
            }
        }
        if method == "$unregisterSourceControl"
            && let Some(handle) = handle
            && let Some((scheme, path)) = self.scm.input_models.remove(&handle)
            && let Some(index) = self.editors.models.iter().position(|model| model.document.is(&scheme, &path))
        {
            self.editors.models.remove(index);
            self.editors_changed(cx);
        }
        self.load_scm_input();
        self.refresh_scm_status_entries();
        let _ = window;
        cx.notify();
        Outcome::done()
    }

    /// `$registerSourceControl`: the input box's text model (`createModelReference` of `inputBoxDocumentUri`, which
    /// the host then sees as a document) and the selection the host hears of for the first repository.
    fn scm_registered(&mut self, handle: u64, input_uri: Value, cx: &mut Context<Self>) {
        if let Ok((scheme, path)) = uri_parts(&input_uri) {
            let document = TextDocument::from_text(scheme, &path, "", "utf8".into(), Some("scminput"), &self.languages, &self.settings.lock().unwrap());
            self.editors.open_model(document);
            self.scm.input_models.insert(handle, (scheme.to_string(), path));
            self.editors_changed(cx);
        }
        // mainThreadSCM.ts: the focused repository (the first one) tells the host it is selected.
        if self.scm.model.providers.first().map(|provider| provider.handle) == Some(handle) {
            self.host.send("ExtHostSCM", "$setSelectedSourceControl", vec![json!(handle)]);
        }
    }

    /// scmViewService.ts `activeRepository`: the focused repository, the first one here.
    // shortcut: one repository is shown and active; the Repositories view and its selection come with multi-root git.
    pub(in crate::workbench) fn active_scm_provider(&self) -> Option<&Provider> {
        self.scm.model.providers.first()
    }

    fn active_scm_provider_mut(&mut self) -> Option<&mut Provider> {
        self.scm.model.providers.first_mut()
    }

    /// activity.ts `_updateStatusBar`: the active repository's `statusBarCommands` as `status.scm.<index>` entries.
    fn refresh_scm_status_entries(&mut self) {
        self.status_entries.retain(|entry| !entry.entry_id.starts_with("status.scm."));
        let Some(provider) = self.active_scm_provider() else { return };
        let root = provider.root_uri["path"].as_str().map(|path| path.rsplit('/').next().unwrap_or_default().to_string());
        let label = match root {
            Some(root) => format!("{root} ({})", provider.label),
            None => provider.label.clone(),
        };
        let entries: Vec<StatusEntry> = provider
            .status_bar_commands
            .iter()
            .enumerate()
            .map(|(index, command)| {
                let tooltip = match command["tooltip"].as_str() {
                    Some(tooltip) => format!("{label} - {tooltip}"),
                    None => label.clone(),
                };
                StatusEntry {
                    entry_id: format!("status.scm.{index}"),
                    id: format!("status.scm.{index}"),
                    name: "Source Control".into(),
                    text: command["title"].as_str().unwrap_or_default().to_string(),
                    tooltip: Some(tooltip),
                    command: command["id"].as_str().map(|id| json!({ "id": id, "arguments": command["arguments"] })),
                    color: None,
                    background_color: None,
                    kind: None,
                    align_left: true,
                    // The first entry at 10000, each next one right after the one before it (compact).
                    priority: 10000. - index as f64 * 1e-6,
                    secondary: 0,
                    show_progress: false,
                }
            })
            .collect();
        for entry in entries {
            set_or_update_entry(&mut self.status_entries, entry);
        }
    }

    /// activity.ts: the badge of the Source Control view, `scm.countBadge` `all` (default), `focused` or `off`.
    pub(in crate::workbench) fn scm_badge(&self) -> u64 {
        match self.setting("scm.countBadge").as_str().unwrap_or("all") {
            "off" => 0,
            "focused" => self.active_scm_provider().map_or(0, Provider::count),
            _ => self.scm.model.providers.iter().map(Provider::count).sum(),
        }
    }

    fn focus_scm_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focus = self.scm.input_focus.clone();
        window.focus(&focus, cx);
        self.blink_caret(InputId::ScmInput, window, cx);
    }

    /// scmInput.ts: an edit of the input box sets the repository's input, which the host hears.
    pub(in crate::workbench) fn edit_scm_input(&mut self, edit: impl FnOnce(&mut InputBox), window: &mut Window, cx: &mut Context<Self>) {
        edit(&mut self.scm.input);
        let value = self.scm.input.value.clone();
        let _ = window;
        let Some(provider) = self.active_scm_provider_mut() else { return };
        if provider.input.value == value {
            return;
        }
        provider.input.value = value.clone();
        provider.input.validation = None;
        let handle = provider.handle;
        self.host.send("ExtHostSCM", "$onInputBoxValueChange", vec![json!(handle), json!(value)]);
        self.sync_scm_input_model(handle, &value, cx);
        cx.notify();
    }

    fn sync_scm_input_model(&mut self, handle: u64, value: &str, cx: &mut Context<Self>) {
        let Some((scheme, path)) = self.scm.input_models.get(&handle).cloned() else { return };
        self.replace_model_text(&scheme, &path, value.to_string(), true, cx);
    }

    /// `scm.acceptInput`: the provider's `acceptInputCommand` with its arguments.
    fn accept_scm_input(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let Some(command) = self.active_scm_provider().and_then(|provider| provider.accept_input_command.clone()) else { return Outcome::done() };
        self.run_dto_command(&command, window, cx)
    }

    /// Runs an `ICommandDto` (`{ id, arguments }`).
    fn run_dto_command(&mut self, command: &Value, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let Some(id) = command["id"].as_str().map(str::to_string) else { return Outcome::done() };
        let args = command["arguments"].as_array().cloned().unwrap_or_default();
        self.execute_command(&id, args, window, cx)
    }

    /// The context keys of a resource or group row (`scmProvider`, `scmProviderRootUri`, `scmProviderHasRootUri`,
    /// `scmResourceGroup`, `scmResourceState`), on top of the workbench's.
    fn scm_context(&self, provider: &Provider, group: Option<&Group>, resource: Option<&Resource>, window: &Window) -> HashMap<String, Value> {
        let mut context = self.key_context(window);
        context.insert("scmProvider".into(), json!(provider.id));
        context.insert("scmProviderRootUri".into(), json!(provider.root_uri["path"]));
        context.insert("scmProviderHasRootUri".into(), json!(!provider.root_uri.is_null()));
        if let Some(group) = group {
            context.insert("scmResourceGroup".into(), json!(group.id));
            context.insert("scmResourceGroupState".into(), json!(group.context_value.clone().unwrap_or_default()));
        }
        if let Some(resource) = resource {
            context.insert("scmResourceState".into(), json!(resource.context_value.clone().unwrap_or_default()));
        }
        context
    }

    /// The key context while the view has focus: the repository and the input's state.
    pub(in crate::workbench) fn scm_key_context(&self, window: &Window, context: &mut HashMap<String, Value>) {
        let Some(provider) = self.active_scm_provider() else { return };
        context.insert("scm.providerCount".into(), json!(self.scm.model.providers.len()));
        if self.scm.input_focus.is_focused(window) {
            context.insert("scmRepository".into(), json!(true));
            context.insert("scmInputHasValidationMessage".into(), json!(provider.input.validation.is_some()));
        }
        if self.side_bar_view == super::search_view::SideBarView::Scm {
            context.insert("activeViewlet".into(), json!("workbench.view.scm"));
        }
    }

    /// The `ISCMResource` argument a resource menu action gets, as `MainThreadSCMResource.toJSON`.
    fn resource_arg(provider: &Provider, group: &Group, resource: &Resource) -> Value {
        json!({ "$mid": 3, "sourceControlHandle": provider.handle, "groupHandle": group.handle, "handle": resource.handle })
    }

    fn group_arg(provider: &Provider, group: &Group) -> Value {
        json!({ "$mid": 4, "sourceControlHandle": provider.handle, "groupHandle": group.handle })
    }

    fn provider_arg(provider: &Provider) -> Value {
        json!({ "$mid": 5, "handle": provider.handle })
    }

    /// The Source Control view: the title, then the Changes pane with the active repository.
    pub(in crate::workbench) fn scm_side_bar(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let foreground = self.foreground_or("sideBar.foreground");
        let title = self.side_bar_title("Source Control").child(self.icon("ellipsis", 22., 16., foreground).cursor_pointer());
        let frame = self.side_bar_frame().text_color(foreground).child(title);
        let Some(provider) = self.active_scm_provider().cloned() else {
            // scmViewPane.ts: no repository shows the welcome content of `views.welcome` (git's "open a folder"
            // messages need git's context keys; the empty pane stands for them).
            return frame.child(self.section_header("Changes", true, false));
        };
        let context = self.scm_context(&provider, None, None, window);
        let title_actions = self.inline_actions(MenuId::ScmTitle, &context, vec![Self::provider_arg(&provider)], foreground, cx);
        let header = self.section_header("Changes", true, false).child(div().ml_auto().mr(px(4.)).flex().child(title_actions));
        let mut rows: Vec<gpui::AnyElement> = Vec::new();
        if provider.input.visible {
            rows.push(self.scm_input_row(&provider, window, cx).into_any_element());
        }
        if let Some(button) = provider.action_button.clone().filter(|button| button["command"].is_object()) {
            rows.push(self.scm_action_button(&button, cx).into_any_element());
        }
        for group in provider.groups.iter().filter(|group| !(group.hide_when_empty && group.resources.is_empty())) {
            rows.push(self.scm_group_row(&provider, group, window, cx).into_any_element());
            if self.scm.collapsed.contains(&(provider.handle, group.handle)) {
                continue;
            }
            for resource in &group.resources {
                rows.push(self.scm_resource_row(&provider, group, resource, window, cx).into_any_element());
            }
        }
        frame.child(header).child(div().id("scm-list").flex_1().min_h_0().overflow_y_scroll().track_focus(&self.scm.list_focus).children(rows))
    }

    /// The input box shows the active repository's input once, then keeps its own state while edited.
    pub(in crate::workbench) fn load_scm_input(&mut self) {
        let shown = self.active_scm_provider().map(|provider| (provider.handle, provider.input.value.clone()));
        if shown.as_ref().map(|shown| shown.0) != self.scm.shown_input {
            self.scm.shown_input = shown.as_ref().map(|shown| shown.0);
            self.scm.input.set_value(shown.map(|shown| shown.1).unwrap_or_default());
        }
    }

    /// scmViewPane.ts `InputRenderer`: the input editor with its placeholder (`{0}` the commit key), 2px border radius
    /// and `input.*` colors, and the validation message under it.
    fn scm_input_row(&self, provider: &Provider, window: &Window, cx: &mut Context<Self>) -> Div {
        let focused = self.scm.input_focus.is_focused(window);
        // scmViewPane.ts `getPlaceholderValue`: `{0}` is the Cmd+Enter label, the provider's placeholder else "Message ({0} to commit)".
        let placeholder = provider.input.placeholder.replace("{0}", "\u{2318}Enter");
        let border = match (focused, &provider.input.validation) {
            (_, Some((_, 0))) => self.theme.color("inputValidation.errorBorder"),
            (_, Some((_, 1))) => self.theme.color("inputValidation.warningBorder"),
            (_, Some(_)) => self.theme.color("inputValidation.infoBorder"),
            (true, None) => self.theme.color("focusBorder"),
            (false, None) => self.theme.color("input.border"),
        };
        let look = InputLook { password: false, placeholder: Some(placeholder), focused };
        let input = self.input_text(InputId::ScmInput, look, cx);
        let field = div()
            .h(px(26.))
            .px(px(6.))
            .flex()
            .items_center()
            .rounded(px(2.))
            .bg(self.color("input.background"))
            .text_color(self.color("input.foreground"))
            .border_1()
            .border_color(border.unwrap_or(gpui::transparent_black().into()))
            .cursor_text()
            .child(input);
        let validation = provider.input.validation.clone().filter(|_| focused).map(|(message, kind)| {
            let key = ["error", "warning", "info"][kind.min(2) as usize];
            div()
                .px(px(6.))
                .py(px(4.))
                .text_size(px(12.))
                .bg(self.color(&format!("inputValidation.{key}Background")))
                .text_color(self.foreground_or(&format!("inputValidation.{key}Foreground")))
                .child(message)
        });
        div().pl(px(INDENT + TWISTIE)).pr(px(12.)).pt(px(4.)).pb(px(4.)).child(field).children(validation)
    }

    /// scmViewPane.ts `ActionButtonRenderer`: the button with its title (`$(icon)` labels) and, with secondary
    /// commands, the dropdown part after a separator.
    fn scm_action_button(&self, button: &Value, cx: &mut Context<Self>) -> Div {
        let enabled = button["enabled"] != false;
        let foreground = self.color("button.foreground");
        let background = self.color("button.background");
        let command = button["command"].clone();
        let title = command["title"].as_str().unwrap_or_default().to_string();
        let label = self.highlighted_label(&title, &[], foreground, 16., foreground);
        let main = div()
            .id("scm-action-button")
            .flex_1()
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .when(enabled, |main| main.cursor_pointer().hover(|main| main.bg(self.color("button.hoverBackground"))))
            .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                if enabled {
                    drop(workbench.run_dto_command(&command, window, cx));
                }
            }))
            .child(label);
        let secondary = button["secondaryCommands"].as_array().is_some_and(|commands| !commands.is_empty());
        let dropdown = secondary.then(|| {
            div()
                .flex()
                .h_full()
                .items_center()
                .child(div().w(px(1.)).h(px(16.)).bg(self.theme.color("button.separator").unwrap_or(foreground)))
                .child(self.icon("chevron-down", 26., 16., foreground).cursor_pointer())
        });
        div()
            .pl(px(INDENT + TWISTIE))
            .pr(px(12.))
            .pb(px(4.))
            .child(div().h(px(26.)).flex().rounded(px(2.)).bg(background).text_color(foreground).when(!enabled, |button| button.opacity(0.4)).child(main).children(dropdown))
    }

    fn scm_group_row(&self, provider: &Provider, group: &Group, window: &Window, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let foreground = self.foreground_or("sideBar.foreground");
        let collapsed = self.scm.collapsed.contains(&(provider.handle, group.handle));
        let context = self.scm_context(provider, Some(group), None, window);
        let actions = self.inline_actions(MenuId::ScmResourceGroupContext, &context, vec![Self::group_arg(provider, group)], foreground, cx);
        let key = (provider.handle, group.handle);
        let selected = self.scm.selected == Some((provider.handle, group.handle, None));
        let badge = div()
            .ml(px(4.))
            .min_w(px(18.))
            .h(px(18.))
            .px(px(5.))
            .rounded(px(11.))
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(11.))
            .bg(self.color("badge.background"))
            .text_color(self.color("badge.foreground"))
            .child(group.resources.len().to_string());
        let id = gpui::SharedString::from(format!("scm-group-{}-{}", provider.handle, group.handle));
        let menu_context = context.clone();
        let menu_args = vec![Self::group_arg(provider, group)];
        div()
            .id(id.clone())
            .group(id.clone())
            .h(px(ROW_HEIGHT))
            .pl(px(INDENT))
            .pr(px(12.))
            .flex()
            .items_center()
            .cursor_pointer()
            .when(selected, |row| row.bg(self.color("list.inactiveSelectionBackground")))
            .hover(|row| row.bg(self.color("list.hoverBackground")))
            .on_click(cx.listener(move |workbench, _: &ClickEvent, _, cx| {
                workbench.scm.selected = Some((key.0, key.1, None));
                if !workbench.scm.collapsed.remove(&key) {
                    workbench.scm.collapsed.insert(key);
                }
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| {
                workbench.show_context_menu(MenuId::ScmResourceGroupContext, &menu_context, Bounds::new(event.position, Default::default()), menu_args.clone(), window, cx);
            }))
            .child(self.icon(if collapsed { "chevron-right" } else { "chevron-down" }, TWISTIE, 16., foreground))
            .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().child(group.label.clone()))
            .child(div().invisible().group_hover(id, |actions| actions.visible()).child(actions))
            .child(badge)
    }

    /// scmViewPane.ts `ResourceRenderer`: file icon, name, the folder as description, inline actions on hover and the
    /// decoration letter from the resource's tooltip and color (git's `letter` via its decoration provider).
    fn scm_resource_row(&self, provider: &Provider, group: &Group, resource: &Resource, window: &Window, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let foreground = self.foreground_or("sideBar.foreground");
        let path = std::path::PathBuf::from(resource.uri["path"].as_str().unwrap_or_default());
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let root = provider.root_uri["path"].as_str().unwrap_or_default();
        let folder = path.parent().and_then(|parent| parent.strip_prefix(root).ok()).map(|folder| folder.to_string_lossy().into_owned()).unwrap_or_default();
        let context = self.scm_context(provider, Some(group), Some(resource), window);
        let args = vec![Self::resource_arg(provider, group, resource)];
        let actions = self.inline_actions(MenuId::ScmResourceStateContext, &context, args.clone(), foreground, cx);
        let decoration = self.scm_decoration(resource);
        let language = self.languages.language_of(&path, "").to_string();
        let icon = self.file_icons.as_ref().and_then(|icons| icons.file_icon(&path, &language, self.theme.kind, 13., ROW_HEIGHT, foreground)).unwrap_or_else(|| div().w(px(16.)));
        let key = (provider.handle, group.handle, Some(resource.handle));
        let selected = self.scm.selected == Some(key);
        let id = gpui::SharedString::from(format!("scm-resource-{}-{}-{}", provider.handle, group.handle, resource.handle));
        let label_color = decoration.as_ref().map_or(foreground, |decoration| decoration.1);
        let handles = (provider.handle, group.handle, resource.handle);
        div()
            .id(id.clone())
            .group(id.clone())
            .h(px(ROW_HEIGHT))
            .pl(px(INDENT * 2. + TWISTIE))
            .pr(px(12.))
            .flex()
            .items_center()
            .cursor_pointer()
            .when(resource.faded, |row| row.opacity(0.7))
            .when(selected, |row| row.bg(self.color("list.inactiveSelectionBackground")))
            .hover(|row| row.bg(self.color("list.hoverBackground")))
            .on_click(cx.listener(move |workbench, event: &ClickEvent, _, cx| {
                workbench.scm.selected = Some(key);
                // `ResourceTree.open`: a single click previews with the focus kept, a double click pins and focuses.
                let preserve_focus = event.click_count() < 2;
                drop(workbench.host.call("ExtHostSCM", "$executeResourceCommand", vec![json!(handles.0), json!(handles.1), json!(handles.2), json!(preserve_focus)]));
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| {
                workbench.show_context_menu(MenuId::ScmResourceStateContext, &context, Bounds::new(event.position, Default::default()), args.clone(), window, cx);
            }))
            .child(icon.mr(px(6.)))
            .child(div().flex_none().whitespace_nowrap().text_color(label_color).when(resource.strike_through, |name| name.line_through()).child(name))
            .child(div().flex_1().min_w_0().ml(px(6.)).overflow_hidden().whitespace_nowrap().text_size(px(12.)).opacity(0.7).child(folder))
            .child(div().invisible().group_hover(id, |actions| actions.visible()).child(actions))
            .children(decoration.map(|(letter, color)| div().w(px(16.)).ml(px(4.)).text_size(px(12.)).text_color(color).child(letter)))
    }

    /// git's decoration of a resource: the letter and color its `IFileDecorationProvider` gives, read from the
    /// tooltip (git's resource states) since the port has no decorations service yet.
    fn scm_decoration(&self, resource: &Resource) -> Option<(String, Rgba)> {
        let (letter, key) = match resource.tooltip.as_str() {
            "Modified" | "Index Modified" => ("M", "gitDecoration.modifiedResourceForeground"),
            "Added" | "Index Added" | "Intent to Add" => ("A", "gitDecoration.addedResourceForeground"),
            "Deleted" | "Index Deleted" => ("D", "gitDecoration.deletedResourceForeground"),
            "Renamed" | "Index Renamed" => ("R", "gitDecoration.renamedResourceForeground"),
            "Untracked" => ("U", "gitDecoration.untrackedResourceForeground"),
            "Ignored" => ("I", "gitDecoration.ignoredResourceForeground"),
            "Conflict" | "Both Modified" | "Both Added" | "Both Deleted" | "Added By Us" | "Added By Them" | "Deleted By Us" | "Deleted By Them" => ("!", "gitDecoration.conflictingResourceForeground"),
            "Type Changed" | "Index Type Changed" => ("T", "gitDecoration.modifiedResourceForeground"),
            _ => return None,
        };
        Some((letter.to_string(), self.theme.color(key).unwrap_or(self.foreground_or("sideBar.foreground"))))
    }

    /// The `inline` group of a menu as icon buttons (`createAndFillInActionBarActions` with `primaryGroup` inline or
    /// navigation): a command's icon, else its title.
    fn inline_actions(&self, menu: MenuId, context: &HashMap<String, Value>, args: Vec<Value>, foreground: Rgba, cx: &mut Context<Self>) -> Div {
        let primary = if menu == MenuId::ScmTitle { "navigation" } else { "inline" };
        let mut items: Vec<&crate::menus::MenuItem> = self.menus.iter().filter(|item| item.menu == menu && item.group == primary && item.when.as_ref().is_none_or(|when| when.evaluate(context))).collect();
        items.sort_by(|a, b| a.order.total_cmp(&b.order).then_with(|| a.title.cmp(&b.title)));
        let mut bar = div().flex().items_center();
        let commands: Vec<(String, String)> = items
            .into_iter()
            .filter_map(|item| match &item.entry {
                crate::menus::Entry::Command { id, .. } => Some((id.clone(), item.title.clone())),
                crate::menus::Entry::Submenu { .. } => None,
            })
            .collect();
        for (id, title) in commands {
            bar = bar.child(self.command_button(&id, title, args.clone(), foreground, cx));
        }
        bar
    }

    /// A menu command as a toolbar button (`MenuEntryActionViewItem`): its icon, else its title, with its hover.
    pub(in crate::workbench) fn command_button(&self, id: &str, title: String, args: Vec<Value>, foreground: Rgba, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let icon = match self.command_icons.get(id) {
            Some(CommandIcon::Codicon(name)) => self.icon(name, 22., 16., foreground),
            Some(CommandIcon::Image(light, dark)) => {
                let path = if self.theme.kind == crate::theme::ThemeKind::Light { light } else { dark };
                div().size(px(22.)).flex().items_center().justify_center().child(img(path.clone()).size(px(16.)))
            }
            None => div().px(px(4.)).child(title.clone()),
        };
        let command = id.to_string();
        icon.id(gpui::SharedString::from(format!("command-button-{id}-{}", args.first().map(Value::to_string).unwrap_or_default())))
            .rounded(px(5.))
            .cursor_pointer()
            .hover(|icon| icon.bg(self.color("toolbar.hoverBackground")))
            .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                cx.stop_propagation();
                drop(workbench.execute_command(&command, args.clone(), window, cx));
            }))
            .child(self.hover_target(gpui::SharedString::from(format!("command-button-hover-{id}")), HoverContent::Text(title), HoverOptions::mouse()))
    }

    /// activityaction.css `.badge .badge-content`: a `NumberBadge` at the item's bottom right, 16px high, 9px bold.
    pub(in crate::workbench) fn activity_badge(&self, count: u64) -> Option<Div> {
        (count > 0).then(|| {
            div()
                .absolute()
                .top(px(21.))
                .right(px(-1.))
                .h(px(16.))
                .min_w(px(16.))
                .px(px(4.))
                .rounded(px(20.))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(9.))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .bg(self.color("activityBarBadge.background"))
                .text_color(self.color("activityBarBadge.foreground"))
                .child(count.to_string())
        })
    }
}
