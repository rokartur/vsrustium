//! gettingStarted.ts, startupPage.ts and gettingStarted.contribution.ts: the Welcome page editor
//! (media/gettingStarted.css), its categories and details slides, the step media of gettingStartedDetailsRenderer.ts
//! in a webview, the walkthrough commands, and the progress events gettingStartedService.ts listens to.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use futures::StreamExt;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui::{AnyElement, ClickEvent, Context, Div, FocusHandle, FontWeight, KeyDownEvent, Pixels, Role, SharedString, Size, Stateful, Toggled, Window, canvas, div, img, prelude::*, px, relative};
use regex::Regex;
use serde_json::{Value, json};

use super::Workbench;
use super::markdown_document::Sanitizer;
use super::opener::{OpenOptions, Target};
use super::search_view::SideBarView;
use crate::commands::Outcome;
use crate::editor::TextDocument;
use crate::quick_input::{Entry, Item, Kind, QuickInput};
use crate::recently_opened;
use crate::theme::ThemeKind;
use crate::walkthroughs::{self, Icon, LinkedText, Media, Node, StartupEditor, ThemePaths, Walkthrough, Walkthroughs};
use crate::webview::{self, Webview, WebviewEvent};

/// `Schemas.walkThrough`, the scheme of `GettingStartedInput.RESOURCE` and of the Editor Playground.
pub const WALKTHROUGH_SCHEME: &str = "walkThrough";
/// `GettingStartedInput.RESOURCE`'s authority, the document path of the Welcome page.
const GETTING_STARTED_PATH: &str = "vscode_getting_started_page";
const STARTUP_EDITOR_KEY: &str = "workbench.startupEditor";
/// startupPage.ts `restoreWalkthroughsConfigurationKey`.
const RESTORE_KEY: &str = "workbench.welcomePage.restorableWalkthroughs";
/// `UNKNOWN_EMPTY_WINDOW_WORKSPACE.id`.
const EMPTY_WINDOW: &str = "empty-window";
const WEBVIEW_ID: &str = "welcome-page-media";
const WEBVIEW_ORIGIN: &str = "5f4c8d2e-1b3a-4e6f-9a7d-welcomepage01";

#[derive(Default)]
pub struct WelcomePage {
    /// The walkthroughs service, loaded with the profile storage by `run_startup_page`.
    model: Option<Walkthroughs>,
    /// `GettingStartedInput.selectedCategory` and `selectedStep`: the details slide when a category is selected.
    category: Option<String>,
    step: Option<String>,
    /// `prevWalkthrough`: the walkthrough Next Section left, which Go Back returns to.
    previous: Option<String>,
    /// `setSlide('details', firstLaunch)`: the back button reads "Welcome".
    first_launch: bool,
    /// Events from `execute_command`, applied in the next frame where the window's context keys are at hand.
    pending_events: Vec<String>,
    /// The `onSettingChanged:` keys with their last values.
    settings: Vec<(String, Value)>,
    contexts: Vec<(String, bool)>,
    extension_ids: Vec<String>,
    visible_view: Option<&'static str>,
    focus: Option<FocusHandle>,
    /// The editor area the page laid out in last, for gettingStarted.ts `layout` (the `*-constrained` classes).
    size: Rc<Cell<Size<Pixels>>>,
    media: Option<MediaWebview>,
    events: Option<UnboundedSender<WebviewEvent>>,
}

/// The step media webview and what it shows.
struct MediaWebview {
    step: String,
    theme: String,
    webview: Webview,
    /// The `checked-on` expressions of a markdown media and the ones last posted as holding.
    checked_on: Vec<String>,
    enabled: Option<Vec<String>>,
}

impl Workbench {
    pub(super) fn register_welcome_page_commands(&mut self) {
        self.commands.register("workbench.action.openWalkthrough", |workbench, args, window, cx| {
            workbench.open_walkthrough(args.first(), window, cx);
            Outcome::done()
        });
        self.commands.register("welcome.goBack", |workbench, _, window, cx| {
            if workbench.welcome_page_active() {
                workbench.welcome_escape(window, cx);
            }
            Outcome::done()
        });
        self.commands.register("walkthroughs.selectStep", |workbench, args, window, cx| {
            let Some(id) = args.first().and_then(Value::as_str) else { return Outcome::done() };
            match workbench.welcome_page_active() {
                true => workbench.select_step_loose(id, window, cx),
                false => eprintln!("Cannot run walkthroughs.selectStep outside of walkthrough context"),
            }
            Outcome::done()
        });
        self.commands.register("welcome.markStepComplete", |workbench, args, window, cx| {
            if let Some(id) = args.first().and_then(Value::as_str) {
                workbench.ensure_walkthroughs(window);
                if workbench.walkthroughs_mut().progress_step(id) {
                    workbench.steps_progressed(vec![id.to_owned()], window);
                }
                cx.notify();
            }
            Outcome::done()
        });
        self.commands.register("welcome.markStepIncomplete", |workbench, args, window, cx| {
            if let Some(id) = args.first().and_then(Value::as_str) {
                workbench.ensure_walkthroughs(window);
                workbench.deprogress_step(id);
                cx.notify();
            }
            Outcome::done()
        });
        self.commands.register("welcome.showAllWalkthroughs", |workbench, _, window, cx| {
            workbench.show_walkthrough_picker(Some("Select a walkthrough to open"), None, window, cx);
            Outcome::done()
        });
        self.commands.register("resetGettingStartedProgress", |workbench, _, window, cx| {
            workbench.ensure_walkthroughs(window);
            {
                let mut storage = workbench.profile_storage.lock().unwrap();
                storage.set(walkthroughs::HIDDEN_KEY, "[]".into());
                storage.set(walkthroughs::METADATA_KEY, "[]".into());
            }
            for id in workbench.walkthroughs_mut().progressed_steps() {
                workbench.deprogress_step(&id);
            }
            cx.notify();
            Outcome::done()
        });
    }

    /// startupPage.ts `StartupPageRunnerContribution.run`, once the startup editors are open.
    pub(crate) fn run_startup_page(&mut self, skip_welcome: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_walkthroughs(window);
        if self.open_restored_walkthrough(window, cx) {
            return;
        }
        let (value, user_set) = {
            let settings = self.settings.lock().unwrap();
            let inspected = settings.configuration.inspect(STARTUP_EDITOR_KEY, &Default::default());
            (inspected.value.and_then(|value| value.as_str().map(str::to_owned)).unwrap_or_default(), inspected.user_local.value.is_some() || inspected.workspace.value.is_some())
        };
        let old_enabled = self.setting("workbench.welcome.enabled");
        let decision = walkthroughs::startup_editor(&value, user_set, Some(&old_enabled), skip_welcome, self.workspace().is_none(), self.editors.active().is_some());
        match decision {
            Some(StartupEditor::WelcomePage) => self.open_welcome_page(true, window, cx),
            Some(StartupEditor::Readme) => self.open_readme(window, cx),
            // shortcut: the port has no terminal editors (`workbench.action.createTerminalEditor`); port with them.
            Some(StartupEditor::Terminal) | None => {}
        }
    }

    /// startupPage.ts `tryOpenWalkthroughForFolder`: a walkthrough a step left to open a folder continues there.
    fn open_restored_walkthrough(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let stored = self.profile_storage.lock().unwrap().get(RESTORE_KEY).map(str::to_owned);
        let Some(restore) = stored.and_then(|text| serde_json::from_str::<Value>(&text).ok()) else { return false };
        let first_folder = self.workspace().and_then(|workspace| workspace.folders.first()).map(|folder| crate::editor_history::file_uri_string(&folder.path));
        let folder = restore["folder"].as_str().unwrap_or_default();
        if folder != EMPTY_WINDOW && Some(folder) != first_folder.as_deref() {
            return false;
        }
        self.open_welcome_page(true, window, cx);
        if let Some(category) = restore["category"].as_str() {
            self.scroll_to_category(category, restore["step"].as_str().map(str::to_owned), window);
        }
        self.profile_storage.lock().unwrap().delete(RESTORE_KEY);
        true
    }

