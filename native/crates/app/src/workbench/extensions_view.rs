//! The Extensions view container (extensionsViewlet.ts, extensionsViews.ts, extensionsWorkbenchService.ts): the
//! search box, the views each query shows, their models from the installed extensions and from Open VSX, the
//! enablement, install, uninstall and update actions, the update check and auto update, and the activity badge.
//! The list rows are drawn in extensions_list_view.rs, the extension editor in extension_editor.rs.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use exthost::extension_management::{Job, UserExtensions, add_user_extensions};
use exthost::gallery::{ExtensionInfo, Gallery, GalleryExtension, QueryOptions};
use exthost::scan::ScannedExtension;
use gpui::{Context, FocusHandle, Image, ImageFormat, Task, UniformListScrollHandle, Window};
use serde_json::{Value, json};

use super::Workbench;
use super::input_box_view::InputView;
use super::search_view::SideBarView;
use crate::commands::Outcome;
use crate::extension_recommendations as recommendations;
use crate::extension_enablement::{Enablement, EnablementInput, EnablementState, Identifier};
use crate::extensions_model::{self as model, Extension, ExtensionState, Query, SortBy};
use crate::input_box::InputBox;
use crate::notifications::Severity;

/// `VIEWLET_ID`.
pub const VIEWLET_ID: &str = "workbench.view.extensions";
/// extensionsWorkbenchService.ts `UpdatesCheckInterval`: 12 hours.
const UPDATES_CHECK_INTERVAL: Duration = Duration::from_secs(12 * 60 * 60);
/// The gallery pager's page size (`queryGallery` `pageSize`).
const PAGE_SIZE: usize = 50;

