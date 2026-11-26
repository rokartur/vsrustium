//! The Source Control view (scmViewPane.ts with scmInput.ts and activity.ts): the `MainThreadSCM` requests, the
//! repository's input box and action button, the resource groups with their resources and inline actions, the
//! `scm/title` actions, the activity bar badge and the status bar entries of the active repository.

use std::collections::{HashMap, HashSet};

use gpui::{Bounds, ClickEvent, Context, Div, FocusHandle, MouseButton, MouseDownEvent, Rgba, Window, div, img, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use super::hover::{HoverContent, HoverOptions};
use super::input_box_view::{InputId, InputLook, InputView};
use super::scm_tree::TreeRow;
use super::view_panes::{PaneContainer, SCM_CHANGES_VIEW, ViewPane};
use crate::commands::Outcome;
use crate::editor::TextDocument;
use crate::input_box::InputBox;
use crate::main_thread_status_bar::{StatusEntry, set_or_update_entry};
use crate::menus::{CommandIcon, MenuId};
use crate::scm::{Group, Provider, Resource, Scm, ScmChange};
use crate::workbench::host_requests::uri_parts;

/// scm.css: list rows are 22px.
pub(super) const ROW_HEIGHT: f32 = 22.;
/// The tree's indent per level (`workbench.tree.indent` 8) plus the twistie (16).
pub(super) const INDENT: f32 = 8.;
pub(super) const TWISTIE: f32 = 16.;

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
    /// Collapsed tree-mode folders: provider, group and the folder relative to the root.
    pub(super) collapsed_folders: HashSet<(u64, u64, std::path::PathBuf)>,
    /// scmViewService.ts: the focused repository, whose input the input box edits.
    pub(super) focused: Option<u64>,
    /// `activeRepository`: the last changed of the active editor's repository and the focused one.
    active: Option<u64>,
    /// The active editor the active repository last followed.
    active_editor: Option<(String, std::path::PathBuf)>,
    /// Collapsed repository rows, by provider handle.
    pub(super) collapsed_repositories: HashSet<u64>,
    pub(super) input_histories: super::scm_actions::InputHistories,
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
            collapsed_folders: HashSet::new(),
            focused: None,
            active: None,
            active_editor: None,
            collapsed_repositories: HashSet::new(),
            input_histories: Default::default(),
        }
    }
}