    /// startupPage.ts `openReadme`: markdown readmes in the preview, others as editors, else the Welcome page.
    fn open_readme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let folders: Vec<PathBuf> = self.workspace().map(|workspace| workspace.folders.iter().map(|folder| folder.path.clone()).collect()).unwrap_or_default();
        let mut readmes = Vec::new();
        for folder in folders {
            let mut names: Vec<String> = std::fs::read_dir(&folder).into_iter().flatten().flatten().map(|entry| entry.file_name().to_string_lossy().into_owned()).collect();
            names.sort();
            let name = names.iter().find(|name| name.to_lowercase() == "readme.md").or_else(|| names.iter().find(|name| name.to_lowercase().starts_with("readme")));
            readmes.extend(name.map(|name| folder.join(name)));
        }
        if readmes.is_empty() {
            return self.open_welcome_page(true, window, cx);
        }
        let (markdown, others): (Vec<PathBuf>, Vec<PathBuf>) = readmes.into_iter().partition(|readme| readme.to_string_lossy().to_lowercase().ends_with(".md"));
        if !markdown.is_empty() {
            let uris: Vec<Value> = markdown.iter().map(|path| json!(crate::editor_history::file_uri_string(path))).collect();
            drop(self.execute_command("markdown.showPreview", vec![Value::Null, Value::Array(uris), json!({ "locked": true })], window, cx));
        }
        for readme in others {
            self.open_file(readme, true, window, cx);
        }
    }

    /// Runs each render: the completion events of commands, settings, context keys, views and installs.
    pub(super) fn welcome_page_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.welcome.model.is_none() {
            return;
        }
        let mut events = std::mem::take(&mut self.welcome.pending_events);
        events.extend(self.track_walkthrough_extensions(window, cx));
        for index in 0..self.welcome.settings.len() {
            let value = self.setting(&self.welcome.settings[index].0);
            if value != self.welcome.settings[index].1 {
                events.push(format!("onSettingChanged:{}", self.welcome.settings[index].0));
                self.welcome.settings[index].1 = value;
            }
        }
        // shortcut: `onContext:` expressions read the `setContext` keys (where extensions set theirs, as git's `git.state`), not the workbench's own.
        for (expression, held) in &mut self.welcome.contexts {
            let holds = walkthroughs::when_holds(Some(expression), &self.context_keys);
            if holds && !*held {
                events.push(format!("onContext:{expression}"));
            }
            *held = holds;
        }
        let view = match self.side_bar_view {
            _ if !self.side_bar_visible() => None,
            SideBarView::Contributed(id) => self.container_views(id).first().copied(),
            view => side_bar_view_id(view),
        };
        if view != self.welcome.visible_view {
            self.welcome.visible_view = view;
            events.extend(view.map(|view| format!("onView:{view}")));
        }
        let mut progressed = Vec::new();
        for event in events {
            progressed.extend(self.walkthroughs_mut().progress_by_event(&event));
        }
        if !progressed.is_empty() {
            self.steps_progressed(progressed, window);
        }
        self.update_welcome_media(window, cx);
    }

    /// `onCommand:` completion events (`ICommandService.onDidExecuteCommand`).
    pub(crate) fn welcome_command_executed(&mut self, id: &str) {
        self.welcome.pending_events.push(format!("onCommand:{id}"));
    }

    /// `GettingStartedInput.getName`.
    pub(super) fn walkthrough_editor_name(&self, path: &Path) -> String {
        if path != Path::new(GETTING_STARTED_PATH) {
            return path.to_string_lossy().into_owned();
        }
        let title = self.welcome.category.as_deref().and_then(|id| self.welcome.model.as_ref()?.get(id)).map(|walkthrough| walkthrough.page_title.clone());
        match title {
            Some(title) => format!("Walkthrough: {title}"),
            None => "Welcome".into(),
        }
    }

    pub(super) fn walkthrough_editor_body(&self, path: &Path, window: &mut Window, cx: &mut Context<Self>) -> Div {
        if path != Path::new(GETTING_STARTED_PATH) || self.welcome.model.is_none() {
            return div().flex_1();
        }
        let size = self.welcome.size.clone();
        let measure = canvas(
            move |bounds, window, _| {
                if size.get() != bounds.size {
                    size.set(bounds.size);
                    window.refresh();
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        let slide = match self.welcome.category.is_some() {
            true => self.welcome_details(window, cx),
            false => self.welcome_categories(window, cx),
        };
        let mut page = div()
            .id("welcome-page")
            .flex_1()
            .min_h_0()
            .relative()
            .overflow_hidden()
            .text_size(px(13.))
            .line_height(px(16.))
            .bg(self.color("welcomePage.background"))
            .text_color(self.color("foreground"))
            .child(measure)
            .child(slide)
            .on_key_down(cx.listener(|workbench, event: &KeyDownEvent, window, cx| workbench.welcome_key_down(event, window, cx)));
        if let Some(focus) = &self.welcome.focus {
            page = page.track_focus(focus);
        }
        div().flex_1().min_h_0().flex().child(page)
    }
}

// The page's state and commands.
impl Workbench {
    fn walkthroughs_mut(&mut self) -> &mut Walkthroughs {
        self.welcome.model.as_mut().expect("walkthroughs load before the page or its commands run")
    }

    fn walkthroughs(&self) -> &Walkthroughs {
        self.welcome.model.as_ref().expect("walkthroughs load before the page draws")
    }

    /// `WalkthroughsService` construction: progress, metadata, the built-in and extension walkthroughs.
    fn ensure_walkthroughs(&mut self, window: &Window) {
        if self.welcome.model.is_some() {
            return;
        }
        let (progress, metadata) = {
            let storage = self.profile_storage.lock().unwrap();
            (storage.get(walkthroughs::PROGRESS_KEY).map(str::to_owned), storage.get(walkthroughs::METADATA_KEY).map(str::to_owned))
        };
        let mut model = Walkthroughs::load(progress.as_deref(), metadata.as_deref());
        let context = self.welcome_context(window);
        for walkthrough in walkthroughs::built_in(&self.app_root) {
            model.register(walkthrough, &context);
        }
        let now = now_millis();
        for extension in &self.extensions {
            for walkthrough in walkthroughs::from_extension(&extension.id, &extension.location, &extension.description) {
                model.note_seen(&walkthrough, now);
                model.register(walkthrough, &context);
            }
            self.welcome.pending_events.push(format!("extensionInstalled:{}", extension.id.to_lowercase()));
        }
        self.profile_storage.lock().unwrap().set(walkthroughs::METADATA_KEY, model.metadata_json());
        self.welcome.extension_ids = self.extensions.iter().map(|extension| extension.id.clone()).collect();
        self.welcome.settings = model.listened("onSettingChanged").into_iter().map(|key| (key.clone(), self.setting(&key))).collect();
        self.welcome.contexts = model.listened("onContext").into_iter().map(|expression| (expression.clone(), walkthroughs::when_holds(Some(&expression), &self.context_keys))).collect();
        self.welcome.model = Some(model);
    }

    /// `onDidInstallExtensions` and the extension point's added and removed walkthroughs. A newly installed
    /// extension's walkthrough opens with `workbench.welcomePage.walkthroughs.openOnInstall`.
    fn track_walkthrough_extensions(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Vec<String> {
        if self.extensions.len() == self.welcome.extension_ids.len() && self.extensions.iter().zip(&self.welcome.extension_ids).all(|(extension, id)| extension.id == *id) {
            return Vec::new();
        }
        let context = self.welcome_context(window);
        let now = now_millis();
        let mut events = Vec::new();
        let mut to_open = None;
        let removed: Vec<String> = self.welcome.extension_ids.iter().filter(|id| !self.extensions.iter().any(|extension| extension.id == **id)).cloned().collect();
        let added: Vec<_> = self.extensions.iter().filter(|extension| !self.welcome.extension_ids.contains(&extension.id)).cloned().collect();
        let model = self.welcome.model.as_mut().expect("loaded");
        for id in removed {
            let prefix = format!("{id}#");
            let ids: Vec<String> = model.walkthroughs.iter().filter(|walkthrough| walkthrough.id.starts_with(&prefix)).map(|walkthrough| walkthrough.id.clone()).collect();
            for walkthrough in ids {
                model.unregister(&walkthrough);
            }
        }
        for extension in added {
            for walkthrough in walkthroughs::from_extension(&extension.id, &extension.location, &extension.description) {
                let newly_installed = model.note_seen(&walkthrough, now);
                if newly_installed && to_open.is_none() && walkthroughs::when_holds(walkthrough.when.as_deref(), &context) {
                    to_open = Some(walkthrough.id.clone());
                }
                model.register(walkthrough, &context);
            }
            events.push(format!("extensionInstalled:{}", extension.id.to_lowercase()));
        }
        let metadata = model.metadata_json();
        self.profile_storage.lock().unwrap().set(walkthroughs::METADATA_KEY, metadata);
        self.welcome.extension_ids = self.extensions.iter().map(|extension| extension.id.clone()).collect();
        let open_on_install = self.setting("workbench.welcomePage.walkthroughs.openOnInstall") != json!(false) && window.is_window_active();
        if let Some(category) = to_open.filter(|_| open_on_install) {
            cx.defer_in(window, move |workbench, window, cx| {
                if workbench.welcome_page_active() {
                    drop(workbench.execute_command("workbench.action.keepEditor", Vec::new(), window, cx));
                }
                drop(workbench.execute_command("workbench.action.openWalkthrough", vec![json!(category)], window, cx));
            });
        }
        events
    }

    /// The window's context keys with the desktop values of keys only the web or remote workbench sets.
    fn welcome_context(&self, window: &Window) -> HashMap<String, Value> {
        let mut context = self.key_context(window);
        walkthroughs::platform_context(&mut context);
        context
    }

    fn welcome_page_active(&self) -> bool {
        self.editors.active().is_some_and(|editor| editor.document().is(WALKTHROUGH_SCHEME, Path::new(GETTING_STARTED_PATH)))
    }

    /// Opens the Welcome page, or activates it; as the startup page and Help > Welcome open it, as a preview.
    fn open_welcome_page(&mut self, preview: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_walkthroughs(window);
        let path = PathBuf::from(GETTING_STARTED_PATH);
        match self.editors.editors.iter().position(|editor| editor.document().is(WALKTHROUGH_SCHEME, &path)) {
            Some(index) => self.editors.activate(index),
            None => {
                let document = TextDocument::from_text(WALKTHROUGH_SCHEME, &path, "", "utf8".into(), None, &self.languages, &self.settings.lock().unwrap());
                self.editors.open(document, preview);
            }
        }
        self.editors_changed(cx);
        let focus = self.welcome.focus.get_or_insert_with(|| cx.focus_handle()).clone();
        window.focus(&focus, cx);
        cx.notify();
    }
}

// The page's commands and dispatch actions.
impl Workbench {
    /// gettingStarted.contribution.ts `workbench.action.openWalkthrough`: a category id or `{ category, step }`
    /// opens that walkthrough; without one the page opens as it was.
    fn open_walkthrough(&mut self, walkthrough: Option<&Value>, window: &mut Window, cx: &mut Context<Self>) {
        let category = walkthrough.and_then(|value| value.as_str().or(value["category"].as_str())).map(str::to_owned);
        let step = walkthrough.and_then(|value| Some(format!("{}#{}", value["category"].as_str()?, value["step"].as_str()?)));
        if let Some(step) = step.as_deref().filter(|_| self.welcome_page_active() && self.welcome.category == category) {
            self.select_step_loose(step, window, cx);
            return;
        }
        self.open_welcome_page(true, window, cx);
        if let Some(category) = category {
            self.scroll_to_category(&category, step, window);
        }
    }

    /// `scrollToCategory`, then `buildCategorySlide`: the walkthrough's first active step not done is selected.
    fn scroll_to_category(&mut self, category: &str, step: Option<String>, window: &mut Window) {
        let Some(walkthrough) = self.walkthroughs().get(category) else {
            eprintln!("Could not find category with ID: {category}");
            return;
        };
        let context = self.welcome_context(window);
        let first_open = walkthrough.steps.iter().find(|step| walkthroughs::when_holds(step.when.as_deref(), &context) && !self.walkthroughs().is_done(&step.id));
        let Some(to_expand) = step.or_else(|| first_open.or(walkthrough.steps.first()).map(|step| step.id.clone())) else { return };
        // buildCategorySlide activates the walkthrough's extension with the id after its extension prefix.
        let walkthrough_id = category.split_once('#').map_or(category, |(_, id)| id).to_owned();
        drop(self.host.activate_by_event(&format!("onWalkthrough:{walkthrough_id}")));
        self.welcome.category = Some(category.to_owned());
        self.welcome.first_launch = false;
        self.select_step(&to_expand, window);
    }

    /// `selectStep`: expands the step and fires its `stepSelected:` event.
    fn select_step(&mut self, id: &str, window: &mut Window) {
        self.welcome.step = Some(id.to_owned());
        let progressed = self.walkthroughs_mut().progress_by_event(&format!("stepSelected:{id}"));
        if !progressed.is_empty() {
            self.steps_progressed(progressed, window);
        }
    }

    /// `selectStepLoose`: a step id with or without its category prefix.
    fn select_step_loose(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(category) = self.welcome.category.clone() else { return };
        let prefix = format!("{category}#");
        let id = if id.starts_with(&prefix) { id.to_owned() } else { format!("{prefix}{id}") };
        self.select_step(&id, window);
        cx.notify();
    }

    /// `escape`: back from a walkthrough, else `runSkip` closes the page.
    fn welcome_escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.welcome.category.is_none() {
            drop(self.execute_command("workbench.action.closeActiveEditor", Vec::new(), window, cx));
            return;
        }
        self.scroll_prev(window);
        cx.notify();
    }

    /// `scrollPrev`: to the walkthrough Next Section left, else to the categories slide.
    fn scroll_prev(&mut self, window: &mut Window) {
        if let Some(previous) = self.welcome.previous.take().filter(|previous| self.welcome.category.as_ref() != Some(previous)) {
            self.scroll_to_category(&previous, None, window);
            return;
        }
        self.welcome.category = None;
        self.welcome.step = None;
        self.welcome.media = None;
    }

    /// `runDispatchCommand`: every page action pins the page first (`workbench.action.keepEditor`).
    fn keep_welcome_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        drop(self.execute_command("workbench.action.keepEditor", Vec::new(), window, cx));
    }

    fn select_category(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.keep_welcome_page(window, cx);
        self.scroll_to_category(id, None, window);
        self.walkthroughs_mut().mark_opened(id);
        let metadata = self.walkthroughs().metadata_json();
        self.profile_storage.lock().unwrap().set(walkthroughs::METADATA_KEY, metadata);
        self.editors_changed(cx);
        cx.notify();
    }

    fn hidden_categories(&self) -> Vec<String> {
        walkthroughs::parse_hidden(self.profile_storage.lock().unwrap().get(walkthroughs::HIDDEN_KEY))
    }

    fn hide_category(&mut self, id: &str) {
        let mut hidden = self.hidden_categories();
        if !hidden.iter().any(|known| known == id) {
            hidden.push(id.to_owned());
        }
        self.profile_storage.lock().unwrap().set(walkthroughs::HIDDEN_KEY, json!(hidden).to_string());
    }

    /// `onDidProgressStep` on the page: saves the memento, hides a walkthrough whose last active step completed,
    /// and announces a completed step of the open walkthrough.
    fn steps_progressed(&mut self, steps: Vec<String>, window: &Window) {
        let progress = self.walkthroughs().progress_json();
        self.profile_storage.lock().unwrap().set(walkthroughs::PROGRESS_KEY, progress);
        let context = self.welcome_context(window);
        let page_open = self.editors.editors.iter().any(|editor| editor.document().is(WALKTHROUGH_SCHEME, Path::new(GETTING_STARTED_PATH)));
        for id in steps {
            let Some((walkthrough, step)) = self.walkthroughs().step(&id) else {
                eprintln!("Tried to progress unknown step");
                continue;
            };
            let (done, total) = self.walkthroughs().stats(walkthrough, &context);
            // Upstream compares the counts from before the step was marked: done then was one less if it is active.
            let done_before = done - usize::from(walkthroughs::when_holds(step.when.as_deref(), &context));
            let (category, title) = (walkthrough.id.clone(), step.title.clone());
            if page_open && done_before + 1 == total {
                self.hide_category(&category);
            }
            if self.welcome.category.as_deref() == Some(category.as_str()) {
                self.aria_alert(&format!("Step {title} completed"));
            }
        }
    }

    fn deprogress_step(&mut self, id: &str) {
        if self.walkthroughs().step(id).is_none() {
            eprintln!("Attempting to access step which does not exist in registry {id}");
            return;
        }
        self.walkthroughs_mut().deprogress_step(id);
        let progress = self.walkthroughs().progress_json();
        self.profile_storage.lock().unwrap().set(walkthroughs::PROGRESS_KEY, progress);
    }

    fn toggle_step_completion(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.keep_welcome_page(window, cx);
        match self.walkthroughs().is_done(id) {
            true => self.deprogress_step(id),
            false => {
                self.walkthroughs_mut().progress_step(id);
                self.steps_progressed(vec![id.to_owned()], window);
            }
        }
        cx.notify();
    }

    /// `markAllStepsComplete`.
    fn mark_all_done(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.keep_welcome_page(window, cx);
        let Some(category) = self.welcome.category.clone() else { return };
        let Some(steps) = self.walkthroughs().get(&category).map(|walkthrough| walkthrough.steps.iter().map(|step| step.id.clone()).collect::<Vec<_>>()) else { return };
        let progressed: Vec<String> = steps.into_iter().filter(|step| self.walkthroughs_mut().progress_step(step)).collect();
        self.steps_progressed(progressed, window);
        self.hide_category(&category);
        self.scroll_prev(window);
        cx.notify();
    }

    fn next_section(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.keep_welcome_page(window, cx);
        let Some(next) = self.welcome.category.as_deref().and_then(|id| self.walkthroughs().get(id)?.next.clone()) else { return };
        self.welcome.previous = self.welcome.category.clone();
        self.scroll_to_category(&next, None, window);
        cx.notify();
    }

    /// `welcome.showAllWalkthroughs` (`placeholder`) and the page's `openWalkthroughSelector` (`title`): the
    /// walkthroughs whose `when` holds.
    fn show_walkthrough_picker(&mut self, placeholder: Option<&str>, title: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_walkthroughs(window);
        let context = self.welcome_context(window);
        let shown: Vec<&Walkthrough> = self.walkthroughs().walkthroughs.iter().filter(|walkthrough| !walkthrough.steps.is_empty() && walkthroughs::when_holds(walkthrough.when.as_deref(), &context)).collect();
        let ids: Vec<String> = shown.iter().map(|walkthrough| walkthrough.id.clone()).collect();
        let items = shown
            .iter()
            .enumerate()
            .map(|(handle, walkthrough)| Entry::Item(Item { handle: handle as i64, label: walkthrough.title.clone(), detail: Some(walkthrough.description.clone()), description: Some(walkthrough.source.clone()), pickable: true, ..Item::default() }))
            .collect();
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = placeholder.map(Into::into);
        input.title = title.map(Into::into);
        input.match_on_description = true;
        input.match_on_detail = true;
        input.set_items(items);
        let picked = self.choose(input, window, cx);
        let from_page = title.is_some();
        cx.spawn_in(window, async move |workbench, cx| {
            let Ok(Ok(Value::Number(handle))) = picked.await else { return };
            let Some(id) = handle.as_u64().and_then(|handle| ids.get(handle as usize)).cloned() else { return };
            let _ = workbench.update_in(cx, |workbench, window, cx| match from_page {
                true => workbench.select_category(&id, window, cx),
                false => drop(workbench.execute_command("workbench.action.openWalkthrough", vec![json!(id)], window, cx)),
            });
        })
        .detach();
    }

    /// `runStepCommand`: `command:` links run with their JSON arguments, other links go to the opener.
    // shortcut: `command:toSide:` runs in the one editor group; side groups come with editor splits (milestone 11).
    fn run_step_command(&mut self, href: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(command) = href.strip_prefix("command:") else {
            drop(self.open_link(Target::Text(href.to_owned()), OpenOptions { allow_commands: true, ..OpenOptions::default() }, window, cx));
            if href.starts_with("https://") || href.starts_with("http://") {
                self.welcome.pending_events.push(format!("onLink:{href}"));
                cx.notify();
            }
            return;
        };
        let command = command.strip_prefix("toSide:").unwrap_or(command);
        let (id, args) = command_args(command);
        if matches!(id.as_str(), "workbench.action.files.openFileFolder" | "workbench.action.files.openFolder") && self.workspace().is_none() {
            self.store_walkthrough_to_restore();
        }
        drop(self.execute_command(&id, args, window, cx));
    }

    /// An empty window's step that opens a folder, with steps left after it: the walkthrough continues there.
    fn store_walkthrough_to_restore(&mut self) {
        let Some(category) = self.welcome.category.clone() else { return };
        let Some(walkthrough) = self.walkthroughs().get(&category) else { return };
        let Some(selected) = walkthrough.steps.iter().position(|step| Some(&step.id) == self.welcome.step.as_ref()) else { return };
        if walkthrough.steps[selected + 1..].iter().all(|step| self.walkthroughs().is_done(&step.id)) {
            return;
        }
        let restore = json!({ "folder": EMPTY_WINDOW, "category": category, "step": self.welcome.step });
        self.profile_storage.lock().unwrap().set(RESTORE_KEY, restore.to_string());
    }

    /// The step list's Up and Down (`stepListContainer` keydown) and Escape (`welcome.goBack`).
    fn welcome_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        if key == "escape" {
            cx.stop_propagation();
            return self.welcome_escape(window, cx);
        }
        let (Some(category), Some(selected)) = (self.welcome.category.clone(), self.welcome.step.clone()) else { return };
        let Some(walkthrough) = self.walkthroughs().get(&category) else { return };
        let context = self.welcome_context(window);
        let current = walkthrough.steps.iter().position(|step| step.id == selected);
        let active = |index: &usize| walkthroughs::when_holds(walkthrough.steps[*index].when.as_deref(), &context);
        let target = match (key, current) {
            ("up", Some(current)) => (0..current).rev().find(active),
            ("down", current) => (current.map_or(0, |current| current + 1)..walkthrough.steps.len()).find(active),
            _ => None,
        };
        let Some(target) = target.map(|index| walkthrough.steps[index].id.clone()) else { return };
        cx.stop_propagation();
        self.select_step(&target, window);
        cx.notify();
    }
}