/// One view of the container: its id, its title and the query it shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ViewDescriptor {
    pub id: &'static str,
    pub title: &'static str,
    /// The query `show(query)` turns the search into; `None` shows the search itself.
    kind: ViewKind,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ViewKind {
    /// `ExtensionsListView`: the search as typed.
    Search,
    /// `ServerInstalledExtensionsView`.
    Installed,
    /// `DefaultPopularExtensionsView`: the gallery sorted by installs.
    Popular,
    /// `DefaultRecommendedExtensionsView`.
    DefaultRecommended,
    /// `OutdatedExtensionsView`.
    Outdated,
    /// `RecentlyUpdatedExtensionsView`.
    RecentlyUpdated,
    /// `StaticQueryExtensionsView` with its query.
    Static(&'static str),
    /// `WorkspaceRecommendedExtensionsView`.
    WorkspaceRecommended,
    /// `RecommendedExtensionsView`.
    OtherRecommended,
    /// The views that show nothing in this product: workspace trust and virtual workspaces are off.
    Empty,
}

impl ViewDescriptor {
    const fn new(id: &'static str, title: &'static str, kind: ViewKind) -> ViewDescriptor {
        ViewDescriptor { id, title, kind }
    }

    /// The query this view's `show` runs for the container's `query`.
    fn view_query(&self, query: &str) -> String {
        match self.kind {
            ViewKind::Search => query.to_owned(),
            ViewKind::Installed => {
                let query = if query.is_empty() { "@installed".to_owned() } else { query.to_owned() };
                match !model::is_local_extensions_query(&query, "") || model::is_sort_installed_extensions_query(&query, "") {
                    true => format!("{query} @installed").trim().to_owned(),
                    false => query,
                }
            }
            ViewKind::Popular => String::new(),
            ViewKind::DefaultRecommended => "@recommended:all".into(),
            ViewKind::Outdated => if query.is_empty() { "@outdated".into() } else { query.replace("@updates", "@outdated") },
            ViewKind::RecentlyUpdated => if query.is_empty() { "@recentlyUpdated".into() } else { query.replace("@updates", "@recentlyUpdated") },
            ViewKind::Static(static_query) => static_query.to_owned(),
            ViewKind::WorkspaceRecommended => "@recommended:workspace".into(),
            ViewKind::OtherRecommended => "@recommended".into(),
            ViewKind::Empty => String::new(),
        }
    }
}

/// The ids of every view `visible_views` can show.
pub const VIEW_IDS: [&str; 19] = [
    "workbench.views.extensions.installed",
    "workbench.views.extensions.popular",
    "extensions.recommendedList",
    "workbench.views.extensions.marketplace",
    "workbench.views.extensions.searchInstalled",
    "workbench.views.extensions.searchOutdated",
    "workbench.views.extensions.searchRecentlyUpdated",
    "workbench.views.extensions.searchEnabled",
    "workbench.views.extensions.searchDisabled",
    "workbench.views.extensions.searchBuiltin",
    "workbench.views.extensions.untrustedUnsupportedExtensions",
    "workbench.views.extensions.untrustedPartiallySupportedExtensions",
    "workbench.views.extensions.workspaceRecommendations",
    "workbench.views.extensions.otherRecommendations",
    "workbench.views.extensions.builtinFeatureExtensions",
    "workbench.views.extensions.builtinThemeExtensions",
    "workbench.views.extensions.builtinProgrammingLanguageExtensions",
    "workbench.views.extensions.deprecatedExtensions",
    "workbench.views.extensions.restartRequired",
];

/// The views of extensionsViewlet.ts `registerViews` the search `value` shows, in their order, with
/// `has_installed` (`hasInstalledExtensions`) and `has_workspace` (`WorkbenchStateContext` not empty).
pub fn visible_views(value: &str, has_installed: bool, has_workspace: bool) -> Vec<ViewDescriptor> {
    let query = Query::parse(value);
    let default_views = value.is_empty() || model::is_sort_installed_extensions_query(&query.value, &query.sort_by);
    let recommended = model::is_recommended_extensions_query(value);
    let mut views = Vec::new();
    if default_views {
        views.push(ViewDescriptor::new("workbench.views.extensions.installed", "Installed", ViewKind::Installed));
        if !has_installed {
            views.push(ViewDescriptor::new("workbench.views.extensions.popular", "Popular", ViewKind::Popular));
        }
        if !model::is_sort_update_date_query(value) {
            views.push(ViewDescriptor::new("extensions.recommendedList", "Recommended", ViewKind::DefaultRecommended));
        }
        return views;
    }
    let marketplace = !model::is_local_extensions_query(value, "") && !recommended && !value.to_lowercase().contains("@mcp") && !value.to_lowercase().contains("@agentplugins");
    if marketplace {
        views.push(ViewDescriptor::new("workbench.views.extensions.marketplace", "Marketplace", ViewKind::Search));
    }
    if model::is_installed_extensions_query(value) || model::is_search_installed_extensions_query(value) {
        views.push(ViewDescriptor::new("workbench.views.extensions.searchInstalled", "Installed", ViewKind::Search));
    }
    let updates = model::is_search_extension_updates_query(value);
    if updates || (model::is_outdated_extensions_query(value) && !updates) {
        views.push(ViewDescriptor::new("workbench.views.extensions.searchOutdated", "Available Updates", ViewKind::Outdated));
    }
    if updates || model::is_search_recently_updated_query(value) {
        views.push(ViewDescriptor::new("workbench.views.extensions.searchRecentlyUpdated", "Recently Updated", ViewKind::RecentlyUpdated));
    }
    if model::is_enabled_extensions_query(value) {
        views.push(ViewDescriptor::new("workbench.views.extensions.searchEnabled", "Enabled", ViewKind::Search));
    }
    if model::is_disabled_extensions_query(value) {
        views.push(ViewDescriptor::new("workbench.views.extensions.searchDisabled", "Disabled", ViewKind::Search));
    }
    if model::is_search_built_in_extensions_query(value) {
        views.push(ViewDescriptor::new("workbench.views.extensions.searchBuiltin", "Builtin", ViewKind::Search));
    }
    if model::is_search_workspace_unsupported_extensions_query(value) {
        views.push(ViewDescriptor::new("workbench.views.extensions.untrustedUnsupportedExtensions", "Disabled in Restricted Mode", ViewKind::Empty));
        views.push(ViewDescriptor::new("workbench.views.extensions.untrustedPartiallySupportedExtensions", "Limited in Restricted Mode", ViewKind::Empty));
    }
    if recommended {
        if has_workspace {
            views.push(ViewDescriptor::new("workbench.views.extensions.workspaceRecommendations", "Workspace Recommendations", ViewKind::WorkspaceRecommended));
        }
        views.push(ViewDescriptor::new("workbench.views.extensions.otherRecommendations", "Other Recommendations", ViewKind::OtherRecommended));
    }
    if model::is_built_in_extensions_query(value) {
        views.push(ViewDescriptor::new("workbench.views.extensions.builtinFeatureExtensions", "Features", ViewKind::Static(BUILTIN_FEATURES_QUERY)));
        views.push(ViewDescriptor::new("workbench.views.extensions.builtinThemeExtensions", "Themes", ViewKind::Static("@builtin category:themes")));
        views.push(ViewDescriptor::new("workbench.views.extensions.builtinProgrammingLanguageExtensions", "Programming Languages", ViewKind::Static("@builtin category:\"programming languages\"")));
    }
    if model::is_search_deprecated_extensions_query(value) {
        views.push(ViewDescriptor::new("workbench.views.extensions.deprecatedExtensions", "Deprecated", ViewKind::Search));
    }
    if model::is_restart_required_query(value) {
        views.push(ViewDescriptor::new("workbench.views.extensions.restartRequired", "Restart Required", ViewKind::Search));
    }
    views
}

/// extensionsViewlet.ts `otherCategoriesQuery`: every category but themes and programming languages, and none.
const BUILTIN_FEATURES_QUERY: &str = "@builtin category:none category:\"ai\" category:\"azure\" category:\"chat\" category:\"data science\" category:\"debuggers\" category:\"extension packs\" category:\"education\" category:\"formatters\" category:\"keymaps\" category:\"language packs\" category:\"linters\" category:\"machine learning\" category:\"notebooks\" category:\"scm providers\" category:\"snippets\" category:\"testing\" category:\"visualization\" category:\"other\"";

/// A view of the container with its model.
pub struct Pane {
    pub descriptor: ViewDescriptor,
    pub query: String,
    pub extensions: Vec<Extension>,
    /// The gallery's total for a gallery query, the pages after the first load on scroll.
    pub total: usize,
    pub loading: bool,
    /// The message of an empty or failed result (`IMessage`).
    pub message: Option<(Severity, String)>,
    pub expanded: bool,
    pub scroll: UniformListScrollHandle,
    /// The gallery results behind `extensions`, merged again with the installed ones on each change.
    gallery: Option<Vec<GalleryExtension>>,
    pages_loaded: usize,
}

pub struct ExtensionsViewlet {
    pub search: InputBox,
    pub search_view: InputView,
    pub search_focus: FocusHandle,
    pub list_focus: FocusHandle,
    pub panes: Vec<Pane>,
    /// The built-in extensions and the user extensions folder, scanned again after each change.
    pub builtins: Vec<ScannedExtension>,
    pub user: Option<UserExtensions>,
    /// Every installed extension, the user copy replacing the built-in one.
    pub installed: Vec<ScannedExtension>,
    /// The gallery entry of each installed extension, by lowercase id, from the last update check.
    pub gallery: HashMap<String, GalleryExtension>,
    pub installing: Vec<String>,
    pub uninstalling: Vec<String>,
    /// The `--disable-extension` ids and `--disable-extensions`.
    pub disabled_in_env: Vec<String>,
    pub disable_all_in_env: bool,
    /// The extension under development (`--extensionDevelopmentPath`).
    pub development: Vec<String>,
    /// Downloaded gallery icons by URL; `None` while loading or after a failure.
    pub icons: HashMap<String, Option<Arc<Image>>>,
    /// The focused and the selected row: pane index and row.
    pub focused: Option<(usize, usize)>,
    pub selected: Option<(usize, usize)>,
    /// The expansion the user gave a view by its id.
    pub expansion: HashMap<&'static str, bool>,
    pub(super) search_task: Option<Task<()>>,
    pub(super) update_check: Option<Task<()>>,
}

impl ExtensionsViewlet {
    pub fn new(cx: &mut Context<Workbench>) -> ExtensionsViewlet {
        ExtensionsViewlet {
            search: InputBox::default(),
            search_view: InputView::default(),
            search_focus: cx.focus_handle(),
            list_focus: cx.focus_handle(),
            panes: Vec::new(),
            builtins: Vec::new(),
            user: None,
            installed: Vec::new(),
            gallery: HashMap::new(),
            installing: Vec::new(),
            uninstalling: Vec::new(),
            disabled_in_env: Vec::new(),
            disable_all_in_env: false,
            development: Vec::new(),
            icons: HashMap::new(),
            focused: None,
            selected: None,
            expansion: HashMap::new(),
            search_task: None,
            update_check: None,
        }
    }

    /// `hasInstalledExtensions`: a user extension is installed.
    pub fn has_installed(&self) -> bool {
        self.installed.iter().any(|extension| extension.description["isBuiltin"] != true)
    }
}

/// The ids main.rs leaves out of the host at startup: what the user disabled, globally or in this workspace.
pub fn disabled_at_startup(extensions: &[ScannedExtension], profile: &mut storage::Storage, workspace: Option<&mut storage::Storage>, development: &[String]) -> Vec<String> {
    let enablement = Enablement { profile, workspace };
    let inputs = enablement_inputs(extensions, &[], false, development);
    (0..inputs.len()).filter(|&index| !enablement.state(&inputs, index).is_enabled()).map(|index| extensions[index].id.clone()).collect()
}

fn enablement_inputs<'a>(extensions: &'a [ScannedExtension], disabled_in_env: &[String], disable_all: bool, development: &[String]) -> Vec<EnablementInput<'a>> {
    extensions
        .iter()
        .map(|extension| {
            let id = &extension.id;
            let is_builtin = extension.description["isBuiltin"] == true;
            EnablementInput {
                identifier: identifier(extension),
                manifest: &extension.description,
                disabled_in_env: disabled_in_env.iter().any(|disabled| disabled.eq_ignore_ascii_case(id)) || (disable_all && !is_builtin),
                enabled_in_env: development.iter().any(|developed| developed.eq_ignore_ascii_case(id)),
                is_valid: true,
            }
        })
        .collect()
}

fn identifier(extension: &ScannedExtension) -> Identifier {
    Identifier { id: extension.id.clone(), uuid: extension.description["uuid"].as_str().map(str::to_owned) }
}