impl Workbench {
    // shortcut: the Run and Debug, Extensions and webview view headers are drawn here, not as panes of view_panes.rs;
    // a container moves there with a `PaneContainer` variant.
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
            if let Some(provider) = workbench.focused_scm_provider_mut() {
                provider.input.validation = None;
            }
            cx.notify();
            Outcome::done()
        });
    }

    /// One `MainThreadSCM` request.
    pub(crate) fn handle_scm_request(&mut self, method: &str, args: &[Value], window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        self.record_scm_input_value(method, args);
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
            if self.focused_scm_provider().map(|provider| provider.handle) == Some(handle) {
                self.scm.input.set_value(value);
            }
        }
        if method == "$unregisterSourceControl"
            && let Some(handle) = handle
            && let Some((scheme, path)) = self.scm.input_models.remove(&handle)
            && let Some(index) = self.editors.models.iter().position(|model| model.document().is(&scheme, &path))
        {
            self.editors.models.remove(index);
            self.editors_changed(cx);
        }
        // scmViewService.ts `onDidRemoveRepository`: the first shown repository takes the removed one's focus.
        if method == "$unregisterSourceControl" && self.scm.focused == handle {
            self.scm.focused = None;
            self.focus_scm_repository(self.visible_scm_providers().first().map(|provider| provider.handle));
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
        // scmViewService.ts `onDidAddRepository`: a repository is focused when none is.
        let visible = self.scm.model.provider(handle).is_some_and(|provider| !provider.hidden);
        if visible && self.focused_scm_provider().is_none_or(|focused| focused.hidden || self.scm.focused.is_none()) {
            self.focus_scm_repository(Some(handle));
        }
        self.restore_scm_input(handle, cx);
    }

    /// scmViewService.ts `visibleRepositories`: every repository but hidden ones (selection mode `multiple`; only the
    /// Repositories view deselects), in `scm.repositories.sortOrder`.
    pub(in crate::workbench) fn visible_scm_providers(&self) -> Vec<&Provider> {
        let mut providers: Vec<&Provider> = self.scm.model.providers.iter().filter(|provider| !provider.hidden).collect();
        match self.setting("scm.repositories.sortOrder").as_str() {
            Some("name") => providers.sort_by_key(|provider| self.scm_repository_name(provider)),
            Some("path") => providers.sort_by(|a, b| a.root_uri["path"].as_str().cmp(&b.root_uri["path"].as_str())),
            _ => {}
        }
        providers
    }

    /// scmViewService.ts `focus`: the focused repository's input is the one the input box edits, the host hears of it
    /// (mainThreadSCM.ts `onDidFocusRepository`) and it becomes the active repository.
    pub(super) fn focus_scm_repository(&mut self, handle: Option<u64>) {
        if self.scm.focused == handle {
            return;
        }
        self.scm.focused = handle;
        self.scm.active = handle;
        self.host.send("ExtHostSCM", "$setSelectedSourceControl", vec![json!(handle)]);
        self.load_scm_input();
        self.refresh_scm_status_entries();
    }

    /// mainThreadSCM.ts `name`: the workspace folder's name when the root is one, else the root's base name, else
    /// the label.
    fn scm_repository_name(&self, provider: &Provider) -> String {
        let Some(root) = provider.root_uri["path"].as_str().map(std::path::Path::new) else { return provider.label.clone() };
        if let Some(folder) = self.workspace().and_then(|workspace| workspace.folder_of(root)).filter(|folder| folder.path == root) {
            return folder.name.clone();
        }
        root.file_name().map_or_else(|| provider.label.clone(), |name| name.to_string_lossy().into_owned())
    }

    pub(super) fn focused_scm_provider(&self) -> Option<&Provider> {
        self.scm.focused.and_then(|handle| self.scm.model.provider(handle))
    }

    fn focused_scm_provider_mut(&mut self) -> Option<&mut Provider> {
        self.scm.focused.and_then(|handle| self.scm.model.provider_mut(handle))
    }

    /// scmViewService.ts `activeRepository`: the repository of the active editor or the focused one, whichever
    /// changed last.
    pub(in crate::workbench) fn active_scm_provider(&self) -> Option<&Provider> {
        self.scm.active.and_then(|handle| self.scm.model.provider(handle)).or_else(|| self.focused_scm_provider())
    }

    /// `_activeEditorRepositoryObs`: a newly active editor inside a repository (`scmService.getRepository`, the
    /// deepest root holding it) makes that repository active.
    pub(in crate::workbench) fn follow_active_editor_repository(&mut self) {
        let Some(editor) = self.editors.active() else { return };
        let key = (editor.document().scheme.clone(), editor.document().path.clone());
        if self.scm.active_editor.as_ref() == Some(&key) {
            return;
        }
        let repository = self
            .scm
            .model
            .providers
            .iter()
            .filter(|provider| provider.root_uri["scheme"] == json!(key.0) && provider.root_uri["path"].as_str().is_some_and(|root| key.1.starts_with(root)))
            .max_by_key(|provider| provider.root_uri["path"].as_str().map_or(0, str::len))
            .map(|provider| provider.handle);
        self.scm.active_editor = Some(key);
        if repository.is_some() && repository != self.scm.active {
            self.scm.active = repository;
            self.refresh_scm_status_entries();
        }
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

    pub(super) fn focus_scm_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focus = self.scm.input_focus.clone();
        window.focus(&focus, cx);
        self.blink_caret(InputId::ScmInput, window, cx);
    }

    /// scmInput.ts: an edit of the input box sets the repository's input, which the host hears.
    pub(in crate::workbench) fn edit_scm_input(&mut self, edit: impl FnOnce(&mut InputBox), window: &mut Window, cx: &mut Context<Self>) {
        edit(&mut self.scm.input);
        let value = self.scm.input.value.clone();
        let _ = window;
        let Some(provider) = self.focused_scm_provider_mut() else { return };
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

    /// The input boxes' text models, which live as long as their source control.
    pub(super) fn scm_input_models(&self) -> Vec<(String, std::path::PathBuf)> {
        self.scm.input_models.values().cloned().collect()
    }

    pub(super) fn sync_scm_input_model(&mut self, handle: u64, value: &str, cx: &mut Context<Self>) {
        let Some((scheme, path)) = self.scm.input_models.get(&handle).cloned() else { return };
        self.replace_model_text(&scheme, &path, value.to_string(), true, cx);
    }

    /// `scm.acceptInput`: the provider's `acceptInputCommand` with its arguments.
    fn accept_scm_input(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let Some(command) = self.focused_scm_provider().and_then(|provider| provider.accept_input_command.clone()) else { return Outcome::done() };
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
    pub(super) fn scm_context(&self, provider: &Provider, group: Option<&Group>, resource: Option<&Resource>, window: &Window) -> HashMap<String, Value> {
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
            // scmInput.ts: the caret at line 1 column 1, or at the end of the last line.
            context.insert("scmInputIsInFirstPosition".into(), json!(self.scm.input.selection.1 == 0));
            context.insert("scmInputIsInLastPosition".into(), json!(self.scm.input.selection.1 == self.scm.input.len()));
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

    pub(super) fn provider_arg(provider: &Provider) -> Value {
        json!({ "$mid": 5, "handle": provider.handle })
    }

    /// The Source Control container: the Changes pane, the Graph pane while the active repository has a history
    /// provider, then the contributed views. Several repositories, or `scm.alwaysShowRepositories`, show a row per
    /// repository with its children below it (scmViewPane.ts `SCMTreeDataSource.getChildren`).
    pub(in crate::workbench) fn scm_side_bar(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let foreground = self.foreground_or("sideBarSectionHeader.foreground");
        let providers = self.visible_scm_providers();
        let show_repositories = providers.len() > 1 || self.setting("scm.alwaysShowRepositories") == json!(true);
        // `updateScmProviderContextKeys` and `getActionsContext`: the title sees a repository only when it is the only row.
        let (mut context, args) = match (show_repositories, providers.first()) {
            (false, Some(provider)) => (self.scm_context(provider, None, None, window), vec![Self::provider_arg(provider)]),
            _ => (self.key_context(window), Vec::new()),
        };
        self.scm_view_context(&mut context);
        // paneview.css: a header action is its 20px label and a 4px margin, 24px apart as these 22px buttons 2px apart.
        let inline = self.inline_actions(MenuId::ScmTitle, &context, args.clone(), foreground, cx).gap(px(2.));
        let actions = div().flex().items_center().gap(px(2.)).child(inline).child(self.scm_more_actions(context, args, foreground, cx));
        let show_actions = self.scm.input_focus.is_focused(window) || self.scm.list_focus.is_focused(window);
        // scmViewPane.ts: no repository shows the view's welcome content.
        let body = match providers.is_empty() {
            true => div().flex().flex_col().children(self.view_welcome(super::views_welcome::SCM_VIEW, window, cx)).into_any_element(),
            false => {
                let mut rows: Vec<gpui::AnyElement> = Vec::new();
                for provider in &providers {
                    if show_repositories {
                        rows.push(self.scm_repository_row(provider, &providers, window, cx).into_any_element());
                        if self.scm.collapsed_repositories.contains(&provider.handle) {
                            continue;
                        }
                    }
                    self.push_scm_repository_children(&mut rows, provider, providers.len(), usize::from(show_repositories), window, cx);
                }
                div().id("scm-list").flex_1().min_h_0().overflow_y_scroll().track_focus(&self.scm.list_focus).children(rows).into_any_element()
            }
        };
        let mut panes = vec![ViewPane { id: SCM_CHANGES_VIEW, title: "Changes".into(), description: None, actions: Some(actions.into_any_element()), show_actions, body }];
        panes.extend(self.active_scm_provider().filter(|provider| provider.has_history_provider).map(|provider| self.scm_graph_pane(provider, window, cx)));
        self.view_pane_container(PaneContainer::Scm, "Source Control", panes, window, cx).child(div().h(px(5.)).flex_none())
    }

    /// A repository's input, action button (`scm.showActionButton`) and groups; the groups show when some has
    /// changes, or for the only repository without an action button.
    fn push_scm_repository_children(&self, rows: &mut Vec<gpui::AnyElement>, provider: &Provider, repository_count: usize, level: usize, window: &Window, cx: &mut Context<Self>) {
        if provider.input.visible {
            rows.push(self.scm_input_row(provider, level, window, cx).into_any_element());
        }
        let button = provider.action_button.as_ref().filter(|button| button["command"].is_object() && self.setting("scm.showActionButton") != json!(false));
        if let Some(button) = button {
            rows.push(self.scm_action_button(provider.handle, button, level, cx).into_any_element());
        }
        let has_some_changes = provider.groups.iter().any(|group| !group.resources.is_empty());
        if !has_some_changes && !(repository_count == 1 && button.is_none()) {
            return;
        }
        for group in provider.groups.iter().filter(|group| !(group.hide_when_empty && group.resources.is_empty())) {
            rows.push(self.scm_group_row(provider, group, level, window, cx).into_any_element());
            if self.scm.collapsed.contains(&(provider.handle, group.handle)) {
                continue;
            }
            let root = std::path::PathBuf::from(provider.root_uri["path"].as_str().unwrap_or_default());
            let collapsed = |folder: &std::path::Path| self.scm.collapsed_folders.contains(&(provider.handle, group.handle, folder.to_path_buf()));
            let tree = self.scm_tree_mode();
            for row in self.scm_rows(&root, &group.resources, &collapsed) {
                rows.push(match row {
                    TreeRow::Folder { path, label, depth } => self.scm_folder_row(provider, group, path, label, depth + level, cx).into_any_element(),
                    TreeRow::Resource { index, depth } => self.scm_resource_row(provider, group, &group.resources[index], tree.then_some(depth), level, window, cx).into_any_element(),
                });
            }
        }
    }

    /// scmRepositoryRenderer.ts `RepositoryRenderer` with `MenuId.SCMTitle`: the twistie, the repository icon
    /// (`repo-selected` among several shown), the bold name with a description telling same-named repositories apart,
    /// then the status bar commands and the title menu's navigation actions, and the count per `scm.providerCountBadge`.
    fn scm_repository_row(&self, provider: &Provider, providers: &[&Provider], window: &Window, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let foreground = self.foreground_or("sideBar.foreground");
        let handle = provider.handle;
        let collapsed = self.scm.collapsed_repositories.contains(&handle);
        let icon = match provider.icon.as_deref() {
            Some(icon) if icon != "repo" => icon,
            _ if providers.len() > 1 => "repo-selected",
            _ => "repo",
        };
        let name = self.scm_repository_name(provider);
        let root = provider.root_uri["path"].as_str();
        let description = root.and_then(|root| {
            if providers.iter().filter(|other| other.root_uri["path"].as_str() == Some(root)).count() > 1 {
                return Some(provider.label.clone());
            }
            // shortcut: same-named repositories show their whole parent folder, not labels.ts `shorten` of it; port
            // `shorten` when a second view needs it.
            let same_name = providers.iter().filter(|other| other.root_uri["path"].is_string() && self.scm_repository_name(other) == name).count();
            (same_name > 1).then(|| std::path::Path::new(root).parent().map(|parent| crate::quick_input::path_label(parent, self.workspace())).unwrap_or_default())
        });
        let title = match root {
            Some(root) => format!("{}: {}", provider.label, crate::quick_input::path_label(std::path::Path::new(root), None)),
            None => provider.label.clone(),
        };
        let mut toolbar = div().flex().items_center().overflow_hidden();
        for (index, command) in provider.status_bar_commands.iter().enumerate() {
            let label = command["title"].as_str().unwrap_or_default().to_string();
            let tooltip = command["tooltip"].as_str().map_or_else(|| label.clone(), str::to_string);
            let command = command.clone();
            let id = gpui::SharedString::from(format!("scm-repository-status-{handle}-{index}"));
            toolbar = toolbar.child(
                div()
                    .id(id.clone())
                    .ml(px(4.))
                    .px(px(3.))
                    .h(px(20.))
                    .flex()
                    .items_center()
                    .rounded(px(5.))
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .hover(|action| action.bg(self.color("toolbar.hoverBackground")))
                    .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                        cx.stop_propagation();
                        drop(workbench.run_dto_command(&command, window, cx));
                    }))
                    .child(self.highlighted_label(&label, &[], foreground, 16., foreground))
                    .child(self.hover_target(id.clone(), HoverContent::Text(tooltip), HoverOptions::mouse())),
            );
        }
        let context = self.scm_context(provider, None, None, window);
        toolbar = toolbar.child(self.inline_actions(MenuId::ScmTitle, &context, vec![Self::provider_arg(provider)], foreground, cx));
        let count = provider.count();
        let show_count = match self.setting("scm.providerCountBadge").as_str() {
            Some("visible") => true,
            Some("auto") => count > 0,
            _ => false,
        };
        let badge = show_count.then(|| {
            div()
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
                .child(count.to_string())
        });
        let selected = self.scm.focused == Some(handle) && providers.len() > 1;
        div()
            .id(gpui::SharedString::from(format!("scm-repository-{handle}")))
            .h(px(ROW_HEIGHT))
            .pr(px(12.))
            .flex()
            .items_center()
            .cursor_pointer()
            .when(selected, |row| row.bg(self.color("list.inactiveSelectionBackground")))
            .hover(|row| row.bg(self.color("list.hoverBackground")))
            .on_click(cx.listener(move |workbench, _: &ClickEvent, _, cx| {
                workbench.focus_scm_repository(Some(handle));
                if !workbench.scm.collapsed_repositories.remove(&handle) {
                    workbench.scm.collapsed_repositories.insert(handle);
                }
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| workbench.show_scm_source_control_menu(handle, event.position, window, cx)))
            .child(self.icon(if collapsed { "chevron-right" } else { "chevron-down" }, TWISTIE, 16., foreground))
            .child(self.icon(icon, 18., 16., foreground).pr(px(2.)))
            .child(div().flex_none().min_w_0().overflow_hidden().whitespace_nowrap().font_weight(gpui::FontWeight::BOLD).child(name))
            .children(description.map(|description| div().ml(px(6.)).flex_none().whitespace_nowrap().text_size(px(12.)).opacity(0.7).child(description)))
            .child(div().flex_1().min_w(px(24.)).flex().justify_end().child(toolbar))
            .children(badge)
            .child(self.hover_target(gpui::SharedString::from(format!("scm-repository-hover-{handle}")), HoverContent::Text(title), HoverOptions::mouse()))
    }

    /// The input box shows the active repository's input once, then keeps its own state while edited.
    pub(in crate::workbench) fn load_scm_input(&mut self) {
        let shown = self.focused_scm_provider().map(|provider| (provider.handle, provider.input.value.clone()));
        if shown.as_ref().map(|shown| shown.0) != self.scm.shown_input {
            self.scm.shown_input = shown.as_ref().map(|shown| shown.0);
            self.scm.input.set_value(shown.map(|shown| shown.1).unwrap_or_default());
        }
    }

    /// scmViewPane.ts `InputRenderer`: the input editor with its placeholder (`{0}` the commit key), 2px border radius
    /// and `input.*` colors, and the validation message under it.
    /// One input box edits the focused repository's input; another repository's input shows its value until a click
    /// focuses that repository.
    fn scm_input_row(&self, provider: &Provider, level: usize, window: &Window, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let editing = self.scm.focused == Some(provider.handle);
        let focused = editing && self.scm.input_focus.is_focused(window);
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
        let input = match editing {
            true => self.input_text(InputId::ScmInput, look, cx).into_any_element(),
            false if provider.input.value.is_empty() => div().whitespace_nowrap().overflow_hidden().text_color(self.color("input.placeholderForeground")).child(look.placeholder.unwrap_or_default()).into_any_element(),
            false => div().whitespace_nowrap().overflow_hidden().child(provider.input.value.lines().next().unwrap_or_default().to_string()).into_any_element(),
        };
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
        let handle = provider.handle;
        div()
            .id(gpui::SharedString::from(format!("scm-input-{handle}")))
            .pl(px(INDENT * (1 + level) as f32 + TWISTIE))
            .pr(px(12.))
            .pt(px(4.))
            .pb(px(4.))
            .when(!editing, |row| {
                row.on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                    workbench.focus_scm_repository(Some(handle));
                    workbench.focus_scm_input(window, cx);
                    cx.notify();
                }))
            })
            .child(field)
            .children(validation)
    }

    /// scmViewPane.ts `ActionButtonRenderer`: the button with its title (`$(icon)` labels) and, with secondary
    /// commands, the dropdown part after a separator.
    fn scm_action_button(&self, handle: u64, button: &Value, level: usize, cx: &mut Context<Self>) -> Div {
        let enabled = button["enabled"] != false;
        let foreground = self.color("button.foreground");
        let background = self.color("button.background");
        let command = button["command"].clone();
        let title = command["title"].as_str().unwrap_or_default().to_string();
        let label = self.highlighted_label(&title, &[], foreground, 16., foreground);
        let main = div()
            .id(gpui::SharedString::from(format!("scm-action-button-{handle}")))
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
        // scmViewPane.ts `ListDelegate.getHeight`: the row is `DEFAULT_HEIGHT` 28 plus 8, the button centered in it.
        div()
            .h(px(36.))
            .flex_none()
            .flex()
            .items_center()
            .pl(px(INDENT * (1 + level) as f32 + TWISTIE))
            .pr(px(12.))
            .child(div().flex_1().h(px(26.)).flex().rounded(px(2.)).bg(background).text_color(foreground).when(!enabled, |button| button.opacity(0.4)).child(main).children(dropdown))
    }

    fn scm_group_row(&self, provider: &Provider, group: &Group, level: usize, window: &Window, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
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
            // abstractTree.ts `indentSize`: the twistie 8px further in than the input row's and the actions, and the
            // label after the twistie's 6px padding (tree.css), measured against the reference with ui-pair.
            .pl(px(INDENT * (1 + level) as f32 + 7.))
            .pr(px(12.))
            .flex()
            .items_center()
            .cursor_pointer()
            .when(selected, |row| row.bg(self.color("list.inactiveSelectionBackground")))
            .hover(|row| row.bg(self.color("list.hoverBackground")))
            .on_click(cx.listener(move |workbench, _: &ClickEvent, _, cx| {
                workbench.focus_scm_repository(Some(key.0));
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
            .child(div().flex_1().min_w_0().ml(px(3.)).overflow_hidden().whitespace_nowrap().child(group.label.clone()))
            .child(div().invisible().group_hover(id, |actions| actions.visible()).child(actions))
            .child(badge)
    }

    /// scmViewPane.ts `ResourceRenderer`: file icon, name, the folder as description, inline actions on hover and the
    /// decoration letter from the resource's tooltip and color (git's `letter` via its decoration provider).
    fn scm_resource_row(&self, provider: &Provider, group: &Group, resource: &Resource, tree_depth: Option<usize>, level: usize, window: &Window, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let foreground = self.foreground_or("sideBar.foreground");
        let path = std::path::PathBuf::from(resource.uri["path"].as_str().unwrap_or_default());
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let root = provider.root_uri["path"].as_str().unwrap_or_default();
        // scmViewPane.ts `ResourceRenderer`: `hidePath` in tree mode.
        let folder = match tree_depth {
            Some(_) => String::new(),
            None => path.parent().and_then(|parent| parent.strip_prefix(root).ok()).map(|folder| folder.to_string_lossy().into_owned()).unwrap_or_default(),
        };
        let context = self.scm_context(provider, Some(group), Some(resource), window);
        let args = vec![Self::resource_arg(provider, group, resource)];
        let actions = self.inline_actions(MenuId::ScmResourceStateContext, &context, args.clone(), foreground, cx);
        let decoration = self.scm_decoration(&path);
        let language = self.languages.language_of(&path, "").to_string();
        let icon = self.file_icons.as_ref().and_then(|icons| icons.file_icon(&path, &language, self.theme.kind, 13., ROW_HEIGHT, foreground)).unwrap_or_else(|| div().w(px(16.)));
        let key = (provider.handle, group.handle, Some(resource.handle));
        let selected = self.scm.selected == Some(key);
        let id = gpui::SharedString::from(format!("scm-resource-{}-{}-{}", provider.handle, group.handle, resource.handle));
        let handles = (provider.handle, group.handle, resource.handle);
        let editor_command = resource.command.clone().filter(|command| matches!(command["id"].as_str(), Some("_workbench.open" | "_workbench.diff")));
        div()
            .id(id.clone())
            .group(id.clone())
            .h(px(ROW_HEIGHT))
            // The icon 4px past the indent, measured against the reference with ui-pair.
            .pl(px(INDENT * (2 + level + tree_depth.unwrap_or(0)) as f32 + 4.))
            .pr(px(12.))
            .flex()
            .items_center()
            .cursor_pointer()
            .when(resource.faded, |row| row.opacity(0.7))
            .when(selected, |row| row.bg(self.color("list.inactiveSelectionBackground")))
            .hover(|row| row.bg(self.color("list.hoverBackground")))
            .on_click(cx.listener(move |workbench, event: &ClickEvent, window, cx| {
                workbench.focus_scm_repository(Some(key.0));
                workbench.scm.selected = Some(key);
                // scmViewPane.ts `open`: `_workbench.open`/`_workbench.diff` arrive converted and run here; the ext host keeps only other commands.
                if let Some(command) = editor_command.as_ref() {
                    drop(workbench.run_dto_command(command, window, cx));
                } else {
                    // `ResourceTree.open`: a single click previews with the focus kept, a double click pins and focuses.
                    let preserve_focus = event.click_count() < 2;
                    drop(workbench.host.call("ExtHostSCM", "$executeResourceCommand", vec![json!(handles.0), json!(handles.1), json!(handles.2), json!(preserve_focus)]));
                }
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| {
                workbench.show_context_menu(MenuId::ScmResourceStateContext, &context, Bounds::new(event.position, Default::default()), args.clone(), window, cx);
            }))
            .child(icon.mr(px(6.)))
            // scmViewPane.ts `fileDecorations: { colors: false }`: only the letter takes the decoration's color.
            .child(div().flex_none().whitespace_nowrap().when(resource.strike_through, |name| name.line_through()).child(name))
            .child(div().flex_1().min_w_0().ml(px(6.)).overflow_hidden().whitespace_nowrap().text_size(px(12.)).opacity(0.7).child(folder))
            .child(div().invisible().group_hover(id, |actions| actions.visible()).child(actions))
            .children(decoration.map(|(letter, color)| div().w(px(16.)).ml(px(4.)).text_size(px(12.)).text_color(color).child(letter)))
    }

    /// The letter and color the decorations service has for a resource (git's `IFileDecorationProvider`).
    fn scm_decoration(&self, path: &std::path::Path) -> Option<(String, Rgba)> {
        let decoration = self.file_decoration(path, false)?;
        let color = decoration.color.as_deref().and_then(|color| self.theme.color(color)).unwrap_or(self.foreground_or("sideBar.foreground"));
        Some((decoration.letter?, color))
    }

    /// The `inline` group of a menu as icon buttons (`createAndFillInActionBarActions` with `primaryGroup` inline or
    /// navigation): a command's icon, else its title.
    pub(in crate::workbench) fn inline_actions(&self, menu: MenuId, context: &HashMap<String, Value>, args: Vec<Value>, foreground: Rgba, cx: &mut Context<Self>) -> Div {
        let primary = if matches!(menu, MenuId::ScmTitle | MenuId::ViewTitle) { "navigation" } else { "inline" };
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
            .on_mouse_down(MouseButton::Left, stop_action_mouse_down)
            .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                cx.stop_propagation();
                drop(workbench.execute_command(&command, args.clone(), window, cx));
            }))
            .child(self.hover_target(gpui::SharedString::from(format!("command-button-hover-{id}")), HoverContent::Text(title), HoverOptions::mouse()))
    }

}

/// actionViewItems.ts `EventHelper.stop(e, true)` on mouse down: an action takes no focus and the header under it
/// neither focuses nor starts a drag.
pub(super) fn stop_action_mouse_down(_: &MouseDownEvent, window: &mut Window, cx: &mut gpui::App) {
    window.prevent_default();
    cx.stop_propagation();
}

#[cfg(test)]
mod tests;