// The categories slide.
impl Workbench {
    fn welcome_categories(&self, window: &Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let size = self.welcome.size.get();
        let (width, height) = (f32::from(size.width), f32::from(size.height));
        let width_constrained = width <= 400.;
        let height_constrained = height <= 600.;
        let context = self.welcome_context(window);
        let hidden = self.hidden_categories();
        let (shown, active_count) = self.walkthroughs().ranked(&hidden, now_millis(), &context, 5);
        let no_walkthroughs = shown.is_empty();
        let all_active = self.walkthroughs().walkthroughs.iter().filter(|walkthrough| walkthroughs::when_holds(walkthrough.when.as_deref(), &context)).count();
        let some_hidden = !hidden.is_empty() || shown.len() < all_active || active_count < all_active;
        let start = self.welcome_start_list(&context, cx);
        let recent = self.welcome_recent_list(if no_walkthroughs { 10 } else { 5 }, cx);
        let (left, right) = match no_walkthroughs {
            true => (vec![start], vec![recent]),
            false => (vec![start, recent], vec![self.welcome_walkthrough_list(&shown, some_hidden, &context, cx)]),
        };
        let column = |lists: Vec<Div>| div().overflow_hidden().flex().flex_col().children(lists);
        let content_height = (height - 24.).max(0.);
        let header = div()
            .h(px(content_height * 0.25))
            .flex()
            .flex_col()
            .justify_end()
            .overflow_hidden()
            .child(div().pt(px(5.)).text_size(px(35.1)).line_height(px(42.)).whitespace_nowrap().child(crate::product::name_long()))
            .child(div().mt(px(10.92)).mb(px(18.2)).text_size(px(18.2)).line_height(px(25.48)).text_color(self.color("descriptionForeground")).child("Editing evolved"));
        let footer = div().flex().justify_center().child(self.welcome_show_on_startup(cx));
        // media/gettingStarted.css `.gettingStartedCategoriesContainer`: columns 1fr 6fr 1fr 6fr 1fr, at most 1200px;
        // `height-constrained` drops the header and puts the columns in the grid's first two tracks.
        let grid = div().w_full().max_w(px(1200.)).mx_auto().min_h(px(content_height)).flex().flex_col();
        let grid = match (width_constrained, height_constrained) {
            (true, _) => grid.child(column(left)).child(column(right)).child(footer),
            (false, true) => grid.child(div().flex_1().flex().child(column(left).w(relative(1. / 15.))).child(column(right).w(relative(6. / 15.)))).child(footer),
            (false, false) => grid
                .child(div().ml(relative(1. / 15.)).w(relative(13. / 15.)).child(header))
                .child(div().flex_1().flex().child(div().w(relative(1. / 15.))).child(column(left).w(relative(6. / 15.))).child(div().w(relative(1. / 15.))).child(column(right).w(relative(6. / 15.))))
                .child(div().ml(relative(1. / 15.)).w(relative(13. / 15.)).child(footer)),
        };
        div().id("welcome-categories").size_full().overflow_y_scroll().px(px(24.)).py(px(12.)).child(grid)
    }