impl Workbench {
    /// Startup: the built-ins and the user folder the window scanned, then the first update check.
    pub fn set_installed_extensions(&mut self, builtins: Vec<ScannedExtension>, user: UserExtensions, development: Vec<String>, cx: &mut Context<Self>) {
        let viewlet = &mut self.extensions_viewlet;
        viewlet.builtins = builtins;
        viewlet.user = Some(user);
        viewlet.development = development;
        self.rescan_installed_extensions();
        self.eventually_check_for_updates(true, cx);
        self.schedule_workspace_recommendations_prompt(cx);
    }

    pub(super) fn rescan_installed_extensions(&mut self) {
        let viewlet = &mut self.extensions_viewlet;
        let mut installed = viewlet.builtins.clone();
        let user = viewlet.user.as_ref().map(|user| user.scan()).transpose();
        match user {
            Ok(user) => add_user_extensions(&mut installed, user.unwrap_or_default()),
            Err(error) => eprintln!("[extensions] scanning the user extensions: {error:#}"),
        }
        viewlet.installed = installed;
        self.sync_language_packs();
    }

    /// The `Extension`s of the installed list (`extensionsWorkbenchService.local`) with their enablement and runtime state.
    pub fn local_extensions(&self) -> Vec<Extension> {
        let viewlet = &self.extensions_viewlet;
        let inputs = enablement_inputs(&viewlet.installed, &viewlet.disabled_in_env, viewlet.disable_all_in_env, &viewlet.development);
        let mut profile = self.profile_storage.lock().unwrap();
        let mut workspace = self.workspace_storage.lock().unwrap();
        let enablement = Enablement { profile: &mut profile, workspace: self.folder.is_some().then_some(&mut *workspace) };
        viewlet
            .installed
            .iter()
            .enumerate()
            .map(|(index, local)| {
                let state = enablement.state(&inputs, index);
                let running = self.extensions.iter().find(|running| running.id.eq_ignore_ascii_case(&local.id));
                let restart_required = match running {
                    Some(running) => !state.is_enabled() || running.location != local.location,
                    None => state.is_enabled(),
                };
                let lower = local.id.to_lowercase();
                let extension_state = match () {
                    _ if viewlet.uninstalling.contains(&lower) => ExtensionState::Uninstalling,
                    _ if viewlet.installing.contains(&lower) => ExtensionState::Installing,
                    _ => ExtensionState::Installed,
                };
                Extension {
                    local: Some(local.clone()),
                    gallery: viewlet.gallery.get(&lower).cloned(),
                    state: extension_state,
                    enablement: state,
                    running: running.is_some(),
                    restart_required,
                }
            })
            .collect()
    }

    /// A gallery result as the view shows it: the installed copy's state when it is installed.
    fn gallery_extension(&self, gallery: &GalleryExtension, local: &[Extension]) -> Extension {
        let lower = gallery.id.to_lowercase();
        if let Some(installed) = local.iter().find(|extension| extension.identifier().id.eq_ignore_ascii_case(&gallery.id)) {
            return Extension { gallery: Some(gallery.clone()), ..installed.clone() };
        }
        let state = if self.extensions_viewlet.installing.contains(&lower) { ExtensionState::Installing } else { ExtensionState::Uninstalled };
        Extension { local: None, gallery: Some(gallery.clone()), state, enablement: EnablementState::EnabledGlobally, running: false, restart_required: false }
    }

    /// The extension the view or the editor shows, by id: installed, else from a gallery result.
    pub fn find_extension(&self, id: &str) -> Option<Extension> {
        let local = self.local_extensions();
        if let Some(extension) = local.iter().find(|extension| extension.identifier().id.eq_ignore_ascii_case(id)) {
            return Some(extension.clone());
        }
        let viewlet = &self.extensions_viewlet;
        let from_panes = viewlet.panes.iter().flat_map(|pane| pane.gallery.iter().flatten());
        let from_editor = self.extension_editors.iter().filter_map(|editor| editor.gallery.as_ref());
        from_panes.chain(from_editor).find(|gallery| gallery.id.eq_ignore_ascii_case(id)).map(|gallery| self.gallery_extension(gallery, &local))
    }