    /// gettingStartedList.ts `GettingStartedIndexList`: the title and the list.
    fn welcome_index_list(&self, id: &str, title: &str, items: Vec<AnyElement>) -> Div {
        div()
            .mb(px(32.))
            .child(div().mb(px(5.)).text_size(px(19.5)).line_height(px(24.)).child(title.to_owned()))
            .child(div().id(SharedString::from(format!("welcome-{id}-list"))).role(Role::List).aria_label(title.to_owned()).flex().flex_col().line_height(px(24.)).children(items))
    }

    fn welcome_link(&self, id: SharedString) -> Stateful<Div> {
        let active = self.color("textLink.activeForeground");
        div().id(id).cursor_pointer().text_color(self.color("textLink.foreground")).hover(move |link| link.text_color(active))
    }

    fn welcome_start_list(&self, context: &HashMap<String, Value>, cx: &mut Context<Self>) -> Div {
        let icon_color = self.color("textLink.foreground");
        let mut items = Vec::new();
        for entry in walkthroughs::START_ENTRIES.iter().filter(|entry| walkthroughs::when_holds(entry.when, context)) {
            let command = entry.command;
            let shortcut = self.welcome_keybinding_label(command.trim_start_matches("command:")).unwrap_or_default();
            items.push(
                div().id(SharedString::from(format!("welcome-start-item-{}", entry.id))).role(Role::ListItem).child(
                    self.welcome_link(SharedString::from(format!("welcome-start-{}", entry.id)))
                        .role(Role::Button)
                        .aria_label(entry.title)
                        .aria_description(format!("{} {shortcut}", entry.description).trim_end().to_owned())
                        .m(px(2.))
                        .flex()
                        .items_center()
                        .child(self.icon(entry.icon, 28., 20., icon_color).w(px(28.)).justify_start())
                        .child(entry.title)
                        .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                            workbench.keep_welcome_page(window, cx);
                            workbench.run_step_command(command, window, cx);
                        })),
                )
                .into_any_element(),
            );
        }
        self.welcome_index_list("start", "Start", items).min_h(px(156.)).mb(px(16.))
    }

    /// `buildRecentlyOpenedList`: folders and workspaces other than this window's, `file` ones only.
    fn welcome_recent_list(&self, limit: usize, cx: &mut Context<Self>) -> Div {
        let current = self.workspace_location();
        let entries: Vec<&Value> = self.recently_opened.workspaces.iter().filter(|entry| recently_opened::path(entry).is_some_and(|path| Some(&path) != current.as_ref())).collect();
        let mut items = Vec::new();
        for (index, entry) in entries.iter().take(limit).enumerate() {
            let full_path = recently_opened::label(entry);
            let (name, parent) = super::workspace_actions::split_recent_label(&full_path);
            let openable = super::workspace_actions::recent_openable(entry);
            let uri = recently_opened::location(entry).to_owned();
            let group = SharedString::from(format!("welcome-recent-{index}"));
            let toolbar_hover = self.color("toolbar.hoverBackground");
            items.push(
                div()
                    .id(group.clone())
                    .group(group.clone())
                    .role(Role::ListItem)
                    .relative()
                    .flex()
                    .items_center()
                    .pr(px(24.))
                    .overflow_hidden()
                    .child(
                        self.welcome_link(SharedString::from(format!("welcome-recent-open-{index}")))
                            .role(Role::Button)
                            .aria_label(format!("Open folder {name} with path {parent}"))
                            .flex_none()
                            .m(px(2.))
                            .child(name.clone())
                            .on_click(cx.listener(move |workbench, event: &ClickEvent, window, cx| {
                                workbench.keep_welcome_page(window, cx);
                                workbench.open_window(vec![openable.clone()], event.modifiers().platform || event.modifiers().control, window, cx);
                            })),
                    )
                    .child(div().flex_1().min_w_0().pl(px(13.)).overflow_hidden().whitespace_nowrap().text_ellipsis().text_color(self.color("descriptionForeground")).child(parent))
                    .child(
                        self.icon("close", 24., 16., self.foreground_or("icon.foreground"))
                            .id(SharedString::from(format!("welcome-recent-remove-{index}")))
                            .role(Role::Button)
                            .aria_label(format!("Remove {name} from Recently Opened"))
                            .absolute()
                            .right_0()
                            .rounded(px(3.))
                            .cursor_pointer()
                            .invisible()
                            .group_hover(group, |button| button.visible())
                            .hover(move |button| button.bg(toolbar_hover))
                            .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                                cx.stop_propagation();
                                recently_opened::remove(&mut workbench.shared_storage.lock().unwrap(), &[uri.as_str()]);
                                workbench.recently_opened_changed(window, cx);
                                cx.notify();
                            })),
                    )
                    .into_any_element(),
            );
        }
        if entries.len() > limit {
            let shortcut = self.welcome_keybinding_label("workbench.action.openRecent").unwrap_or_default();
            items.push(
                div().child(
                    self.welcome_link("welcome-recent-more".into())
                        .role(Role::Button)
                        .aria_label("More...")
                        .aria_description(format!("Show All Recent Folders {shortcut}").trim_end().to_owned())
                        .m(px(2.))
                        .child("More...")
                        .on_click(cx.listener(|workbench, _: &ClickEvent, window, cx| {
                            workbench.keep_welcome_page(window, cx);
                            drop(workbench.execute_command("workbench.action.openRecent", Vec::new(), window, cx));
                        })),
                )
                .into_any_element(),
            );
        } else if entries.is_empty() {
            let open_folder = if self.workspace().is_some_and(|workspace| workspace.file.is_some()) { "workbench.action.files.openFolderViaWorkspace" } else { "workbench.action.files.openFolder" };
            items.push(
                div()
                    .flex()
                    .flex_wrap()
                    .child("You have no recent folders,\u{a0}")
                    .child(self.welcome_link("welcome-recent-open-folder".into()).role(Role::Button).aria_label("open a folder").child("open a folder").on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                        workbench.keep_welcome_page(window, cx);
                        drop(workbench.execute_command(open_folder, Vec::new(), window, cx));
                    })))
                    .child("\u{a0}to start.")
                    .into_any_element(),
            );
        }
        self.welcome_index_list("recent", "Recent", items).mb(px(8.))
    }

    fn welcome_walkthrough_list(&self, shown: &[&Walkthrough], some_hidden: bool, context: &HashMap<String, Value>, cx: &mut Context<Self>) -> Div {
        let mut items: Vec<AnyElement> = shown.iter().map(|walkthrough| self.walkthrough_tile(walkthrough, context, cx).into_any_element()).collect();
        if some_hidden {
            items.push(div().child(self.welcome_link("welcome-walkthroughs-more".into()).role(Role::Button).aria_label("More...").child("More...").on_click(cx.listener(|workbench, _: &ClickEvent, window, cx| {
                workbench.keep_welcome_page(window, cx);
                workbench.show_walkthrough_picker(None, Some("Open Walkthrough..."), window, cx);
            }))).into_any_element());
        }
        self.welcome_index_list("walkthroughs", "Walkthroughs", items)
    }

    /// `renderGetttingStaredWalkthrough`: icon, title, badges, hide button, description and progress bar.
    fn walkthrough_tile(&self, walkthrough: &Walkthrough, context: &HashMap<String, Value>, cx: &mut Context<Self>) -> Stateful<Div> {
        let (new_entry, new_items, _) = self.walkthroughs().freshness(walkthrough, now_millis());
        let (done, total) = self.walkthroughs().stats(walkthrough, context);
        let featured = walkthrough.is_featured;
        let id = walkthrough.id.clone();
        let hide_id = walkthrough.id.clone();
        let group = SharedString::from(format!("welcome-walkthrough-{}", walkthrough.id));
        let hover = self.color("welcomePage.tileHoverBackground");
        let toolbar_hover = self.color("toolbar.hoverBackground");
        let badge_background = self.color("activityBarBadge.background");
        let badge_foreground = self.color("activityBarBadge.foreground");
        let mut main = div()
            .flex()
            .items_center()
            .w_full()
            .child(self.walkthrough_icon(&walkthrough.icon, featured))
            .child(div().my(px(4.)).font_weight(FontWeight::SEMIBOLD).overflow_hidden().text_ellipsis().whitespace_nowrap().child(walkthrough.title.clone()));
        if (new_entry || new_items) && !featured {
            main = main.child(div().m(px(4.)).px(px(4.)).py(px(2.)).rounded(px(2.)).text_size(px(11.)).whitespace_nowrap().bg(badge_background).text_color(badge_foreground).child(if new_entry { "New" } else { "Updated" }));
        }
        main = main.child(
            self.icon("close", 22., 16., self.foreground_or("icon.foreground"))
                .id(SharedString::from(format!("welcome-hide-{}", walkthrough.id)))
                .role(Role::Button)
                .aria_label("Hide")
                .ml_auto()
                .rounded(px(5.))
                .cursor_pointer()
                .invisible()
                .group_hover(group.clone(), |button| button.visible())
                .hover(move |button| button.bg(toolbar_hover))
                .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                    cx.stop_propagation();
                    workbench.keep_welcome_page(window, cx);
                    workbench.hide_category(&hide_id);
                    cx.notify();
                })),
        );
        let progress_label = match done == total {
            true => format!("All {done} steps complete!"),
            false => format!("{done} of {total} steps complete"),
        };
        let progress = div()
            .id(SharedString::from(format!("welcome-progress-{}", walkthrough.id)))
            .role(Role::ProgressIndicator)
            .aria_label(progress_label)
            .aria_min_numeric_value(0.)
            .aria_max_numeric_value(total as f64)
            .aria_numeric_value(done as f64)
            .mt(px(4.))
            .h(px(4.))
            .bg(self.color("welcomePage.progress.background"))
            .child(div().h_full().w(relative(done as f32 / total.max(1) as f32)).bg(self.color("welcomePage.progress.foreground")));
        let tile = div()
            .id(group.clone())
            .group(group)
            .role(Role::Button)
            .aria_label(walkthrough.title.clone())
            .aria_description(walkthrough.description.clone())
            .relative()
            .overflow_hidden()
            .cursor_pointer()
            // `.getting-started-category { line-height: normal }` inside the 24px list.
            .line_height(px(16.))
            .my(px(8.))
            .rounded(px(6.))
            .border_1()
            .border_color(self.color("contrastBorder"))
            .bg(self.color("welcomePage.tileBackground"))
            .hover(move |tile| tile.bg(hover))
            .map(|tile| if done == 0 { tile.pt(px(4.)).pr(px(4.)).pb(px(4.)).pl(px(6.)) } else { tile.px(px(6.)).py(px(4.)) })
            .when(featured, |tile| tile.child(div().absolute().top_0().left_0().size(px(20.)).flex().items_center().justify_center().rounded_tl(px(6.)).rounded_br(px(6.)).bg(badge_background).child(self.icon("star-full", 20., 12., badge_foreground))))
            .child(main)
            .when(featured, |tile| tile.child(div().ml(px(28.)).mb(px(8.)).child(walkthrough.description.clone())))
            .when(done > 0, |tile| tile.child(div().absolute().bottom_0().left_0().w_full().child(progress)))
            .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| workbench.select_category(&id, window, cx)));
        // `width: calc(100% - 16px)` with `margin: 8px 8px 8px 0`.
        div().id(SharedString::from(format!("welcome-walkthrough-item-{}", walkthrough.id))).role(Role::ListItem).pr(px(16.)).child(tile)
    }

    /// `iconWidgetFor`: a codicon in the link color, or the extension's image; hidden on a featured tile.
    fn walkthrough_icon(&self, icon: &Icon, featured: bool) -> Div {
        let widget = match icon {
            Icon::Codicon(name) => self.icon(name, 28., 20., self.color("textLink.foreground")).w(px(28.)).justify_start(),
            Icon::Image(path) => div().w(px(28.)).flex_none().child(img(path.clone()).size(px(20.))),
        };
        widget.when(featured, |widget| widget.invisible())
    }

    /// The footer's show-on-startup toggle: `workbench.startupEditor` is `welcomePage` or `none`.
    fn welcome_show_on_startup(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let checked = self.setting(STARTUP_EDITOR_KEY) == json!("welcomePage");
        div()
            .id("welcome-show-on-startup")
            .role(Role::CheckBox)
            .aria_label("Show welcome page on startup")
            .aria_toggled(if checked { Toggled::True } else { Toggled::False })
            .cursor_pointer()
            .p(px(4.))
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                div()
                    .size(px(18.))
                    .mr(px(9.))
                    .rounded(px(3.))
                    .border_1()
                    .border_color(self.color("checkbox.border"))
                    .bg(self.color("checkbox.background"))
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(checked, |checkbox| checkbox.child(self.icon("check", 16., 16., self.foreground_or("checkbox.foreground")))),
            )
            .child("Show welcome page on startup")
            .on_click(cx.listener(move |workbench, _: &ClickEvent, _, cx| {
                workbench.update_setting(STARTUP_EDITOR_KEY, json!(if checked { "none" } else { "welcomePage" }));
                cx.notify();
            }))
    }

    fn welcome_keybinding_label(&self, command: &str) -> Option<String> {
        let item = self.keybindings.resolver.lookup_primary_keybinding(command, &self.context_keys)?;
        Some(crate::keybindings::label(&item.keybinding))
    }
}

// The details slide.
impl Workbench {
    fn welcome_details(&self, window: &Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let Some(walkthrough) = self.welcome.category.as_deref().and_then(|category| self.walkthroughs().get(category)) else { return div().id("welcome-details") };
        let size = self.welcome.size.get();
        let semi_constrained = f32::from(size.width) <= 950.;
        let height = f32::from(size.height);
        let context = self.welcome_context(window);
        let step = self.welcome.step.as_deref().and_then(|id| walkthrough.steps.iter().find(|step| step.id == id));
        let back_label = if self.welcome.first_launch { "Welcome" } else { "Go Back" };
        let back = self
            .welcome_link("welcome-go-back".into())
            .role(Role::Button)
            .aria_label(back_label)
            .absolute()
            .top(px(if semi_constrained || height <= 600. { -10. } else { 5. } + 10.))
            .left(px(if semi_constrained || height <= 600. { 0. } else { 40. } + 10.))
            .px(px(2.))
            .pb(px(2.))
            .flex()
            .items_center()
            .child(self.icon("chevron-left", 16., 16., self.color("textLink.foreground")).ml(px(-4.)))
            .when(height > 600., |back| back.child(back_label))
            .on_click(cx.listener(|workbench, _: &ClickEvent, window, cx| {
                workbench.keep_welcome_page(window, cx);
                workbench.scroll_prev(window);
                cx.notify();
            }));
        let description_color = self.color("descriptionForeground");
        // h2's 0.83em margin above the title and the description paragraph's 1em margin (user agent styles).
        let mut title = div().mb(px(24.)).child(div().mt(px(21.58)).mb(px(8.)).text_size(px(26.)).line_height(px(31.)).child(walkthrough.title.clone()));
        for (index, line) in walkthroughs::parse_description(&walkthrough.description).iter().enumerate() {
            title = title.child(self.linked_text_paragraph(line, &format!("category-{index}"), cx).my(px(18.2)).text_size(px(18.2)).line_height(px(25.48)).text_color(description_color));
        }
        let steps = self.walkthrough_steps(walkthrough, &context, cx);
        let spacer = || div().flex_none().h(px((height * 0.25 - 100.).max(0.)));
        let content = match step.map(|step| &step.media).filter(|_| !semi_constrained) {
            None => div().max_w(px(500.)).mx_auto().pt(px(30.)).flex().flex_col().child(title).child(steps),
            // `.markdown` and `.video` media fill the media column top to bottom.
            Some(Media::Markdown { .. } | Media::Video { .. }) => div()
                .h_full()
                .flex()
                .child(div().w(relative(1. / 15.)))
                .child(div().w(relative(5. / 15.)).flex().flex_col().child(spacer()).child(title).child(steps))
                .child(div().w(relative(1. / 15.)))
                .child(div().w(relative(8. / 15.)).h_full().child(self.welcome_media_area())),
            // `.image` media spans the title and steps rows.
            Some(media) => div().h_full().flex().flex_col().child(spacer()).child(
                div()
                    .flex()
                    .child(div().w(relative(1. / 15.)))
                    .child(div().w(relative(5. / 15.)).flex().flex_col().child(title).child(steps))
                    .child(div().w(relative(1. / 15.)))
                    .child(div().w(relative(8. / 15.)).flex().justify_center().child(self.welcome_media(media, step.expect("media of the selected step"), cx))),
            ),
        };
        div().id("welcome-details").relative().size_full().overflow_y_scroll().child(div().h_full().max_w(relative(0.8)).mx_auto().px(px(32.)).child(content)).child(back)
    }