    pub(super) fn register_extensions_view_commands(&mut self) {
        self.commands.register(VIEWLET_ID, |workbench, _, window, cx| {
            workbench.open_extensions_viewlet(window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.extensions.action.focusExtensionsView", |workbench, _, window, cx| {
            workbench.open_extensions_viewlet(window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.extensions.search", |workbench, args, window, cx| {
            let query = args.first().and_then(Value::as_str).unwrap_or_default().to_owned();
            workbench.search_extensions(&query, window, cx);
            Outcome::done()
        });
        let searches = [
            ("workbench.extensions.action.installExtensions", ""),
            ("workbench.extensions.action.showRecommendedExtensions", "@recommended "),
            ("workbench.extensions.action.showRecommendedKeymapExtensions", "@recommended:keymaps "),
            ("workbench.extensions.action.showLanguageExtensions", "@recommended:languages "),
            ("workbench.extensions.action.showPopularExtensions", "@popular "),
            ("workbench.extensions.action.recentlyPublishedExtensions", "@recentlyPublished "),
            ("workbench.extensions.action.installedExtensions", "@installed "),
            ("workbench.extensions.action.listBuiltInExtensions", "@builtin "),
            ("workbench.extensions.action.extensionUpdates", "@updates"),
            ("workbench.extensions.action.listWorkspaceUnsupportedExtensions", "@workspaceUnsupported"),
            ("workbench.extensions.action.showEnabledExtensions", "@enabled "),
            ("workbench.extensions.action.showDisabledExtensions", "@disabled "),
        ];
        for (id, query) in searches {
            self.commands.register(id, move |workbench, _, window, cx| {
                workbench.search_extensions(query, window, cx);
                Outcome::done()
            });
        }
        self.commands.register("extensions.filter.featured", |workbench, _, window, cx| {
            workbench.search_extensions("@featured ", window, cx);
            Outcome::done()
        });
        for category in model::CATEGORIES {
            self.commands.register(format!("extensions.actions.searchByCategory.{category}"), move |workbench, _, window, cx| {
                workbench.search_extensions(&format!("@category:\"{}\"", category.to_lowercase()), window, cx);
                Outcome::done()
            });
        }
        for (sort, _) in super::extensions_list_view::SORTS {
            // The Sort By actions keep the query and replace its `@sort:`.
            self.commands.register(format!("extensions.sort.{sort}"), move |workbench, _, window, cx| {
                let query = Query { sort_by: sort.into(), ..Query::parse(&workbench.extensions_viewlet.search.value) };
                workbench.search_extensions(&query.to_string(), window, cx);
                Outcome::done()
            });
        }
        self.commands.register("workbench.extensions.action.clearExtensionsSearchResults", |workbench, _, window, cx| {
            workbench.search_extensions("", window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.extensions.action.refreshExtension", |workbench, _, window, cx| {
            workbench.rescan_installed_extensions();
            workbench.check_for_updates(cx);
            workbench.show_extensions_views(true, window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.extensions.action.checkForUpdates", |workbench, _, _, cx| {
            workbench.check_for_updates_and_notify(cx);
            Outcome::done()
        });
        self.commands.register("workbench.extensions.action.updateAllExtensions", |workbench, _, _, cx| {
            let outdated: Vec<Extension> = workbench.outdated_extensions().into_iter().filter(|extension| extension.enablement.is_enabled()).collect();
            for extension in outdated {
                drop(workbench.update_extension(&extension, cx));
            }
            Outcome::done()
        });
        self.commands.register("workbench.extensions.action.enableAutoUpdate", |workbench, _, _, cx| workbench.write_user_setting_value("extensions.autoUpdate", json!(true), cx));
        self.commands.register("workbench.extensions.action.disableAutoUpdate", |workbench, _, _, cx| workbench.write_user_setting_value("extensions.autoUpdate", json!(false), cx));
        let all = [
            ("workbench.extensions.action.enableAll", EnablementState::EnabledGlobally),
            ("workbench.extensions.action.enableAllWorkspace", EnablementState::EnabledWorkspace),
            ("workbench.extensions.action.disableAll", EnablementState::DisabledGlobally),
            ("workbench.extensions.action.disableAllWorkspace", EnablementState::DisabledWorkspace),
        ];
        for (id, state) in all {
            self.commands.register(id, move |workbench, _, _, cx| workbench.set_all_enablement(state, cx));
        }
        let single = [
            ("extensions.enableGlobally", EnablementState::EnabledGlobally),
            ("extensions.enableForWorkspace", EnablementState::EnabledWorkspace),
            ("extensions.disableGlobally", EnablementState::DisabledGlobally),
            ("extensions.disableForWorkspace", EnablementState::DisabledWorkspace),
        ];
        for (id, state) in single {
            self.commands.register(id, move |workbench, args, _, cx| {
                let Some(id) = args.first().and_then(Value::as_str) else { return Outcome::error("Extension id required.") };
                workbench.set_extension_enablement(&[id.to_owned()], state, cx)
            });
        }
        self.commands.register("extensions.uninstall", |workbench, args, window, cx| {
            let id = args.first().and_then(Value::as_str).unwrap_or_default().to_owned();
            workbench.execute_command("workbench.extensions.uninstallExtension", vec![json!(id)], window, cx)
        });
        self.commands.register("extensions.install", |workbench, args, window, cx| {
            let id = args.first().and_then(Value::as_str).unwrap_or_default().to_owned();
            let pre_release = args.get(1) == Some(&Value::Bool(true));
            workbench.execute_command("workbench.extensions.installExtension", vec![json!(id), json!({ "installPreReleaseVersion": pre_release })], window, cx)
        });
        self.commands.register("extensions.update", |workbench, args, _, cx| {
            let id = args.first().and_then(Value::as_str).unwrap_or_default();
            match workbench.find_extension(id) {
                Some(extension) => workbench.update_extension(&extension, cx),
                None => Outcome::error(format!("Extension '{id}' is not installed.")),
            }
        });
        self.commands.register("extension.open", |workbench, args, window, cx| {
            let Some(id) = args.first().and_then(Value::as_str) else { return Outcome::error("Extension id required.") };
            match workbench.find_extension(id) {
                Some(extension) => {
                    workbench.open_extension_editor(extension, false, window, cx);
                    Outcome::done()
                }
                None => {
                    let id = id.to_owned();
                    workbench.open_gallery_extension_editor(id, window, cx);
                    Outcome::done()
                }
            }
        });
        self.commands.register("workbench.extensions.action.copyExtension", |workbench, args, _, cx| {
            let Some(extension) = args.first().and_then(Value::as_str).and_then(|id| workbench.find_extension(id)) else { return Outcome::done() };
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy_text(&extension)));
            Outcome::done()
        });
        self.commands.register("workbench.extensions.action.copyExtensionId", |_, args, _, cx| {
            if let Some(id) = args.first().and_then(Value::as_str) {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(id.to_owned()));
            }
            Outcome::done()
        });
        self.commands.register("workbench.extensions.action.copyLink", |workbench, args, _, cx| {
            let link = args.first().and_then(Value::as_str).and_then(|id| workbench.find_extension(id)).and_then(|extension| extension.gallery.map(|gallery| gallery.detail_link));
            if let Some(link) = link {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(link));
            }
            Outcome::done()
        });
        self.commands.register("workbench.extensions.action.configure", |workbench, args, window, cx| {
            let id = args.first().and_then(Value::as_str).unwrap_or_default();
            workbench.execute_command("workbench.action.openSettings", vec![json!(format!("@ext:{id}"))], window, cx)
        });
        self.commands.register("workbench.extensions.action.configureKeybindings", |workbench, args, window, cx| {
            let id = args.first().and_then(Value::as_str).unwrap_or_default();
            workbench.execute_command("workbench.action.openGlobalKeybindings", vec![json!(format!("@ext:{id}"))], window, cx)
        });
        self.commands.register("workbench.extensions.action.toggleAutoUpdateForExtension", |workbench, args, _, cx| {
            let id = args.first().and_then(Value::as_str).unwrap_or_default().to_lowercase();
            workbench.toggle_auto_update_for(&id, cx);
            Outcome::done()
        });
        // The list commands act on the focused list: the extensions list here, else whoever registered before.
        let moves: [(&str, isize); 6] = [("list.focusDown", 1), ("list.focusUp", -1), ("list.focusPageDown", 10), ("list.focusPageUp", -10), ("list.focusFirst", isize::MIN), ("list.focusLast", isize::MAX)];
        for (id, delta) in moves {
            let previous = self.commands.get(id);
            self.commands.register(id, move |workbench, args, window, cx| {
                if workbench.extensions_viewlet.list_focus.is_focused(window) {
                    workbench.move_extensions_focus(delta, window, cx);
                    return Outcome::done();
                }
                previous.as_ref().map_or_else(Outcome::done, |previous| previous(workbench, args, window, cx))
            });
        }
        // Enter opens the focused extension pinned.
        let previous = self.commands.get("list.select");
        self.commands.register("list.select", move |workbench, args, window, cx| {
            if workbench.extensions_viewlet.list_focus.is_focused(window) {
                if let Some(extension) = workbench.extensions_viewlet.focused.and_then(|(pane, row)| workbench.extensions_viewlet.panes.get(pane)?.extensions.get(row).cloned()) {
                    workbench.open_extension_editor(extension, false, window, cx);
                }
                return Outcome::done();
            }
            previous.as_ref().map_or_else(Outcome::done, |previous| previous(workbench, args, window, cx))
        });
    }

    /// The view's context keys: `activeViewlet`, `extensionsSearchValue`, the list's `listFocus`, the box's `inputFocus`.
    pub(super) fn extensions_context(&self, window: &Window, context: &mut HashMap<String, Value>) {
        let viewlet = &self.extensions_viewlet;
        let input = viewlet.search_focus.is_focused(window);
        let list = viewlet.list_focus.is_focused(window);
        context.insert("activeViewlet".into(), json!(VIEWLET_ID));
        context.insert("extensionsSearchValue".into(), json!(viewlet.search.value));
        context.insert("sideBarFocus".into(), json!(input || list));
        context.insert("hasInstalledExtensions".into(), json!(viewlet.has_installed()));
        if input {
            context.insert("inputFocus".into(), json!(true));
            context.insert("inputBoxFocus".into(), json!(true));
        }
        if list {
            context.insert("listFocus".into(), json!(true));
        }
    }