    /// `buildStepList` and the Mark Done / Next Section row.
    fn walkthrough_steps(&self, walkthrough: &Walkthrough, context: &HashMap<String, Value>, cx: &mut Context<Self>) -> Div {
        let title_color = self.foreground_or("walkthrough.stepTitle.foreground");
        let link = self.color("textLink.foreground");
        let mut list = div().id("welcome-steps").role(Role::List).aria_label(walkthrough.title.clone()).flex().flex_col();
        for step in walkthrough.steps.iter().filter(|step| walkthroughs::when_holds(step.when.as_deref(), context)) {
            let expanded = self.welcome.step.as_deref() == Some(step.id.as_str());
            let done = self.walkthroughs().is_done(&step.id);
            let (toggle_id, select_id) = (step.id.clone(), step.id.clone());
            let check_color = if done || expanded { link } else { self.color("descriptionForeground") };
            // Codicon.circleLargeOutline (0xebb5) ships as circle-large.svg.
            let checkbox = self
                .icon(if done { "pass-filled" } else { "circle-large" }, 16., 16., check_color)
                .id(SharedString::from(format!("welcome-step-check-{}", step.id)))
                .role(Role::CheckBox)
                .aria_label(if done { format!("{}: Completed", step.title) } else { format!("{}: Not completed", step.title) })
                .aria_toggled(if done { Toggled::True } else { Toggled::False })
                .flex_none()
                .mr(px(8.))
                .cursor_pointer()
                .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                    cx.stop_propagation();
                    workbench.toggle_step_completion(&toggle_id, window, cx);
                }));
            let step_title = div().font_weight(FontWeight::SEMIBOLD).child(step.title.clone());
            let step_title = match expanded {
                true => step_title.text_color(title_color),
                false => step_title.overflow_hidden().whitespace_nowrap().text_ellipsis(),
            };
            let body = div().flex_1().min_w_0().child(step_title).when(expanded, |body| body.child(self.step_description(step, cx)));
            let hover = self.color("welcomePage.tileHoverBackground");
            let row = div()
                .id(SharedString::from(format!("welcome-step-{}", step.id)))
                .role(Role::ListItem)
                .aria_label(step.title.clone())
                .aria_expanded(expanded)
                .flex()
                .w_full()
                .my(px(1.))
                .p(px(16.))
                .rounded(px(6.))
                .overflow_hidden()
                .child(checkbox)
                .child(body);
            let row = match expanded {
                true => row.bg(self.color("welcomePage.tileBackground")).border_1().border_color(self.color("welcomePage.tileBorder")),
                false => row.h(px(48.)).cursor_pointer().text_color(self.color("descriptionForeground")).hover(move |row| row.bg(hover)).on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                    workbench.keep_welcome_page(window, cx);
                    workbench.select_step(&select_id, window);
                    cx.notify();
                })),
            };
            list = list.child(row);
        }
        let has_next = walkthrough.next.as_deref().is_some_and(|next| self.walkthroughs().get(next).is_some());
        let done_next = div()
            .flex()
            .p(px(16.))
            .child(
                self.welcome_link("welcome-mark-done".into())
                    .role(Role::Button)
                    .aria_label("Mark Done")
                    .m(px(2.))
                    .flex()
                    .items_center()
                    .child(self.icon("check-all", 20., 16., link).mr(px(4.)))
                    .child("Mark Done")
                    .on_click(cx.listener(|workbench, _: &ClickEvent, window, cx| workbench.mark_all_done(window, cx))),
            )
            .when(has_next, |row| {
                row.child(
                    self.welcome_link("welcome-next-section".into())
                        .role(Role::Button)
                        .aria_label("Next Section")
                        .ml_auto()
                        .my(px(2.))
                        .flex()
                        .items_center()
                        .child("Next Section")
                        .child(self.icon("arrow-right", 22., 18., link).pl(px(4.)))
                        .on_click(cx.listener(|workbench, _: &ClickEvent, window, cx| workbench.next_section(window, cx))),
                )
            });
        div().flex().flex_col().child(list).child(done_next)
    }

    /// `buildMarkdownDescription`: a line that is a lone link is a button with the command's keyboard shortcut tip,
    /// other lines are paragraphs with inline links.
    fn step_description(&self, step: &walkthroughs::Step, cx: &mut Context<Self>) -> Div {
        let mut container = div().pt(px(8.)).flex().flex_col().items_start();
        let count = step.description.len();
        for (index, line) in step.description.iter().enumerate() {
            let id = format!("{}-{index}", step.id);
            let Some((label, href, title)) = walkthroughs::as_button(line) else {
                container = container.child(self.linked_text_paragraph(line, &id, cx).my(px(13.)).line_height(px(16.9)));
                continue;
            };
            let href_owned = href.to_owned();
            let hover = self.color("button.hoverBackground");
            container = container.child(
                div().when(index + 1 < count, |button| button.mb(px(6.))).child(
                    div()
                        .id(SharedString::from(format!("welcome-step-button-{id}")))
                        .role(Role::Button)
                        .aria_label(title.unwrap_or(label).to_owned())
                        .cursor_pointer()
                        .flex()
                        .justify_center()
                        .px(px(12.))
                        .py(px(6.))
                        .rounded(px(2.))
                        .bg(self.color("button.background"))
                        .text_color(self.color("button.foreground"))
                        .border_1()
                        .border_color(self.color("button.border"))
                        .hover(move |button| button.bg(hover))
                        .line_height(px(18.))
                        .whitespace_nowrap()
                        .child(label.to_owned())
                        .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                            cx.stop_propagation();
                            workbench.keep_welcome_page(window, cx);
                            workbench.run_step_command(&href_owned, window, cx);
                        })),
                ),
            );
            let Some(command) = href.strip_prefix("command:") else { continue };
            let command = command.strip_prefix("toSide:").unwrap_or(command);
            let Some(shortcut) = self.welcome_keybinding_label(command.split('?').next().unwrap_or(command)) else { continue };
            let key = div()
                .px(px(3.))
                .py(px(2.))
                .mx(px(2.))
                .rounded(px(2.))
                .border_1()
                .text_size(px(10.))
                .font_weight(FontWeight::SEMIBOLD)
                .bg(self.color("keybindingLabel.background"))
                .text_color(self.color("keybindingLabel.foreground"))
                .border_color(self.color("keybindingLabel.border"))
                .child(shortcut);
            container = container.child(div().mt(px(12.)).flex().items_center().text_size(px(12.)).text_color(self.color("descriptionForeground")).child("Tip: Use keyboard shortcut\u{a0}").child(key));
        }
        container
    }

    /// A paragraph of text and inline links (`Link` with `runStepCommand` as its opener).
    fn linked_text_paragraph(&self, line: &LinkedText, id: &str, cx: &mut Context<Self>) -> Div {
        let mut paragraph = div().flex().flex_wrap();
        for (index, node) in line.iter().enumerate() {
            match node {
                // One flex item per word, so a long sentence wraps inside the row next to its links.
                Node::Text(text) => paragraph = paragraph.children(strip_formatting(text).split_inclusive(' ').map(str::to_owned).collect::<Vec<_>>()),
                Node::Link { label, href, title } => {
                    let tooltip = title.clone().unwrap_or_else(|| href.clone());
                    let href = href.clone();
                    paragraph = paragraph.child(
                        self.welcome_link(SharedString::from(format!("welcome-link-{id}-{index}")))
                            .role(Role::Link)
                            .aria_label(label.clone())
                            .aria_description(tooltip)
                            .child(label.clone())
                            .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                                cx.stop_propagation();
                                workbench.keep_welcome_page(window, cx);
                                workbench.run_step_command(&href, window, cx);
                            })),
                    );
                }
            }
        }
        paragraph
    }

    /// Image media is drawn by GPUI for the theme kind; a click on it opens the step's only http link. Svg media
    /// gets the webview area.
    fn welcome_media(&self, media: &Media, step: &walkthroughs::Step, cx: &mut Context<Self>) -> Div {
        let Media::Image { paths, alt_text } = media else { return self.welcome_media_area().w_full().min_h(px(300.)) };
        let path = theme_path(paths, self.theme.kind);
        let links: Vec<String> = step.description.iter().flatten().filter_map(|node| if let Node::Link { href, .. } = node { Some(href.clone()) } else { None }).collect();
        let single_http = match links.as_slice() {
            [href] if href.starts_with("http") => Some(href.clone()),
            _ => None,
        };
        let image = if path.starts_with("https://") { img(SharedString::from(path)) } else { img(PathBuf::from(path)) };
        div().w_full().flex().justify_center().child(
            div()
                .id("welcome-media-image")
                .role(Role::Image)
                .aria_label(format!("Image showing {alt_text}"))
                .when(single_http.is_some(), |image| image.cursor_pointer())
                .child(image.max_w_full())
                .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                    if let Some(href) = &single_http {
                        drop(workbench.open_link(Target::Text(href.clone()), OpenOptions::default(), window, cx));
                    }
                })),
        )
    }

    /// The box the step media webview is placed over.
    fn welcome_media_area(&self) -> Div {
        let view = self.welcome.media.as_ref().map(|media| media.webview.view());
        div().size_full().child(
            canvas(
                move |bounds, window, _| {
                    if let Some(view) = &view {
                        webview::place(view, window, bounds);
                    }
                },
                |_, _, _, _| {},
            )
            .size_full(),
        )
    }
}

// The step media webview (gettingStartedDetailsRenderer.ts).
impl Workbench {
    /// Runs each render: the selected step's svg, markdown or video media has its webview while the page shows it.
    fn update_welcome_media(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let wide = f32::from(self.welcome.size.get().width) > 950.;
        let step = self.welcome.step.clone().filter(|_| wide && self.welcome_page_active() && self.welcome.category.is_some());
        let media = step.as_deref().and_then(|id| self.walkthroughs().step(id)).map(|(_, step)| step.media.clone()).filter(|media| !matches!(media, Media::Image { .. }));
        let (Some(step), Some(media)) = (step, media) else {
            self.welcome.media = None;
            return;
        };
        let covered = self.quick_inputs.current().is_some() || self.keybindings_editor.open || self.context_menu.is_some();
        let theme = self.theme.settings_id.clone();
        if let Some(shown) = &mut self.welcome.media
            && shown.step == step
            && shown.theme == theme
        {
            if covered {
                shown.webview.hide();
            }
            self.post_enabled_context_keys(window);
            return;
        }
        let (html, roots, checked_on) = self.media_html(&media);
        let events = self.welcome_webview_events(window, cx);
        let pre = self.app_root.join("src/vs/workbench/contrib/webview/browser/pre");
        let mut webview = Webview::new(window, WEBVIEW_ID, WEBVIEW_ORIGIN, "", pre, roots, events);
        let label = self.color_themes.iter().find(|entry| entry.settings_id == self.theme.settings_id).map_or(self.theme.settings_id.clone(), |entry| entry.label.clone());
        webview.send("styles", webview::theme_data(&self.theme, &label, &self.setting("editor"), self.setting("accessibility.underlineLinks") == true, self.is_motion_reduced()));
        let allow_scripts = !matches!(media, Media::Svg { .. });
        webview.send("content", json!({ "contents": html, "title": "", "options": { "allowMultipleAPIAcquire": false, "allowScripts": allow_scripts, "allowForms": false }, "state": Value::Null, "cspSource": "'self' https://*.vscode-cdn.net", "confirmBeforeClose": "keyboardOnly" }));
        self.welcome.media = Some(MediaWebview { step, theme, webview, checked_on, enabled: None });
        self.post_enabled_context_keys(window);
    }

    /// `postTrueKeysMessage`: the `checked-on` expressions that hold, posted when they change.
    fn post_enabled_context_keys(&mut self, window: &Window) {
        if self.welcome.media.as_ref().is_none_or(|media| media.checked_on.is_empty()) {
            return;
        }
        let context = self.welcome_context(window);
        let media = self.welcome.media.as_mut().expect("checked above");
        let enabled: Vec<String> = media.checked_on.iter().filter(|expression| walkthroughs::when_holds(Some(expression), &context)).cloned().collect();
        if media.enabled.as_ref() == Some(&enabled) {
            return;
        }
        media.webview.send("message", json!({ "message": { "enabledContextKeys": enabled }, "buffers": [] }));
        media.enabled = Some(enabled);
    }

    /// `renderSVG`, `renderMarkdown` and `renderVideo`: the document, its local resource roots and its `checked-on` expressions.
    fn media_html(&self, media: &Media) -> (String, Vec<PathBuf>, Vec<String>) {
        let nonce = format!("{:x}", now_millis() as u64);
        let styles = format!("{}\n{}", default_markdown_styles(&self.app_root), self.tokens_css());
        match media {
            Media::Svg { path, .. } => {
                let content = std::fs::read_to_string(path).unwrap_or_else(|error| {
                    eprintln!("Error reading svg document at `{}`: {error}", path.display());
                    String::new()
                });
                (render_svg(&nonce, &styles, &content), Vec::new(), Vec::new())
            }
            Media::Markdown { path, module, base, root } => {
                let text = match module {
                    Some(module) => walkthroughs::module_content(module).to_owned(),
                    None => std::fs::read_to_string(path).unwrap_or_else(|error| {
                        eprintln!("Error reading markdown document at `{}`: {error}", path.display());
                        String::new()
                    }),
                };
                let body = super::markdown_document::render_markdown_document(&transform_uris(&text, base), &WALKTHROUGH_SANITIZER);
                let checked_on = checked_on_expressions(&body);
                (render_markdown_document(&nonce, &styles, &body), vec![root.clone()], checked_on)
            }
            Media::Video { paths, poster, alt_text, root } => {
                let source = webview_uri(&theme_path(paths, self.theme.kind));
                let poster = poster.as_ref().map(|poster| format!("poster=\"{}\"", webview_uri(&theme_path(poster, self.theme.kind)))).unwrap_or_default();
                (render_video(&nonce, &source, &poster, alt_text), vec![root.clone()], Vec::new())
            }
            Media::Image { .. } => unreachable!("image media is drawn by GPUI"),
        }
    }

    /// `generateTokensCSSForColorMap` of the theme's token colors.
    fn tokens_css(&self) -> String {
        let mut css = String::new();
        for (index, color) in self.theme.tokens.color_map().iter().enumerate().skip(1) {
            css += &format!(".mtk{index} {{ color: {color}; }}\n");
        }
        css + ".mtki { font-style: italic; }\n.mtkb { font-weight: bold; }\n.mtku { text-decoration: underline; text-underline-position: under; }\n.mtks { text-decoration: line-through; }\n.mtks.mtku { text-decoration: underline line-through; text-underline-position: under; }"
    }