    /// `openExtensionsViewlet`: the view shows and its search box takes the focus.
    pub(super) fn open_extensions_viewlet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let first_open = self.extensions_viewlet.panes.is_empty();
        self.side_bar_view = SideBarView::Extensions;
        if first_open {
            self.show_extensions_views(false, window, cx);
        }
        window.focus(&self.extensions_viewlet.search_focus.clone(), cx);
        cx.notify();
    }

    /// `search(value)`: the box takes the text and the views show it at once.
    pub(super) fn search_extensions(&mut self, value: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.side_bar_view = SideBarView::Extensions;
        self.extensions_viewlet.search.set_value(value.to_owned());
        self.extensions_viewlet.search.select(value.chars().count(), value.chars().count());
        window.focus(&self.extensions_viewlet.search_focus.clone(), cx);
        self.show_extensions_views(true, window, cx);
    }

    /// The search box changed: `triggerSearch` waits 500 ms while the box has text, an empty box shows at once.
    pub(super) fn edit_extensions_search(&mut self, edit: impl FnOnce(&mut InputBox), window: &mut Window, cx: &mut Context<Self>) {
        let before = self.extensions_viewlet.search.value.clone();
        edit(&mut self.extensions_viewlet.search);
        cx.notify();
        if self.extensions_viewlet.search.value == before {
            return;
        }
        if self.extensions_viewlet.search.value.trim().is_empty() {
            return self.show_extensions_views(true, window, cx);
        }
        self.extensions_viewlet.search_task = Some(cx.spawn_in(window, async move |workbench, cx| {
            cx.background_executor().timer(Duration::from_millis(500)).await;
            let _ = workbench.update_in(cx, |workbench, window, cx| workbench.show_extensions_views(true, window, cx));
        }));
    }

    /// `doSearch`: the views of the normalized query, each with its model.
    pub(super) fn show_extensions_views(&mut self, refresh: bool, window: &mut Window, cx: &mut Context<Self>) {
        let value = model::normalized_query(&self.extensions_viewlet.search.value);
        let descriptors = visible_views(&value, self.extensions_viewlet.has_installed(), self.folder.is_some());
        let unchanged = !refresh && self.extensions_viewlet.panes.iter().map(|pane| pane.descriptor).eq(descriptors.iter().copied());
        if unchanged {
            return;
        }
        self.extensions_viewlet.focused = None;
        self.extensions_viewlet.selected = None;
        self.extensions_viewlet.panes = descriptors
            .into_iter()
            .map(|descriptor| Pane {
                query: descriptor.view_query(&value),
                descriptor,
                extensions: Vec::new(),
                total: 0,
                loading: false,
                message: None,
                expanded: true,
                scroll: UniformListScrollHandle::new(),
                gallery: None,
                pages_loaded: 0,
            })
            .collect();
        for index in 0..self.extensions_viewlet.panes.len() {
            self.load_pane(index, window, cx);
        }
        cx.notify();
    }

    /// `ExtensionsListView.show(query)`: local queries filter the installed extensions, the others ask the gallery.
    fn load_pane(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let pane = &self.extensions_viewlet.panes[index];
        let query = Query::parse(&pane.query);
        let descriptor = pane.descriptor;
        if descriptor.kind == ViewKind::Empty {
            return;
        }
        if model::is_recommendations_query(&query.value) {
            return self.load_recommendations_pane(index, window, cx);
        }
        let is_local = model::is_local_extensions_query(&query.value, &query.sort_by);
        if is_local && !query.value.starts_with("@id:") {
            let local = self.local_extensions();
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_millis() as u64);
            let extensions: Vec<Extension> = model::filter_local(&local, &query, SortBy::parse(&query.sort_by), now).into_iter().cloned().collect();
            let pane = &mut self.extensions_viewlet.panes[index];
            pane.total = extensions.len();
            pane.extensions = extensions;
            pane.message = pane.extensions.is_empty().then(|| (Severity::Info, "No extensions found.".to_owned()));
            let expanded = self.pane_expanded(index);
            self.extensions_viewlet.panes[index].expanded = expanded;
            return;
        }
        self.load_gallery_page(index, 1, window, cx);
    }

    /// Whether a view starts expanded: the user's choice, else collapsed for an empty recommended view of the
    /// default views without installed extensions (`DefaultRecommendedExtensionsView`) and for empty
    /// `OutdatedExtensionsView` and `WorkspaceRecommendedExtensionsView`.
    fn pane_expanded(&self, index: usize) -> bool {
        let pane = &self.extensions_viewlet.panes[index];
        if let Some(expanded) = self.extensions_viewlet.expansion.get(pane.descriptor.id) {
            return *expanded;
        }
        match pane.descriptor.kind {
            ViewKind::DefaultRecommended if !self.extensions_viewlet.has_installed() => !pane.extensions.is_empty(),
            ViewKind::Outdated | ViewKind::WorkspaceRecommended => !pane.extensions.is_empty(),
            _ => true,
        }
    }

    /// `queryGallery` for one page; `@sort:` and the default install count order as upstream.
    fn load_gallery_page(&mut self, index: usize, page: usize, window: &mut Window, cx: &mut Context<Self>) {
        let pane = &mut self.extensions_viewlet.panes[index];
        let query = Query::parse(&pane.query);
        let mut text = query.value.clone();
        let mut sort_by = match query.sort_by.as_str() {
            "installs" => Some(exthost::gallery::SortBy::InstallCount),
            "rating" => Some(exthost::gallery::SortBy::WeightedRating),
            "name" => Some(exthost::gallery::SortBy::Title),
            "publishedDate" => Some(exthost::gallery::SortBy::PublishedDate),
            "updateDate" => Some(exthost::gallery::SortBy::LastUpdatedDate),
            _ => None,
        };
        if model::is_search_popular_query(&text) {
            text = text.replace("@popular", "");
            sort_by = sort_by.or(Some(exthost::gallery::SortBy::InstallCount));
        }
        if model::is_search_recently_published_query(&text) {
            text = text.replace("@recentlyPublished", "");
            sort_by = sort_by.or(Some(exthost::gallery::SortBy::PublishedDate));
        }
        let ids: Vec<String> = text.split_whitespace().filter_map(|word| word.strip_prefix("@id:")).map(str::to_owned).collect();
        pane.loading = true;
        let options = QueryOptions { text: text.trim().chars().take(350).collect(), page_size: Some(PAGE_SIZE), page_number: page, sort_by, sort_order: None, include_pre_release: true };
        let pane_query = pane.query.clone();
        let fetch = cx.background_executor().spawn(async move {
            let gallery = Gallery::open_vsx(crate::VERSION);
            if !ids.is_empty() {
                let infos: Vec<ExtensionInfo> = ids.iter().map(|id| ExtensionInfo { id: id.clone(), uuid: None, version: None, pre_release: true }).collect();
                return gallery.get_extensions(&infos, false).map(|found| {
                    let total = found.len();
                    (found, total)
                });
            }
            gallery.query(&options)
        });
        cx.spawn_in(window, async move |workbench, cx| {
            let result = fetch.await;
            let _ = workbench.update(cx, |workbench, cx| {
                let local = workbench.local_extensions();
                let Some(index) = workbench.extensions_viewlet.panes.iter().position(|pane| pane.query == pane_query) else { return };
                let merged: Vec<Extension> = match &result {
                    Ok((found, _)) => found.iter().map(|gallery| workbench.gallery_extension(gallery, &local)).collect(),
                    Err(_) => Vec::new(),
                };
                let expanded = workbench.pane_expanded(index);
                let pane = &mut workbench.extensions_viewlet.panes[index];
                pane.loading = false;
                pane.expanded = expanded;
                match result {
                    Ok((found, total)) => {
                        pane.total = total;
                        pane.gallery.get_or_insert_with(Vec::new).extend(found);
                        pane.extensions.extend(merged);
                        pane.pages_loaded = page;
                        pane.message = pane.extensions.is_empty().then(|| (Severity::Info, "No extensions found.".to_owned()));
                    }
                    // extensionsViews.ts `getMessage`: a failed gallery request reads as offline.
                    Err(error) => {
                        eprintln!("[extensions] gallery query failed: {error:#}");
                        pane.message = Some((Severity::Warning, "Unable to search the Marketplace when offline, please check your network connection.".into()));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The pager: the next gallery page once the list scrolls near its end.
    pub(super) fn load_next_extensions_page(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let pane = &self.extensions_viewlet.panes[index];
        if pane.loading || pane.gallery.is_none() || pane.extensions.len() >= pane.total {
            return;
        }
        let next = pane.pages_loaded + 1;
        self.load_gallery_page(index, next, window, cx);
    }

    /// `queryRecommendations`: the ids a recommendations query shows, in order. Code - OSS's product.json has no
    /// `extensionRecommendations`, `keymapExtensionTips`, `languageExtensionTips`, `exeBasedExtensionTips` or
    /// `configBasedExtensionTips`, so only the workspace's own recommendations exist.
    fn recommendations_for(&self, query: &str) -> Vec<String> {
        let lower = query.to_lowercase();
        if lower.contains("@recommended:workspace") {
            return self.workspace_recommendations();
        }
        if lower.contains("@recommended:all") {
            let local = self.local_extensions();
            return self.workspace_recommendations().into_iter().filter(|id| !local.iter().any(|extension| extension.identifier().id.eq_ignore_ascii_case(id))).collect();
        }
        if model::is_search_recommended_extensions_query(query) {
            let value = query.replace("@recommended", "").trim().to_lowercase();
            return self.workspace_recommendations().into_iter().filter(|id| id.contains(&value)).collect();
        }
        Vec::new()
    }

    /// `getWorkspaceRecommendations`: the folders' and the workspace file's extensions.json, minus the ignored ids.
    pub(super) fn workspace_recommendations(&self) -> Vec<String> {
        let configs = self.extensions_configs();
        let ignored = recommendations::parse_ignored(self.profile_storage.lock().unwrap().get(recommendations::IGNORED_RECOMMENDATIONS_KEY));
        recommendations::allowed_recommendations(&recommendations::workspace_recommendations(&configs), &ignored)
    }

    /// `getExtensionsConfigs`: the workspace file's first, then each folder's.
    fn extensions_configs(&self) -> Vec<recommendations::ExtensionsConfig> {
        let settings = self.settings.lock().unwrap();
        let Some(workspace) = &settings.workspace else { return Vec::new() };
        let mut configs: Vec<_> = workspace.file.as_deref().and_then(recommendations::read_workspace_file_config).into_iter().collect();
        configs.extend(workspace.folders.iter().map(|folder| recommendations::read_folder_config(&folder.path)));
        configs
    }

    /// The recommendation views: the recommended ids looked up on Open VSX, in the recommendations' order.
    fn load_recommendations_pane(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.extensions_viewlet.panes[index].query.clone();
        let ids = self.recommendations_for(&query);
        if ids.is_empty() {
            let expanded = self.pane_expanded(index);
            let pane = &mut self.extensions_viewlet.panes[index];
            pane.expanded = expanded;
            pane.message = Some((Severity::Info, "No extensions found.".into()));
            return;
        }
        let pane = &mut self.extensions_viewlet.panes[index];
        pane.loading = true;
        let pane_query = pane.query.clone();
        let fetch = cx.background_executor().spawn(async move {
            let infos: Vec<ExtensionInfo> = ids.iter().map(|id| ExtensionInfo { id: id.clone(), uuid: None, version: None, pre_release: false }).collect();
            let found = Gallery::open_vsx(crate::VERSION).get_extensions(&infos, true)?;
            // The view lists them in the recommendations' order.
            Ok::<_, anyhow::Error>(ids.iter().filter_map(|id| found.iter().find(|extension| extension.id.eq_ignore_ascii_case(id)).cloned()).collect::<Vec<_>>())
        });
        cx.spawn_in(window, async move |workbench, cx| {
            let result = fetch.await;
            let _ = workbench.update(cx, |workbench, cx| {
                let local = workbench.local_extensions();
                let Some(index) = workbench.extensions_viewlet.panes.iter().position(|pane| pane.query == pane_query) else { return };
                let found = result.unwrap_or_else(|error| {
                    eprintln!("[extensions] recommendations query failed: {error:#}");
                    Vec::new()
                });
                let mut found = found;
                // `getAllRecommendationsModel` shows at most 8.
                if pane_query.to_lowercase().contains("@recommended:all") {
                    found.truncate(8);
                }
                let merged = found.iter().map(|gallery| workbench.gallery_extension(gallery, &local)).collect();
                let pane = &mut workbench.extensions_viewlet.panes[index];
                pane.loading = false;
                pane.total = found.len();
                pane.gallery = Some(found);
                pane.extensions = merged;
                pane.message = pane.extensions.is_empty().then(|| (Severity::Info, "No extensions found.".to_owned()));
                let expanded = workbench.pane_expanded(index);
                workbench.extensions_viewlet.panes[index].expanded = expanded;
                cx.notify();
            });
        })
        .detach();
    }

    /// After an install, uninstall or enablement change: the local views filter again, the gallery views merge
    /// their results with the new installed state (`onDidChangeModel`).
    pub(super) fn refresh_extensions_views(&mut self, cx: &mut Context<Self>) {
        self.rescan_installed_extensions();
        let local = self.local_extensions();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_millis() as u64);
        for index in 0..self.extensions_viewlet.panes.len() {
            let pane = &self.extensions_viewlet.panes[index];
            let extensions: Vec<Extension> = match &pane.gallery {
                Some(gallery) => gallery.iter().map(|gallery| self.gallery_extension(gallery, &local)).collect(),
                None if pane.descriptor.kind == ViewKind::Empty => Vec::new(),
                None => {
                    let query = Query::parse(&pane.query);
                    model::filter_local(&local, &query, SortBy::parse(&query.sort_by), now).into_iter().cloned().collect()
                }
            };
            let pane = &mut self.extensions_viewlet.panes[index];
            pane.total = pane.total.max(extensions.len());
            pane.message = extensions.is_empty().then(|| (Severity::Info, "No extensions found.".to_owned()));
            pane.extensions = extensions;
        }
        self.refresh_extension_editors();
        cx.notify();
    }

    /// `setEnablement` and the extension service's delta: enabled extensions start, disabled ones stop unless they
    /// already activated.
    pub(super) fn set_extension_enablement(&mut self, ids: &[String], state: EnablementState, cx: &mut Context<Self>) -> Outcome {
        let viewlet = &self.extensions_viewlet;
        let inputs = enablement_inputs(&viewlet.installed, &viewlet.disabled_in_env, viewlet.disable_all_in_env, &viewlet.development);
        let changed: Result<Vec<usize>, String> = {
            let mut profile = self.profile_storage.lock().unwrap();
            let mut workspace = self.workspace_storage.lock().unwrap();
            let mut enablement = Enablement { profile: &mut profile, workspace: self.folder.is_some().then_some(&mut *workspace) };
            let mut changed = Vec::new();
            let mut result = Ok(());
            for id in ids {
                let Some(index) = viewlet.installed.iter().position(|extension| extension.id.eq_ignore_ascii_case(id)) else {
                    result = Err(format!("Extension '{id}' is not installed."));
                    break;
                };
                match enablement.set_enablement(&inputs, index, state) {
                    Ok(indexes) => changed.extend(indexes),
                    Err(error) => {
                        result = Err(error);
                        break;
                    }
                }
            }
            result.map(|()| changed)
        };
        let changed = match changed {
            Ok(changed) => changed,
            Err(error) => {
                self.notify_error(error.clone());
                return Outcome::error(error);
            }
        };
        if changed.is_empty() {
            return Outcome::done();
        }
        self.apply_enablement(cx)
    }

    /// Starts what is enabled and not running, stops what runs and is disabled.
    fn apply_enablement(&mut self, cx: &mut Context<Self>) -> Outcome {
        let local = self.local_extensions();
        let to_add: Vec<ScannedExtension> = local.iter().filter(|extension| extension.enablement.is_enabled() && !extension.running).filter_map(|extension| extension.local.clone()).collect();
        let to_remove: Vec<String> = local.iter().filter(|extension| !extension.enablement.is_enabled() && extension.running).map(|extension| extension.identifier().id).collect();
        self.refresh_extensions_views(cx);
        if to_add.is_empty() && to_remove.is_empty() {
            return Outcome::done();
        }
        self.run_extension_jobs(vec![Job::Delta(to_add, to_remove)], None, cx, |error| error)
    }

    /// `_onDidUninstallExtension`: the uninstalled extension leaves the enablement lists.
    pub(super) fn reset_extension_enablement(&mut self, id: &str) {
        let mut profile = self.profile_storage.lock().unwrap();
        let mut workspace = self.workspace_storage.lock().unwrap();
        let mut enablement = Enablement { profile: &mut profile, workspace: self.folder.is_some().then_some(&mut *workspace) };
        enablement.reset(&Identifier { id: id.to_owned(), uuid: None });
    }

    fn set_all_enablement(&mut self, state: EnablementState, cx: &mut Context<Self>) -> Outcome {
        let ids: Vec<String> = self.local_extensions().iter().filter(|extension| !extension.is_builtin() && !extension.is_language_pack()).map(|extension| extension.identifier().id).collect();
        self.set_extension_enablement(&ids, state, cx)
    }

    /// `extensionsWorkbenchService.outdated`.
    pub fn outdated_extensions(&self) -> Vec<Extension> {
        self.local_extensions().into_iter().filter(|extension| extension.outdated()).collect()
    }

    /// `install(extension)` of the gallery's latest version over the installed one.
    pub(super) fn update_extension(&mut self, extension: &Extension, cx: &mut Context<Self>) -> Outcome {
        let pre_release = extension.pre_release();
        let id = extension.identifier().id;
        self.install_from_gallery(id, None, pre_release, cx)
    }

    /// `eventuallyCheckForUpdates`: now, then every 12 hours while `extensions.autoCheckUpdates` is on.
    fn eventually_check_for_updates(&mut self, immediate: bool, cx: &mut Context<Self>) {
        let delay = if immediate { Duration::ZERO } else { UPDATES_CHECK_INTERVAL };
        self.extensions_viewlet.update_check = Some(cx.spawn(async move |workbench, cx| {
            cx.background_executor().timer(delay).await;
            let _ = workbench.update(cx, |workbench, cx| {
                if workbench.setting("extensions.autoCheckUpdates") != false {
                    workbench.check_for_updates(cx);
                }
                workbench.eventually_check_for_updates(false, cx);
            });
        }));
    }

    /// `checkForUpdates`: the gallery's latest compatible version of each installed user extension; then
    /// `autoUpdateExtensions` when `extensions.autoUpdate` is on.
    pub(super) fn check_for_updates(&mut self, cx: &mut Context<Self>) {
        drop(self.check_for_updates_task(cx));
    }

    fn check_for_updates_task(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let infos: Vec<ExtensionInfo> = self
            .extensions_viewlet
            .installed
            .iter()
            .filter(|extension| extension.description["isBuiltin"] != true)
            .map(|extension| ExtensionInfo {
                id: extension.id.clone(),
                uuid: extension.description["uuid"].as_str().map(str::to_owned),
                version: None,
                pre_release: extension.description["preRelease"] == true,
            })
            .collect();
        if infos.is_empty() {
            return Task::ready(());
        }
        let fetch = cx.background_executor().spawn(async move { Gallery::open_vsx(crate::VERSION).get_extensions(&infos, true) });
        cx.spawn(async move |workbench, cx| {
            let found = match fetch.await {
                Ok(found) => found,
                Err(error) => return eprintln!("[extensions] checking for updates: {error:#}"),
            };
            let _ = workbench.update(cx, |workbench, cx| {
                for gallery in found {
                    workbench.extensions_viewlet.gallery.insert(gallery.id.to_lowercase(), gallery);
                }
                workbench.refresh_extensions_views(cx);
                workbench.auto_update_extensions(cx);
            });
        })
    }

    /// `CheckForUpdatesAction`: the check, then a notification of what it found.
    fn check_for_updates_and_notify(&mut self, cx: &mut Context<Self>) {
        let check = self.check_for_updates_task(cx);
        cx.spawn(async move |workbench, cx| {
            check.await;
            let _ = workbench.update(cx, |workbench, cx| {
                let outdated = workbench.outdated_extensions();
                let message = match outdated.len() {
                    0 => "All extensions are up to date.".to_owned(),
                    1 => "1 extension has an update available.".to_owned(),
                    count => format!("{count} extensions have updates available."),
                };
                drop(workbench.prompt_notification(Severity::Info, message, &[]));
                cx.notify();
            });
        })
        .detach();
    }

    /// `autoUpdateExtensions`: `shouldAutoUpdateExtension` with `extensions.autoUpdate`, the per extension choice,
    /// and `extensions.autoUpdateDelay` hours after the gallery published the version.
    fn auto_update_extensions(&mut self, cx: &mut Context<Self>) {
        let on = self.auto_update_on();
        let selected = self.auto_update_selection();
        let delay_hours = self.setting("extensions.autoUpdateDelay").as_f64().unwrap_or(12.);
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0., |elapsed| elapsed.as_millis() as f64);
        for extension in self.outdated_extensions() {
            let id = extension.identifier().id.to_lowercase();
            let wanted = match on {
                true => !selected.contains(&format!("-{id}")) && !matches!(extension.enablement, EnablementState::DisabledGlobally | EnablementState::DisabledWorkspace),
                false => selected.contains(&id),
            };
            let published = extension.gallery.as_ref().and_then(|gallery| iso_millis(&gallery.last_updated)).unwrap_or(0.);
            let delayed = published > 0. && now - published < delay_hours * 3_600_000.;
            if wanted && !delayed {
                drop(self.update_extension(&extension, cx));
            }
        }
    }

    /// The stored per extension choices (`extensions.autoUpdate` storage key): ids to update, `-id` to skip.
    fn auto_update_selection(&self) -> Vec<String> {
        let storage = self.profile_storage.lock().unwrap();
        storage.get("extensions.autoUpdate").and_then(|text| serde_json::from_str::<Vec<String>>(text).ok()).unwrap_or_default()
    }

    /// `ToggleAutoUpdateForExtensionAction`: with auto update on, `-id` marks it skipped; off, `id` marks it updated.
    fn toggle_auto_update_for(&mut self, id: &str, cx: &mut Context<Self>) {
        let mut selected = self.auto_update_selection();
        let on = self.auto_update_on();
        let entry = if on { format!("-{id}") } else { id.to_owned() };
        match selected.iter().position(|selected| *selected == entry) {
            Some(index) => drop(selected.remove(index)),
            None => selected.push(entry),
        }
        let mut storage = self.profile_storage.lock().unwrap();
        match selected.is_empty() {
            true => storage.delete("extensions.autoUpdate"),
            false => storage.set("extensions.autoUpdate", json!(selected).to_string()),
        }
        drop(storage);
        cx.notify();
    }

    /// `isAutoUpdateEnabled`: `extensions.autoUpdate` is neither false, "off" nor "onlySelectedExtensions".
    pub(super) fn auto_update_on(&self) -> bool {
        let auto_update = self.setting("extensions.autoUpdate");
        !matches!(auto_update.as_str(), Some("off") | Some("onlySelectedExtensions")) && auto_update != false
    }

    /// `isAutoUpdateEnabledFor(extension)`.
    pub(super) fn is_auto_update_enabled_for(&self, extension: &Extension) -> bool {
        let on = self.auto_update_on();
        let id = extension.identifier().id.to_lowercase();
        let selected = self.auto_update_selection();
        match on {
            true => !selected.contains(&format!("-{id}")),
            false => selected.contains(&id),
        }
    }

    /// StatusUpdater: the outdated enabled extensions plus those needing a restart, and its hover text.
    pub(super) fn extensions_badge(&self) -> Option<(usize, String)> {
        let local = self.local_extensions();
        let auto_restart = self.setting("extensions.autoRestart") == true;
        let action_required = if auto_restart { 0 } else { local.iter().filter(|extension| extension.restart_required && !extension.is_builtin()).count() };
        let outdated = local.iter().filter(|extension| extension.outdated() && extension.enablement.is_enabled() && !extension.restart_required).count();
        let count = outdated + action_required;
        if count == 0 {
            return None;
        }
        let mut message = String::new();
        if outdated > 0 {
            message += &if outdated == 1 { format!("{outdated} requires update") } else { format!("{outdated} require update") };
        }
        if outdated > 0 && action_required > 0 {
            message += ", ";
        }
        if action_required > 0 {
            message += &if action_required == 1 { format!("{action_required} requires restart") } else { format!("{action_required} require restart") };
        }
        Some((count, message))
    }

    /// Downloads a gallery icon once; the row shows it when it arrives.
    pub(super) fn request_extension_icon(&mut self, url: &str, cx: &mut Context<Self>) {
        if self.extensions_viewlet.icons.contains_key(url) {
            return;
        }
        self.extensions_viewlet.icons.insert(url.to_owned(), None);
        let url = url.to_owned();
        let fetch = {
            let url = url.clone();
            cx.background_executor().spawn(async move {
                let response = exthost::request::request(&exthost::request::Request { url: &url, method: "GET", headers: &[], body: None, timeout: Duration::from_secs(30) })?;
                anyhow::ensure!(response.status == 200, "status {}", response.status);
                Ok(response.body)
            })
        };
        cx.spawn(async move |workbench, cx| {
            let bytes: anyhow::Result<Vec<u8>> = fetch.await;
            let _ = workbench.update(cx, |workbench, cx| {
                let image = bytes.ok().and_then(|bytes| image_format(&bytes).map(|format| Arc::new(Image::from_bytes(format, bytes))));
                workbench.extensions_viewlet.icons.insert(url, image);
                cx.notify();
            });
        })
        .detach();
    }

    /// Writes a user setting from a command (the auto update actions).
    fn write_user_setting_value(&mut self, key: &str, value: Value, cx: &mut Context<Self>) -> Outcome {
        let mut settings = self.settings.lock().unwrap();
        match settings.update_value_derived(key, Some(&value)) {
            Ok(change) if !change.is_empty() => self.host.send_configuration_change(&settings, &change),
            Ok(_) => {}
            Err(error) => return Outcome::error(error.to_string()),
        }
        drop(settings);
        cx.notify();
        Outcome::done()
    }
}

/// The image format by its magic bytes; gallery icons are PNG, JPEG or SVG.
fn image_format(bytes: &[u8]) -> Option<ImageFormat> {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => Some(ImageFormat::Png),
        [0xFF, 0xD8, ..] => Some(ImageFormat::Jpeg),
        [b'G', b'I', b'F', ..] => Some(ImageFormat::Gif),
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => Some(ImageFormat::Webp),
        _ if String::from_utf8_lossy(&bytes[..bytes.len().min(512)]).contains("<svg") => Some(ImageFormat::Svg),
        _ => None,
    }
}

/// `Date.parse` of the gallery's ISO 8601 UTC dates (`2024-01-31T12:00:00.000Z`), in milliseconds.
pub fn iso_millis(date: &str) -> Option<f64> {
    let (day, time) = date.split_once('T')?;
    let mut day_parts = day.split('-').map(|part| part.parse::<i64>().ok());
    let (year, month, day) = (day_parts.next()??, day_parts.next()??, day_parts.next()??);
    let time = time.trim_end_matches('Z');
    let mut time_parts = time.split(':');
    let hours: f64 = time_parts.next()?.parse().ok()?;
    let minutes: f64 = time_parts.next()?.parse().ok()?;
    let seconds: f64 = time_parts.next().unwrap_or("0").parse().ok()?;
    // Days from civil (Howard Hinnant's algorithm).
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146097 + day_of_era - 719468;
    Some(days as f64 * 86_400_000. + hours * 3_600_000. + minutes * 60_000. + seconds * 1000.)
}

/// `CopyExtensionAction`: name, id, description, version, publisher and the gallery link.
fn copy_text(extension: &Extension) -> String {
    let mut lines = vec![
        format!("Name: {}", extension.display_name()),
        format!("Id: {}", extension.identifier().id),
        format!("Description: {}", extension.description()),
        format!("Version: {}", extension.version()),
        format!("Publisher: {}", extension.publisher_display_name()),
    ];
    if let Some(gallery) = &extension.gallery {
        lines.push(format!("Open VSX Link: {}", gallery.detail_link));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests;