    /// The media webview's own event channel, read on the window's executor.
    fn welcome_webview_events(&mut self, window: &mut Window, cx: &mut Context<Self>) -> UnboundedSender<WebviewEvent> {
        if let Some(events) = &self.welcome.events {
            return events.clone();
        }
        let (sender, mut receiver) = unbounded();
        cx.spawn_in(window, async move |workbench, cx| {
            while let Some(event) = receiver.next().await {
                if workbench.update_in(cx, |workbench, window, cx| workbench.handle_welcome_webview_event(event, window, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        self.welcome.events = Some(sender.clone());
        sender
    }

    /// The media webview's `onMessage` (the theme picker's `setTheme:` and `command:`) and `onDidClickLink`.
    fn handle_welcome_webview_event(&mut self, event: WebviewEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(media) = &mut self.welcome.media else { return };
        match event.channel.as_str() {
            "webview-ready" => media.webview.did_become_ready(),
            "did-focus" => {
                media.webview.take_keyboard();
                window.blur(cx);
            }
            "onmessage" => {
                let message = event.data["message"].as_str().and_then(|text| serde_json::from_str::<Value>(text).ok());
                let message = message.as_ref().and_then(Value::as_str).unwrap_or_default().to_owned();
                if message.starts_with("command:") {
                    drop(self.open_link(Target::Text(message), OpenOptions { allow_commands: true, ..OpenOptions::default() }, window, cx));
                } else if let Some(theme) = message.strip_prefix("setTheme:") {
                    self.update_setting("workbench.colorTheme", json!(theme));
                    cx.notify();
                } else {
                    eprintln!("Unexpected message {message}");
                }
            }
            "did-click-link" => {
                // shortcut: `command:toSide:` opens in the one editor group; side groups come with editor splits (milestone 11).
                let link = event.data["uri"].as_str().unwrap_or_default().replacen("command:toSide:", "command:", 1);
                if ["https:", "http:", "command:"].iter().any(|scheme| link.starts_with(scheme)) {
                    drop(self.open_link(Target::Text(link), OpenOptions { allow_commands: true, ..OpenOptions::default() }, window, cx));
                }
            }
            _ => {}
        }
    }
}

fn now_millis() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0., |elapsed| elapsed.as_millis() as f64)
}

/// The first view of a side bar container, for `onView:` events.
// shortcut: only the side bar's shown container counts, not panel or auxiliary bar views; port with view visibility events (milestone 11).
fn side_bar_view_id(view: SideBarView) -> Option<&'static str> {
    match view {
        SideBarView::Explorer => Some("workbench.explorer.fileView"),
        SideBarView::Search => Some("workbench.view.search"),
        SideBarView::Scm => Some("workbench.scm"),
        SideBarView::Debug => Some("workbench.debug.variablesView"),
        SideBarView::Testing => Some("workbench.view.testing"),
        SideBarView::Extensions | SideBarView::Contributed(_) | SideBarView::Generated(_) => None,
    }
}

/// `command:<id>?<JSON arguments>`, the query URI-decoded first (`runStepCommand`).
fn command_args(command: &str) -> (String, Vec<Value>) {
    let Some((id, query)) = command.split_once('?') else { return (command.to_owned(), Vec::new()) };
    let decoded = crate::uri::percent_decode(query);
    let args = match serde_json::from_str::<Value>(&decoded).or_else(|_| serde_json::from_str::<Value>(query)) {
        Ok(Value::Array(args)) => args,
        Ok(arg) => vec![arg],
        Err(_) => Vec::new(),
    };
    (id.to_owned(), args)
}

fn theme_path(paths: &ThemePaths, kind: ThemeKind) -> String {
    match kind {
        ThemeKind::Dark => paths.dark.clone(),
        ThemeKind::Light => paths.light.clone(),
        ThemeKind::HighContrastDark => paths.hc_dark.clone(),
        ThemeKind::HighContrastLight => paths.hc_light.clone(),
    }
}

/// `asWebviewUri` of a local file; https URLs stay.
fn webview_uri(path: &str) -> String {
    if path.starts_with("https://") {
        return path.to_owned();
    }
    format!("https://file+.vscode-resource.vscode-cdn.net{}", path.replace('%', "%25").replace(' ', "%20").replace('#', "%23"))
}

/// gettingStartedDetailsRenderer.ts: links of any protocol (`override: '*'`), the media made absolute first.
const WALKTHROUGH_SANITIZER: Sanitizer = Sanitizer { link_protocols: None, media_protocols: &["http", "https"], allow_relative_media: false };

/// gettingStartedDetailsRenderer.ts `transformUris`: relative `src="..."` and `![](...)` to webview URIs under `base`.
fn transform_uris(content: &str, base: &Path) -> String {
    static PATTERNS: std::sync::OnceLock<(Regex, Regex)> = std::sync::OnceLock::new();
    let (src, image) = PATTERNS.get_or_init(|| (Regex::new(r#"src="([^"]*)""#).expect("src regex"), Regex::new(r"!\[([^\]]*)\]\(([^)]*)\)").expect("image regex")));
    let resolve = |source: &str| match source.starts_with("https://") {
        true => source.to_owned(),
        false => webview_uri(&base.join(source.trim_start_matches("./")).to_string_lossy()),
    };
    let content = src.replace_all(content, |captures: &regex::Captures| format!("src=\"{}\"", resolve(&captures[1])));
    image.replace_all(&content, |captures: &regex::Captures| format!("![{}]({})", &captures[1], resolve(&captures[2]))).into_owned()
}

/// The `checked-on` attributes of rendered markdown, collected as gettingStarted.ts does.
fn checked_on_expressions(html: &str) -> Vec<String> {
    static CHECKED_ON: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let pattern = CHECKED_ON.get_or_init(|| Regex::new(r#"checked-on="([^'][^"]*)""#).expect("checked-on regex"));
    pattern.captures_iter(html).map(|captures| captures[1].replace("&#39;", "'").replace("&amp;", "&")).collect()
}

/// `renderFormattedText` without its markup: `**bold**` and `__italic__` show as plain text.
// shortcut: inline bold and italic runs inside a wrapping paragraph are not drawn; port with styled text runs when a step needs them.
fn strip_formatting(text: &str) -> String {
    text.replace("**", "").replace("__", "")
}

/// markdownDocumentRenderer.ts `DEFAULT_MARKDOWN_STYLES`, read from the shipped source.
fn default_markdown_styles(app_root: &Path) -> &'static str {
    static STYLES: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    STYLES.get_or_init(|| {
        let source = std::fs::read_to_string(app_root.join("src/vs/workbench/contrib/markdown/browser/markdownDocumentRenderer.ts")).unwrap_or_default();
        let start = source.find("DEFAULT_MARKDOWN_STYLES = `").map(|start| start + "DEFAULT_MARKDOWN_STYLES = `".len());
        start.and_then(|start| source[start..].find('`').map(|end| source[start..start + end].to_owned())).unwrap_or_default()
    })
}

fn render_svg(nonce: &str, styles: &str, content: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html>
	<head>
		<meta http-equiv="Content-type" content="text/html;charset=UTF-8">
		<meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src data:; style-src 'nonce-{nonce}';">
		<style nonce="{nonce}">
			{styles}
			svg {{
				position: fixed;
				height: 100%;
				width: 80%;
				left: 50%;
				top: 50%;
				max-width: 530px;
				min-width: 350px;
				transform: translate(-50%,-50%);
			}}
		</style>
	</head>
	<body>
		{content}
	</body>
</html>"#
    )
}

fn render_markdown_document(nonce: &str, styles: &str, content: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html>
	<head>
		<meta http-equiv="Content-type" content="text/html;charset=UTF-8">
		<meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src https: data:; media-src https:; script-src 'nonce-{nonce}'; style-src 'nonce-{nonce}';">
		<style nonce="{nonce}">
			{styles}
			body > img {{ align-self: flex-start; }}
			body > img[centered] {{ align-self: center; }}
			body {{ display: flex; flex-direction: column; padding: 0; height: inherit; }}
			.theme-picker-row {{ display: flex; justify-content: center; gap: 32px; }}
			checklist {{ display: flex; gap: 32px; flex-direction: column; }}
			checkbox {{ display: flex; flex-direction: column; align-items: center; margin: 5px; cursor: pointer; }}
			checkbox > img {{ margin-bottom: 8px !important; }}
			checkbox.checked > img {{ box-sizing: border-box; }}
			checkbox.checked > img {{ outline: 2px solid var(--vscode-focusBorder); outline-offset: 4px; border-radius: 4px; }}
			.theme-picker-link {{ margin-top: 16px; color: var(--vscode-textLink-foreground); }}
			blockquote > p:first-child {{ margin-top: 0; }}
			body > * {{ margin-block-end: 0.25em; margin-block-start: 0.25em; }}
			vertically-centered {{ padding-top: 5px; padding-bottom: 5px; display: flex; justify-content: center; flex-direction: column; }}
			html {{ height: 100%; padding-right: 32px; }}
			h1 {{ font-size: 19.5px; }}
			h2 {{ font-size: 18.5px; }}
		</style>
	</head>
	<body>
		<vertically-centered>
			{content}
		</vertically-centered>
	</body>
	<script nonce="{nonce}">
		const vscode = acquireVsCodeApi();
		document.querySelectorAll('[when-checked]').forEach(el => {{
			el.addEventListener('click', () => {{
				vscode.postMessage(el.getAttribute('when-checked'));
			}});
		}});
		let ongoingLayout = undefined;
		const doLayout = () => {{
			document.querySelectorAll('vertically-centered').forEach(element => {{
				element.style.marginTop = Math.max((document.body.clientHeight - element.scrollHeight) * 3/10, 0) + 'px';
			}});
			ongoingLayout = undefined;
		}};
		const layout = () => {{
			if (ongoingLayout) {{
				clearTimeout(ongoingLayout);
			}}
			ongoingLayout = setTimeout(doLayout, 0);
		}};
		layout();
		document.querySelectorAll('img').forEach(element => {{
			element.onload = layout;
		}})
		window.addEventListener('message', event => {{
			if (event.data.layoutMeNow) {{
				layout();
			}}
			if (event.data.enabledContextKeys) {{
				document.querySelectorAll('.checked').forEach(element => element.classList.remove('checked'))
				for (const key of event.data.enabledContextKeys) {{
					document.querySelectorAll('[checked-on="' + key + '"]').forEach(element => element.classList.add('checked'))
				}}
			}}
		}});
	</script>
</html>"#
    )
}

fn render_video(nonce: &str, source: &str, poster: &str, description: &str) -> String {
    let label = if description.is_empty() { String::new() } else { format!("aria-label=\"{description}\"") };
    format!(
        r#"<!DOCTYPE html>
<html>
	<head>
		<meta http-equiv="Content-type" content="text/html;charset=UTF-8">
		<meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src https:; media-src https:; script-src 'nonce-{nonce}'; style-src 'nonce-{nonce}';">
		<style nonce="{nonce}">
			video {{ max-width: 100%; max-height: 100%; object-fit: cover; }}
		</style>
	</head>
	<body>
		<video controls autoplay {poster} muted {label}>
			<source src="{source}" type="video/mp4">
		</video>
	</body>
</html>"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transforms_media_uris_and_reads_checked_on() {
        let html = transform_uris(r#"<img src="./dark.png"/> <img src="https://x/y.png"> ![a](img/b.png)"#, Path::new("/media"));
        assert_eq!(html, r#"<img src="https://file+.vscode-resource.vscode-cdn.net/media/dark.png"/> <img src="https://x/y.png"> ![a](https://file+.vscode-resource.vscode-cdn.net/media/img/b.png)"#);
        let rendered = super::super::markdown_document::render_markdown_document(walkthroughs::module_content("theme_picker"), &super::WALKTHROUGH_SANITIZER);
        assert_eq!(checked_on_expressions(&rendered), ["config.workbench.colorTheme == 'Dark 2026'", "config.workbench.colorTheme == 'Light 2026'", "config.workbench.colorTheme == 'Default High Contrast'", "config.workbench.colorTheme == 'Default High Contrast Light'", "false"]);
        assert_eq!(command_args("a.b?%5B1%2C%22x%22%5D"), ("a.b".into(), vec![json!(1), json!("x")]));
        assert_eq!(command_args("notebook.setProfile?%7B%22profile%22%3A%22jupyter%22%7D"), ("notebook.setProfile".into(), vec![json!({ "profile": "jupyter" })]));
    }
}
