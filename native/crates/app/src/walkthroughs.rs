//! gettingStartedContent.ts and gettingStartedService.ts: the built-in walkthroughs and start entries, extension
//! `contributes.walkthroughs`, step progress (`memento/gettingStartedService`), walkthrough metadata and the
//! completion events that mark steps done.
//!
//! Left out of the built-in content: the four Copilot setup steps of Setup (`createCopilotSetupStep`), Beginner's
//! `settingsAndSync` (settings sync), the web-only SetupWeb walkthrough and the remote start entries (Open
//! Repository..., Connect to..., Open Tunnel...).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use regex::Regex;
use serde_json::{Map, Value, json};

use crate::contextkey::{Context, Expr};

/// `Memento('gettingStartedService')` in the profile scope: `{ stepId: { done: true } }`.
pub const PROGRESS_KEY: &str = "memento/gettingStartedService";
pub const HIDDEN_KEY: &str = "workbench.welcomePage.hiddenCategories";
pub const METADATA_KEY: &str = "workbench.welcomePage.walkthroughMetadata";
pub const BUILT_IN_SOURCE: &str = "Built-In";
const DAY_MS: f64 = 24. * 60. * 60. * 1000.;
const NEW_WALKTHROUGH_TIME: f64 = 7. * DAY_MS;
/// gettingStartedService.ts resolves built-in media under this module folder.
pub const MEDIA_FOLDER: &str = "src/vs/workbench/contrib/welcomeGettingStarted/common/media";

pub struct StartEntry {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub icon: &'static str,
    pub when: Option<&'static str>,
    pub command: &'static str,
}

/// `startEntries`, in order (`rankElement` is `-order`).
pub const START_ENTRIES: [StartEntry; 5] = [
    StartEntry { id: "welcome.showNewFileEntries", title: "New File...", description: "Open a new untitled text file, notebook, or custom editor.", icon: "new-file", when: None, command: "command:welcome.showNewFileEntries" },
    StartEntry { id: "topLevelOpenMac", title: "Open...", description: "Open a file or folder to start working", icon: "folder-opened", when: Some("!isWeb && isMac"), command: "command:workbench.action.files.openFileFolder" },
    StartEntry { id: "topLevelOpenFile", title: "Open File...", description: "Open a file to start working", icon: "go-to-file", when: Some("isWeb || !isMac"), command: "command:workbench.action.files.openFile" },
    StartEntry { id: "topLevelOpenFolderWeb", title: "Open Folder...", description: "Open a folder to start working", icon: "folder-opened", when: Some("!openFolderWorkspaceSupport && workbenchState == 'workspace'"), command: "command:workbench.action.files.openFolderViaWorkspace" },
    StartEntry { id: "topLevelGitClone", title: "Clone Git Repository...", description: "Clone a remote repository to a local folder", icon: "source-control", when: Some("config.git.enabled && !git.missing"), command: "command:git.clone" },
];

#[derive(Clone, Debug, PartialEq)]
pub enum Icon {
    Codicon(&'static str),
    Image(PathBuf),
}

/// An image or video per theme kind: `hcDark`, `hcLight`, `dark`, `light`.
#[derive(Clone, Debug, PartialEq)]
pub struct ThemePaths {
    pub dark: String,
    pub light: String,
    pub hc_dark: String,
    pub hc_light: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Media {
    Image { paths: ThemePaths, alt_text: String },
    Svg { path: PathBuf, alt_text: String },
    /// `path` is a file, or a `gettingStartedContentRegistry` module of `common/media` (`theme_picker`, `empty`).
    Markdown { path: PathBuf, module: Option<&'static str>, base: PathBuf, root: PathBuf },
    Video { paths: ThemePaths, poster: Option<ThemePaths>, alt_text: String, root: PathBuf },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Text(String),
    Link { label: String, href: String, title: Option<String> },
}

/// A `LinkedText`: one line of a step description.
pub type LinkedText = Vec<Node>;

#[derive(Clone, Debug)]
pub struct Step {
    pub id: String,
    pub title: String,
    pub description: Vec<LinkedText>,
    pub when: Option<String>,
    pub completion_events: Vec<String>,
    pub media: Media,
}

#[derive(Clone, Debug)]
pub struct Walkthrough {
    pub id: String,
    pub title: String,
    pub description: String,
    pub is_featured: bool,
    pub next: Option<String>,
    pub icon: Icon,
    pub when: Option<String>,
    pub page_title: String,
    pub order: usize,
    pub source: String,
    pub steps: Vec<Step>,
}

/// `WalkthroughMetaDataType` values; `manaullyOpened` is upstream's spelling of the stored key.
#[derive(Clone, Debug, PartialEq)]
pub struct Metadata {
    pub first_seen: f64,
    pub step_ids: Vec<String>,
    pub manually_opened: bool,
}

/// `WalkthroughsService`: the registered walkthroughs and the session's progress state.
#[derive(Default)]
pub struct Walkthroughs {
    pub walkthroughs: Vec<Walkthrough>,
    progress: Map<String, Value>,
    metadata: Vec<(String, Metadata)>,
    session_events: HashSet<String>,
    /// `completionListeners`: event to step ids, in registration order.
    listeners: Vec<(String, String)>,
}

/// linkedText.ts `LINK_REGEX` (the title's quote backreference written as two alternatives).
fn link_regex() -> &'static Regex {
    static LINK: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    LINK.get_or_init(|| Regex::new(r#"(?i)\[([^\]]+)\]\(((?:https?://|command:|file:)[^\)\s]+)(?: (?:"(.+?)"|'(.+?)'))?\)"#).expect("link regex"))
}

/// `parseLinkedText`.
pub fn parse_linked_text(text: &str) -> LinkedText {
    let mut nodes = Vec::new();
    let mut index = 0;
    for captures in link_regex().captures_iter(text) {
        let whole = captures.get(0).expect("match");
        if whole.start() > index {
            nodes.push(Node::Text(text[index..whole.start()].to_owned()));
        }
        let title = captures.get(3).or(captures.get(4)).map(|title| title.as_str().to_owned());
        nodes.push(Node::Link { label: captures[1].to_owned(), href: captures[2].to_owned(), title });
        index = whole.end();
    }
    if index < text.len() {
        nodes.push(Node::Text(text[index..].to_owned()));
    }
    nodes
}

/// `parseDescription`: one linked text per non-empty line.
pub fn parse_description(description: &str) -> Vec<LinkedText> {
    description.split('\n').filter(|line| !line.is_empty()).map(parse_linked_text).collect()
}

/// A line that is a lone link renders as a button (`buildMarkdownDescription`).
pub fn as_button(line: &LinkedText) -> Option<(&str, &str, Option<&str>)> {
    match line.as_slice() {
        [Node::Link { label, href, title }] => Some((label, href, title.as_deref())),
        _ => None,
    }
}

/// `registerDoneListeners`: the step's events, derived from its button links when it declares none, normalized
/// as the listener keys `progressByEvent` is called with.
pub fn completion_events(step: &Step) -> Vec<String> {
    let mut declared = step.completion_events.clone();
    if declared.is_empty() {
        for line in &step.description {
            let Some((_, href, _)) = as_button(line) else { continue };
            if let Some(command) = href.strip_prefix("command:") {
                declared.push(format!("onCommand:{}", command.split('?').next().unwrap_or(command)));
            } else if href.starts_with("https://") || href.starts_with("http://") {
                declared.push(format!("onLink:{href}"));
            }
        }
    }
    if declared.is_empty() {
        declared.push("stepSelected".into());
    }
    let mut events = Vec::new();
    for event in declared {
        let (kind, argument) = event.split_once(':').unwrap_or((&event, ""));
        let normalized = match kind {
            "onLink" | "onEvent" | "onView" | "onSettingChanged" => event.clone(),
            "onContext" => match Expr::deserialize(argument) {
                Some(expression) => format!("onContext:{}", expression.serialize()),
                None => {
                    eprintln!("Unable to parse context key expression: {argument} in walkthrough step {}", step.id);
                    continue;
                }
            },
            "onStepSelected" | "stepSelected" => format!("stepSelected:{}", step.id),
            "onCommand" => format!("onCommand:{}", argument.strip_prefix("toSide:").unwrap_or(argument)),
            "onExtensionInstalled" | "extensionInstalled" => format!("extensionInstalled:{}", argument.to_lowercase()),
            _ => {
                eprintln!("Unknown completionEvent {event} when registering step {}", step.id);
                continue;
            }
        };
        events.push(normalized);
    }
    events
}

/// A `when` clause that is absent or holds.
pub fn when_holds(when: Option<&str>, context: &dyn Context) -> bool {
    when.and_then(Expr::deserialize).is_none_or(|when| when.evaluate(context))
}

impl Walkthroughs {
    /// The stored progress and metadata; values that do not parse start empty, as `JSON.parse` of a bad value would throw upstream.
    pub fn load(progress: Option<&str>, metadata: Option<&str>) -> Walkthroughs {
        let progress = progress.and_then(|text| serde_json::from_str::<Map<String, Value>>(text).ok()).unwrap_or_default();
        let metadata = metadata.and_then(|text| serde_json::from_str::<Vec<(String, Value)>>(text).ok()).unwrap_or_default();
        let metadata = metadata
            .into_iter()
            .map(|(id, value)| {
                let step_ids = value["stepIDs"].as_array().into_iter().flatten().filter_map(|id| id.as_str().map(str::to_owned)).collect();
                (id, Metadata { first_seen: value["firstSeen"].as_f64().unwrap_or(0.), step_ids, manually_opened: value["manaullyOpened"] == true })
            })
            .collect();
        Walkthroughs { progress, metadata, ..Walkthroughs::default() }
    }

    /// `_registerWalkthrough`: a second walkthrough with a known id is skipped; the steps' completion listeners
    /// are registered, and `onContext:` events whose expression already holds are spent for the session.
    pub fn register(&mut self, walkthrough: Walkthrough, context: &dyn Context) {
        if self.walkthroughs.iter().any(|known| known.id == walkthrough.id) {
            eprintln!("Skipping attempt to overwrite walkthrough. ({})", walkthrough.id);
            return;
        }
        for step in &walkthrough.steps {
            for event in completion_events(step) {
                if let Some(expression) = event.strip_prefix("onContext:").and_then(Expr::deserialize)
                    && expression.evaluate(context)
                {
                    self.session_events.insert(event.clone());
                }
                self.listeners.push((event, step.id.clone()));
            }
        }
        self.walkthroughs.push(walkthrough);
    }

    /// `unregisterExtensionWalkthroughContributions`.
    pub fn unregister(&mut self, id: &str) {
        let Some(index) = self.walkthroughs.iter().position(|walkthrough| walkthrough.id == id) else { return };
        let removed = self.walkthroughs.remove(index);
        self.listeners.retain(|(_, step)| !removed.steps.iter().any(|removed| removed.id == *step));
    }

    pub fn get(&self, id: &str) -> Option<&Walkthrough> {
        self.walkthroughs.iter().find(|walkthrough| walkthrough.id == id)
    }

    /// The step and its walkthrough.
    pub fn step(&self, id: &str) -> Option<(&Walkthrough, &Step)> {
        self.walkthroughs.iter().find_map(|walkthrough| walkthrough.steps.iter().find(|step| step.id == id).map(|step| (walkthrough, step)))
    }

    pub fn is_done(&self, step: &str) -> bool {
        self.progress.get(step).is_some_and(|progress| progress["done"] == true)
    }

    /// `progressByEvent`: once per session per event; returns the steps it marked done.
    pub fn progress_by_event(&mut self, event: &str) -> Vec<String> {
        if !self.session_events.insert(event.to_owned()) {
            return Vec::new();
        }
        let steps: Vec<String> = self.listeners.iter().filter(|(listened, _)| listened == event).map(|(_, step)| step.clone()).collect();
        steps.into_iter().filter(|step| self.progress_step(step)).collect()
    }

    /// `progressStep`: whether the step was not done before.
    pub fn progress_step(&mut self, step: &str) -> bool {
        if self.is_done(step) {
            return false;
        }
        self.progress.insert(step.to_owned(), json!({ "done": true }));
        true
    }

    pub fn deprogress_step(&mut self, step: &str) {
        self.progress.remove(step);
    }

    /// The memento's ids, for Reset Welcome Page Walkthrough Progress.
    pub fn progressed_steps(&self) -> Vec<String> {
        self.progress.keys().cloned().collect()
    }

    pub fn progress_json(&self) -> String {
        Value::Object(self.progress.clone()).to_string()
    }

    /// The listened events of one kind (`onSettingChanged`, `onContext`, `onView`), without the prefix.
    pub fn listened(&self, kind: &str) -> Vec<String> {
        let mut found: Vec<String> = Vec::new();
        for (event, _) in &self.listeners {
            if let Some(argument) = event.strip_prefix(kind).and_then(|rest| rest.strip_prefix(':'))
                && !found.iter().any(|known| known == argument)
            {
                found.push(argument.to_owned());
            }
        }
        found
    }

    /// An extension walkthrough seen for the first time gets its metadata (`isNewlyInstalled`).
    pub fn note_seen(&mut self, walkthrough: &Walkthrough, now: f64) -> bool {
        if self.metadata.iter().any(|(id, _)| *id == walkthrough.id) {
            return false;
        }
        let step_ids = walkthrough.steps.iter().map(|step| step.id.rsplit('#').next().unwrap_or(&step.id).to_owned()).collect();
        self.metadata.push((walkthrough.id.clone(), Metadata { first_seen: now, step_ids, manually_opened: false }));
        true
    }

    /// `markWalkthroughOpened`.
    pub fn mark_opened(&mut self, id: &str) {
        let Some(steps) = self.get(id).map(|walkthrough| walkthrough.steps.iter().map(|step| step.id.clone()).collect::<Vec<_>>()) else { return };
        if let Some((_, metadata)) = self.metadata.iter_mut().find(|(known, _)| known == id) {
            metadata.manually_opened = true;
            metadata.step_ids = steps;
        }
    }

    pub fn metadata_json(&self) -> String {
        let entries: Vec<Value> = self.metadata.iter().map(|(id, metadata)| json!([id, { "firstSeen": metadata.first_seen, "stepIDs": metadata.step_ids, "manaullyOpened": metadata.manually_opened }])).collect();
        Value::Array(entries).to_string()
    }

    /// `resolveWalkthrough`: (newEntry, newItems, recencyBonus).
    pub fn freshness(&self, walkthrough: &Walkthrough, now: f64) -> (bool, bool, f64) {
        let Some((_, metadata)) = self.metadata.iter().find(|(id, _)| *id == walkthrough.id) else { return (false, false, 0.) };
        let is_new = metadata.first_seen > now - NEW_WALKTHROUGH_TIME;
        let current: Vec<&str> = walkthrough.steps.iter().map(|step| step.id.as_str()).collect();
        let new_items = current.len() != metadata.step_ids.len() || current.iter().zip(&metadata.step_ids).any(|(id, last)| id != last);
        let recency_bonus = ((NEW_WALKTHROUGH_TIME - (now - metadata.first_seen)) / NEW_WALKTHROUGH_TIME).max(0.);
        (is_new && !metadata.manually_opened, new_items, recency_bonus)
    }

    /// gettingStarted.ts `rankWalkthrough`: `None` for a hidden walkthrough.
    pub fn rank(&self, walkthrough: &Walkthrough, hidden: &[String], now: f64) -> Option<f64> {
        if hidden.iter().any(|id| *id == walkthrough.id) {
            return None;
        }
        let (new_entry, new_items, recency_bonus) = self.freshness(walkthrough, now);
        let mut rank = walkthrough.order as f64;
        if walkthrough.is_featured {
            rank += 7.;
        }
        if new_entry {
            rank += 3.;
        }
        if new_items {
            rank += 2.;
        }
        Some(rank + 4. * recency_bonus)
    }

    /// The Walkthroughs list: ranked (stable), then filtered by `when`, at most `limit`; and the count before the limit.
    pub fn ranked(&self, hidden: &[String], now: f64, context: &dyn Context, limit: usize) -> (Vec<&Walkthrough>, usize) {
        let mut ranked: Vec<(&Walkthrough, f64)> = self.walkthroughs.iter().filter(|walkthrough| !walkthrough.steps.is_empty()).filter_map(|walkthrough| Some((walkthrough, self.rank(walkthrough, hidden, now)?))).collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
        let active: Vec<&Walkthrough> = ranked.into_iter().map(|(walkthrough, _)| walkthrough).filter(|walkthrough| when_holds(walkthrough.when.as_deref(), context)).collect();
        let count = active.len();
        (active.into_iter().take(limit).collect(), count)
    }

    /// `getWalkthroughCompletionStats`: done and total of the steps whose `when` holds.
    pub fn stats(&self, walkthrough: &Walkthrough, context: &dyn Context) -> (usize, usize) {
        let active: Vec<&Step> = walkthrough.steps.iter().filter(|step| when_holds(step.when.as_deref(), context)).collect();
        (active.iter().filter(|step| self.is_done(&step.id)).count(), active.len())
    }
}

/// `workbench.welcomePage.hiddenCategories`.
pub fn parse_hidden(stored: Option<&str>) -> Vec<String> {
    stored.and_then(|text| serde_json::from_str(text).ok()).unwrap_or_default()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StartupEditor {
    Readme,
    WelcomePage,
    Terminal,
}

/// startupPage.ts `isStartupPageEnabled` and `run`: what opens at startup. `user_set` is whether the user or
/// workspace sets `workbench.startupEditor`; `old_enabled` is `workbench.welcome.enabled`.
pub fn startup_editor(value: &str, user_set: bool, old_enabled: Option<&Value>, skip_welcome: bool, empty_workbench: bool, has_active_editor: bool) -> Option<StartupEditor> {
    if skip_welcome || has_active_editor {
        return None;
    }
    let old = old_enabled.filter(|value| !value.is_null() && !user_set);
    let enabled = match old {
        Some(old) => old.as_bool().unwrap_or(true),
        None => matches!(value, "welcomePage" | "readme" | "terminal") || (empty_workbench && value == "welcomePageInEmptyWorkbench"),
    };
    if !enabled {
        return None;
    }
    match value {
        "readme" => Some(StartupEditor::Readme),
        "welcomePage" | "welcomePageInEmptyWorkbench" => Some(StartupEditor::WelcomePage),
        "terminal" => Some(StartupEditor::Terminal),
        _ => None,
    }
}

/// The `common/media` module content providers (`gettingStartedContentRegistry`).
pub fn module_content(module: &str) -> &'static str {
    match module {
        "theme_picker" => THEME_PICKER,
        "notebookProfile" => NOTEBOOK_PROFILE,
        _ => "",
    }
}

const THEME_PICKER: &str = r#"
<checklist>
	<div class="theme-picker-row">
		<checkbox when-checked="setTheme:Dark 2026" checked-on="config.workbench.colorTheme == 'Dark 2026'">
			<img width="200" src="./dark.png"/>
			Dark Modern
		</checkbox>
		<checkbox when-checked="setTheme:Light 2026" checked-on="config.workbench.colorTheme == 'Light 2026'">
			<img width="200" src="./light.png"/>
			Light Modern
		</checkbox>
	</div>
	<div class="theme-picker-row">
		<checkbox when-checked="setTheme:Default High Contrast" checked-on="config.workbench.colorTheme == 'Default High Contrast'">
			<img width="200" src="./dark-hc.png"/>
			Dark High Contrast
		</checkbox>
		<checkbox when-checked="setTheme:Default High Contrast Light" checked-on="config.workbench.colorTheme == 'Default High Contrast Light'">
			<img width="200" src="./light-hc.png"/>
			Light High Contrast
		</checkbox>
	</div>
</checklist>
<checkbox class="theme-picker-link" when-checked="command:workbench.action.selectTheme" checked-on="false">
	See More Themes...
</checkbox>
"#;

const NOTEBOOK_PROFILE: &str = r#"
<vertically-centered>
<checklist>
	<checkbox on-checked="command:notebook.setProfile?%7B%22profile%22%3A%22default%22%7D" checked-on="config.notebook.cellFocusIndicator == 'border' && config.notebook.insertToolbarLocation == 'both' && config.notebook.globalToolbar == false && config.notebook.compactView == true && config.notebook.showCellStatusBar == 'visible'">
		<img width="400" src="./notebookThemes/default.png"/>
		Default
	</checkbox>
	<checkbox on-checked="command:notebook.setProfile?%7B%22profile%22%3A%22jupyter%22%7D" checked-on="config.notebook.cellFocusIndicator == 'gutter' && config.notebook.insertToolbarLocation == 'notebookToolbar' && config.notebook.globalToolbar == true && config.notebook.compactView == true  && config.notebook.showCellStatusBar == 'visible'">
		<img width="400" src="./notebookThemes/jupyter.png"/>
		Jupyter
	</checkbox>
	<checkbox on-checked="command:notebook.setProfile?%7B%22profile%22%3A%22colab%22%7D" checked-on="config.notebook.cellFocusIndicator == 'border' && config.notebook.insertToolbarLocation == 'betweenCells' && config.notebook.globalToolbar == false && config.notebook.compactView == false && config.notebook.showCellStatusBar == 'hidden'">
		<img width="400" src="./notebookThemes/colab.png"/>
		Colab
	</checkbox>
</checklist>
</vertically-centered>
"#;

struct BuiltInStep {
    id: &'static str,
    title: &'static str,
    description: &'static str,
    when: Option<&'static str>,
    completion_events: &'static [&'static str],
    /// `svg:<file>` with its alt text, or `markdown:<module>`.
    media: (&'static str, &'static str),
}

struct BuiltIn {
    id: &'static str,
    title: &'static str,
    description: &'static str,
    is_featured: bool,
    icon: &'static str,
    when: Option<&'static str>,
    next: Option<&'static str>,
    page_title: &'static str,
    steps: &'static [BuiltInStep],
}

const fn step(id: &'static str, title: &'static str, description: &'static str, when: Option<&'static str>, media: (&'static str, &'static str)) -> BuiltInStep {
    BuiltInStep { id, title, description, when, completion_events: &[], media }
}

const EMPTY: (&str, &str) = ("markdown:empty", "");

const BUILT_INS: [BuiltIn; 4] = [
    BuiltIn {
        id: "Setup",
        title: "Get started with VS Code",
        description: "Customize your editor, learn the basics, and start coding",
        is_featured: true,
        icon: "zap",
        when: Some("!isWeb"),
        next: Some("Beginner"),
        page_title: "Setup VS Code",
        steps: &[
            BuiltInStep {
                id: "pickColorTheme",
                title: "Choose your theme",
                description: "The right theme helps you focus on your code, is easy on your eyes, and is simply more fun to use.\n[Browse Color Themes](command:workbench.action.selectTheme)",
                when: None,
                completion_events: &["onSettingChanged:workbench.colorTheme", "onCommand:workbench.action.selectTheme"],
                media: ("markdown:theme_picker", ""),
            },
            step("videoTutorial", "Watch video tutorials", "Watch the first in a series of short & practical video tutorials for VS Code's key features.\n[Watch Tutorial](https://aka.ms/vscode-getting-started-video)", None, ("svg:learn.svg", "VS Code Settings")),
        ],
    },
    BuiltIn {
        id: "SetupAccessibility",
        title: "Get Started with Accessibility Features",
        description: "Learn the tools and shortcuts that make VS Code accessible. Note that some actions are not actionable from within the context of the walkthrough.",
        is_featured: true,
        icon: "zap",
        when: Some("accessibilityModeEnabled"),
        next: Some("Setup"),
        page_title: "Setup VS Code Accessibility",
        steps: &[
            step("accessibilityHelp", "Use the accessibility help dialog to learn about features", "The accessibility help dialog provides information about what to expect from a feature and the commands/keybindings to operate them.\n With focus in an editor, terminal, notebook, chat response, comment, or debug console, the relevant dialog can be opened with the Open Accessibility Help command.\n[Open Accessibility Help](command:editor.action.accessibilityHelp)", None, EMPTY),
            step("accessibleView", "Screen reader users can inspect content line by line, character by character in the accessible view.", "The accessible view is available for the terminal, hovers, notifications, comments, notebook output, chat responses, inline completions, and debug console output.\n With focus in any of those features, it can be opened with the Open Accessible View command.\n[Open Accessible View](command:editor.action.accessibleView)", None, EMPTY),
            step("verbositySettings", "Control the verbosity of aria labels", "Screen reader verbosity settings exist for features around the workbench so that once a user is familiar with a feature, they can avoid hearing hints about how to operate it. For example, features for which an accessibility help dialog exists will indicate how to open the dialog until the verbosity setting for that feature has been disabled.\n These and other accessibility settings can be configured by running the Open Accessibility Settings command.\n[Open Accessibility Settings](command:workbench.action.openAccessibilitySettings)", None, EMPTY),
            step("commandPaletteTaskAccessibility", "Unlock productivity with the Command Palette ", "Run commands without reaching for your mouse to accomplish any task in VS Code.\n[Open Command Palette](command:workbench.action.showCommands)", None, EMPTY),
            step("keybindingsAccessibility", "Customize your keyboard shortcuts", "Once you have discovered your favorite commands, create custom keyboard shortcuts for instant access.\n[Keyboard Shortcuts](command:toSide:workbench.action.openGlobalKeybindings)", None, EMPTY),
            step("accessibilitySignals", "Fine tune which accessibility signals you want to receive via audio or a braille device", "Accessibility sounds and announcements are played around the workbench for different events.\n These can be discovered and configured using the List Signal Sounds and List Signal Announcements commands.\n[List Signal Sounds](command:signals.sounds.help)\n[List Signal Announcements](command:accessibility.announcement.help)", None, EMPTY),
            step("hover", "Access the hover in the editor to get more information on a variable or symbol", "While focus is in the editor on a variable or symbol, a hover can be focused with the Show or Open Hover command.\n[Show or Focus Hover](command:editor.action.showHover)", None, EMPTY),
            step("goToSymbol", "Navigate to symbols in a file", "The Go to Symbol command is useful for navigating between important landmarks in a document.\n[Go to Symbol](command:editor.action.goToSymbol)", None, EMPTY),
            step("codeFolding", "Use code folding to collapse blocks of code and focus on the code you're interested in.", "Fold or unfold a code section with the Toggle Fold command.\n[Toggle Fold](command:editor.toggleFold)\n Fold or unfold recursively with the Toggle Fold Recursively Command\n[Toggle Fold Recursively](command:editor.toggleFoldRecursively)\n", None, EMPTY),
            step("intellisense", "Use Intellisense to improve coding efficiency", "Intellisense suggestions can be opened with the Trigger Intellisense command.\n[Trigger Intellisense](command:editor.action.triggerSuggest)\n Inline intellisense suggestions can be triggered with Trigger Inline Suggestion\n[Trigger Inline Suggestion](command:editor.action.inlineSuggest.trigger)\n Useful settings include editor.inlineCompletionsAccessibilityVerbose and editor.screenReaderAnnounceInlineSuggestion.", None, EMPTY),
            step("accessibilitySettings", "Configure accessibility settings", "Accessibility settings can be configured by running the Open Accessibility Settings command.\n[Open Accessibility Settings](command:workbench.action.openAccessibilitySettings)", None, EMPTY),
            step("dictation", "Use dictation to write code and text in the editor and terminal", "Dictation allows you to write code and text using your voice. It can be activated with the Voice: Start Dictation in Editor command.\n[Voice: Start Dictation in Editor](command:workbench.action.editorDictation.start)\n For dictation in the terminal, use the Voice: Start Dictation in Terminal and Voice: Stop Dictation in Terminal commands.\n[Terminal: Start Dictation in Terminal](command:workbench.action.terminal.startVoice)\n[Terminal: Stop Dictation in Terminal](command:workbench.action.terminal.stopVoice)", Some("hasSpeechProvider"), EMPTY),
        ],
    },
    BuiltIn {
        id: "Beginner",
        title: "Learn the Fundamentals",
        description: "Get an overview of the most essential features",
        is_featured: false,
        icon: "lightbulb",
        when: None,
        next: None,
        page_title: "Essential Features",
        steps: &[
            step("extensions", "Code with extensions", "Extensions are VS Code's power-ups. They range from handy productivity hacks, expanding out-of-the-box features, to adding completely new capabilities.\n[Browse Popular Extensions](command:workbench.extensions.action.showPopularExtensions)", Some("workspacePlatform != 'webworker'"), ("svg:extensions.svg", "VS Code extension marketplace with featured language extensions")),
            step("terminal", "Built-in terminal", "Quickly run shell commands and monitor build output, right next to your code.\n[Open Terminal](command:workbench.action.terminal.toggleTerminal)", Some("workspacePlatform != 'webworker' && remoteName != codespaces && !terminalIsOpen"), ("svg:terminal.svg", "Integrated terminal running a few npm commands")),
            step("debugging", "Watch your code in action", "Accelerate your edit, build, test, and debug loop by setting up a launch configuration.\n[Run your Project](command:workbench.action.debug.selectandstart)", Some("workspacePlatform != 'webworker' && workspaceFolderCount != 0"), ("svg:debug.svg", "Run and debug view.")),
            step("scmClone", "Track your code with Git", "Set up the built-in version control for your project to track your changes and collaborate with others.\n[Clone Repository](command:git.clone)", Some("config.git.enabled && !git.missing && workspaceFolderCount == 0"), ("svg:git.svg", "Source Control view.")),
            step("scmSetup", "Track your code with Git", "Set up the built-in version control for your project to track your changes and collaborate with others.\n[Initialize Git Repository](command:git.init)", Some("config.git.enabled && !git.missing && workspaceFolderCount != 0 && gitOpenRepositoryCount == 0"), ("svg:git.svg", "Source Control view.")),
            step("scm", "Track your code with Git", "No more looking up Git commands! Git and GitHub workflows are seamlessly integrated.\n[Open Source Control](command:workbench.view.scm)", Some("config.git.enabled && !git.missing && workspaceFolderCount != 0 && gitOpenRepositoryCount != 0 && activeViewlet != 'workbench.view.scm'"), ("svg:git.svg", "Source Control view.")),
            BuiltInStep {
                id: "installGit",
                title: "Install Git",
                description: "Install Git to track changes in your projects.\n[Install Git](https://aka.ms/vscode-install-git)\n[Reload window](command:workbench.action.reloadWindow) after installation to complete Git setup.",
                when: Some("git.missing"),
                completion_events: &["onContext:git.state == initialized"],
                media: ("svg:git.svg", "Install Git."),
            },
            step("tasks", "Automate your project tasks", "Create tasks for your common workflows and enjoy the integrated experience of running scripts and automatically checking results.\n[Run Auto-detected Tasks](command:workbench.action.tasks.runTask)", Some("workspaceFolderCount != 0 && workspacePlatform != 'webworker'"), ("svg:runTask.svg", "Task runner.")),
            step("shortcuts", "Customize your shortcuts", "Once you have discovered your favorite commands, create custom keyboard shortcuts for instant access.\n[Keyboard Shortcuts](command:toSide:workbench.action.openGlobalKeybindings)", None, ("svg:shortcuts.svg", "Interactive shortcuts.")),
            step("workspaceTrust", "Safely browse and edit code", "[Workspace Trust](https://code.visualstudio.com/docs/editor/workspace-trust) lets you decide whether your project folders should **allow or restrict** automatic code execution __(required for extensions, debugging, etc)__.\nOpening a file/folder will prompt to grant trust. You can always [enable trust](command:toSide:workbench.trust.manage) later.", Some("workspacePlatform != 'webworker' && !isWorkspaceTrusted && workspaceFolderCount == 0"), ("svg:workspaceTrust.svg", "Workspace Trust editor in Restricted mode and a primary button for switching to Trusted mode.")),
        ],
    },
    BuiltIn {
        id: "notebooks",
        title: "Customize Notebooks",
        description: "",
        is_featured: false,
        icon: "zap",
        when: Some("config.notebook.experimental.openGettingStarted && userHasOpenedNotebook"),
        next: None,
        page_title: "Notebooks",
        steps: &[BuiltInStep {
            id: "notebookProfile",
            title: "Select the layout for your notebooks",
            description: "Get notebooks to feel just the way you prefer",
            when: Some("userHasOpenedNotebook"),
            completion_events: &["onCommand:notebook.setProfile"],
            media: ("markdown:notebookProfile", ""),
        }],
    },
];

/// `registerWalkthroughs`: the built-in walkthroughs, `order` counting down so the first ranks highest.
pub fn built_in(app_root: &Path) -> Vec<Walkthrough> {
    let media_root = app_root.join(MEDIA_FOLDER);
    let count = BUILT_INS.len();
    BUILT_INS
        .iter()
        .enumerate()
        .map(|(index, walkthrough)| Walkthrough {
            id: walkthrough.id.into(),
            title: walkthrough.title.into(),
            description: walkthrough.description.into(),
            is_featured: walkthrough.is_featured,
            next: walkthrough.next.map(Into::into),
            icon: Icon::Codicon(walkthrough.icon),
            when: walkthrough.when.map(Into::into),
            page_title: walkthrough.page_title.into(),
            order: count - index,
            source: BUILT_IN_SOURCE.into(),
            steps: walkthrough.steps.iter().map(|step| built_in_step(step, &media_root)).collect(),
        })
        .collect()
}

fn built_in_step(step: &BuiltInStep, media_root: &Path) -> Step {
    let (kind, alt_text) = step.media;
    let media = match kind.split_once(':') {
        Some(("svg", file)) => Media::Svg { path: media_root.join(file), alt_text: alt_text.into() },
        Some(("markdown", module)) => Media::Markdown { path: media_root.join(module), module: Some(module_name(module)), base: media_root.to_owned(), root: media_root.to_owned() },
        _ => unreachable!("built-in media is svg or markdown"),
    };
    Step {
        id: step.id.into(),
        title: step.title.into(),
        description: parse_description(step.description),
        when: step.when.map(Into::into),
        completion_events: step.completion_events.iter().map(|event| event.to_string()).collect(),
        media,
    }
}

fn module_name(module: &str) -> &'static str {
    match module {
        "theme_picker" => "theme_picker",
        "notebookProfile" => "notebookProfile",
        _ => "empty",
    }
}

/// `registerExtensionWalkthroughContributions` for one extension: ids `<extension id>#<walkthrough id>` and
/// `<extension id>#<walkthrough id>#<step id>`. A step without media is dropped with the error upstream throws.
// shortcut: `featuredFor` globs are not matched against the workspace, so extension walkthroughs are never featured; port when a caller needs it.
pub fn from_extension(extension_id: &str, location: &Path, manifest: &Value) -> Vec<Walkthrough> {
    let Some(walkthroughs) = manifest["contributes"]["walkthroughs"].as_array() else { return Vec::new() };
    let name = manifest["displayName"].as_str().or(manifest["name"].as_str()).unwrap_or(extension_id).to_owned();
    let mut result = Vec::new();
    for walkthrough in walkthroughs {
        let Some(walkthrough_id) = walkthrough["id"].as_str() else { continue };
        let id = format!("{extension_id}#{walkthrough_id}");
        let mut steps = Vec::new();
        for step in walkthrough["steps"].as_array().into_iter().flatten() {
            let step_id = format!("{id}#{}", step["id"].as_str().unwrap_or_default());
            let Some(media) = extension_media(location, &step["media"], &step_id) else { continue };
            steps.push(Step {
                id: step_id,
                title: step["title"].as_str().unwrap_or_default().to_owned(),
                description: parse_description(step["description"].as_str().unwrap_or_default()),
                when: step["when"].as_str().map(str::to_owned),
                completion_events: step["completionEvents"].as_array().into_iter().flatten().filter_map(|event| event.as_str().map(str::to_owned)).collect(),
                media,
            });
        }
        let icon = walkthrough["icon"].as_str().or(manifest["icon"].as_str());
        result.push(Walkthrough {
            id,
            title: walkthrough["title"].as_str().unwrap_or_default().to_owned(),
            description: walkthrough["description"].as_str().unwrap_or_default().to_owned(),
            is_featured: false,
            next: None,
            icon: icon.map_or(Icon::Codicon("extensions"), |icon| Icon::Image(location.join(icon))),
            when: walkthrough["when"].as_str().map(str::to_owned),
            page_title: name.clone(),
            order: 0,
            source: name.clone(),
            steps,
        });
    }
    result
}

fn extension_media(location: &Path, media: &Value, step_id: &str) -> Option<Media> {
    if media.is_null() {
        eprintln!("missing media in walkthrough step: {step_id}");
        return None;
    }
    if !media["image"].is_null() {
        return Some(Media::Image { paths: theme_paths(location, &media["image"]), alt_text: media["altText"].as_str().unwrap_or_default().to_owned() });
    }
    if let Some(markdown) = media["markdown"].as_str() {
        let path = location.join(markdown);
        let base = path.parent().unwrap_or(location).to_owned();
        return Some(Media::Markdown { path, module: None, base, root: location.to_owned() });
    }
    if let Some(svg) = media["svg"].as_str() {
        return Some(Media::Svg { path: location.join(svg), alt_text: svg.to_owned() });
    }
    if !media["video"].is_null() {
        let poster = (!media["poster"].is_null()).then(|| theme_paths(location, &media["poster"]));
        return Some(Media::Video { paths: theme_paths(location, &media["video"]), poster, alt_text: media["altText"].as_str().unwrap_or_default().to_owned(), root: location.to_owned() });
    }
    eprintln!("Unknown walkthrough format detected for {step_id}");
    None
}

/// `convertExtensionRelativePathsToBrowserURIs`: one path for every theme kind, or `{ hc, hcLight?, dark, light }`.
fn theme_paths(location: &Path, value: &Value) -> ThemePaths {
    let convert = |path: &str| if path.starts_with("https://") { path.to_owned() } else { location.join(path).to_string_lossy().into_owned() };
    if let Some(path) = value.as_str() {
        let path = convert(path);
        return ThemePaths { dark: path.clone(), light: path.clone(), hc_dark: path.clone(), hc_light: path };
    }
    let field = |key: &str| value[key].as_str().map(convert).unwrap_or_default();
    let light = field("light");
    ThemePaths { dark: field("dark"), hc_dark: field("hc"), hc_light: value["hcLight"].as_str().map(convert).unwrap_or_else(|| light.clone()), light }
}

/// Context keys the walkthrough `when` clauses read that only exist in the web or remote workbench.
pub fn platform_context(context: &mut HashMap<String, Value>) {
    context.entry("openFolderWorkspaceSupport".into()).or_insert(json!(true));
    context.entry("workspacePlatform".into()).or_insert(json!("mac"));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(pairs: &[(&str, Value)]) -> HashMap<String, Value> {
        let mut context: HashMap<String, Value> = pairs.iter().map(|(key, value)| (key.to_string(), value.clone())).collect();
        platform_context(&mut context);
        context
    }

    fn walkthrough(id: &str, order: usize, featured: bool, steps: Vec<Step>) -> Walkthrough {
        Walkthrough { id: id.into(), title: id.into(), description: String::new(), is_featured: featured, next: None, icon: Icon::Codicon("zap"), when: None, page_title: id.into(), order, source: BUILT_IN_SOURCE.into(), steps }
    }

    fn plain_step(id: &str, description: &str, events: &[&str]) -> Step {
        Step { id: id.into(), title: id.into(), description: parse_description(description), when: None, completion_events: events.iter().map(|event| event.to_string()).collect(), media: Media::Svg { path: PathBuf::new(), alt_text: String::new() } }
    }

    #[test]
    fn parses_linked_text() {
        assert_eq!(parse_linked_text("[Open](command:a.b?%5B1%5D)"), vec![Node::Link { label: "Open".into(), href: "command:a.b?%5B1%5D".into(), title: None }]);
        assert_eq!(
            parse_linked_text("a [x](https://e.com \"T\") b [y](ftp://no)"),
            vec![Node::Text("a ".into()), Node::Link { label: "x".into(), href: "https://e.com".into(), title: Some("T".into()) }, Node::Text(" b [y](ftp://no)".into())]
        );
        assert_eq!(parse_description("one\n\ntwo").len(), 2);
    }

    #[test]
    fn derives_completion_events_from_buttons() {
        let step = plain_step("s", "text [inline](command:not.a.button)\n[Button](command:toSide:workbench.action.openSettings?%5B%5D)\n[Site](https://example.com)", &[]);
        assert_eq!(completion_events(&step), ["onCommand:workbench.action.openSettings", "onLink:https://example.com"]);
        assert_eq!(completion_events(&plain_step("s", "no links", &[])), ["stepSelected:s"]);
        let declared = plain_step("s", "[B](command:x)", &["onCommand:toSide:a.b", "extensionInstalled:Pub.Ext", "onContext:git.state == initialized", "bogus:x"]);
        assert_eq!(completion_events(&declared), ["onCommand:a.b", "extensionInstalled:pub.ext", "onContext:git.state == 'initialized'"]);
    }

    #[test]
    fn progresses_by_event_once_per_session() {
        let mut service = Walkthroughs::load(Some(r#"{"old":{"done":true}}"#), None);
        let steps = vec![plain_step("a", "[Run](command:x.run)", &[]), plain_step("b", "", &["onContext:ready", "onSettingChanged:editor.fontSize"])];
        service.register(walkthrough("w", 1, false, steps), &context(&[("ready", json!(true))]));
        assert!(service.is_done("old"));
        assert_eq!(service.progress_by_event("onCommand:x.run"), ["a"]);
        service.deprogress_step("a");
        assert!(service.progress_by_event("onCommand:x.run").is_empty(), "an event counts once per session");
        // The context already held at registration: the event is spent.
        assert!(service.progress_by_event("onContext:ready").is_empty());
        assert_eq!(service.listened("onSettingChanged"), ["editor.fontSize"]);
        assert_eq!(service.progress_by_event("onSettingChanged:editor.fontSize"), ["b"]);
        assert_eq!(serde_json::from_str::<Value>(&service.progress_json()).unwrap(), json!({ "old": { "done": true }, "b": { "done": true } }));
        let (done, total) = service.stats(service.get("w").unwrap(), &context(&[]));
        assert_eq!((done, total), (1, 2));
    }

    #[test]
    fn ranks_walkthroughs() {
        let now = 100. * DAY_MS;
        let mut service = Walkthroughs::default();
        let built_ins = built_in(Path::new("/app"));
        for walkthrough in built_ins {
            service.register(walkthrough, &context(&[]));
        }
        let extension = walkthrough("pub.ext#tour", 0, false, vec![plain_step("pub.ext#tour#one", "", &[])]);
        assert!(service.note_seen(&extension, now - DAY_MS));
        service.register(extension, &context(&[]));
        // Setup 4 + 7 featured; the extension 0 + 3 new + 2 new items (raw step ids stored) + 4 * 6/7 recency; Beginner 2.
        let ids = |list: Vec<&Walkthrough>| list.iter().map(|walkthrough| walkthrough.id.clone()).collect::<Vec<_>>();
        let (shown, count) = service.ranked(&[], now, &context(&[]), 5);
        assert_eq!(ids(shown), ["Setup", "pub.ext#tour", "Beginner"]);
        assert_eq!(count, 3);
        let (shown, _) = service.ranked(&["Setup".into()], now, &context(&[("accessibilityModeEnabled", json!(true))]), 5);
        assert_eq!(ids(shown), ["SetupAccessibility", "pub.ext#tour", "Beginner"]);
        service.mark_opened("pub.ext#tour");
        let (new_entry, new_items, recency_bonus) = service.freshness(service.get("pub.ext#tour").unwrap(), now);
        assert_eq!((new_entry, new_items), (false, false));
        assert!((recency_bonus - 6. / 7.).abs() < 1e-9);
        let stored = Walkthroughs::load(None, Some(&service.metadata_json()));
        assert_eq!(stored.metadata, service.metadata);
    }

    #[test]
    fn filters_start_entries_and_steps_by_when() {
        let shown = |context: &HashMap<String, Value>| START_ENTRIES.iter().filter(|entry| when_holds(entry.when, context)).map(|entry| entry.id).collect::<Vec<_>>();
        assert_eq!(shown(&context(&[("config.git.enabled", json!(true))])), ["welcome.showNewFileEntries", "topLevelOpenMac", "topLevelGitClone"]);
        assert_eq!(shown(&context(&[("config.git.enabled", json!(true)), ("git.missing", json!(true)), ("workbenchState", json!("workspace"))])), ["welcome.showNewFileEntries", "topLevelOpenMac"]);
        let beginner = &built_in(Path::new("/app"))[2];
        let steps = |context: &HashMap<String, Value>| beginner.steps.iter().filter(|step| when_holds(step.when.as_deref(), context)).map(|step| step.id.as_str()).collect::<Vec<_>>();
        let empty_window = context(&[("config.git.enabled", json!(true)), ("workspaceFolderCount", json!(0)), ("isWorkspaceTrusted", json!(false))]);
        assert_eq!(steps(&empty_window), ["extensions", "terminal", "scmClone", "shortcuts", "workspaceTrust"]);
    }

    #[test]
    fn decides_the_startup_editor() {
        assert_eq!(startup_editor("welcomePage", false, None, false, true, false), Some(StartupEditor::WelcomePage));
        assert_eq!(startup_editor("welcomePage", false, None, true, true, false), None);
        assert_eq!(startup_editor("welcomePage", false, None, false, true, true), None);
        assert_eq!(startup_editor("welcomePageInEmptyWorkbench", true, None, false, false, false), None);
        assert_eq!(startup_editor("welcomePageInEmptyWorkbench", true, None, false, true, false), Some(StartupEditor::WelcomePage));
        assert_eq!(startup_editor("none", true, None, false, true, false), None);
        // `workbench.welcome.enabled` decides while `workbench.startupEditor` is not set by the user.
        assert_eq!(startup_editor("welcomePage", false, Some(&json!(false)), false, true, false), None);
        assert_eq!(startup_editor("readme", true, Some(&json!(false)), false, false, false), Some(StartupEditor::Readme));
    }

    #[test]
    fn reads_extension_walkthroughs() {
        let manifest = json!({ "displayName": "Ext", "contributes": { "walkthroughs": [{ "id": "tour", "title": "Tour", "description": "D", "steps": [
            { "id": "one", "title": "One", "description": "[Go](command:ext.go)", "media": { "image": { "light": "l.png", "dark": "d.png", "hc": "h.png" }, "altText": "A" } },
            { "id": "two", "title": "Two", "description": "", "media": { "markdown": "docs/two.md" } },
            { "id": "bad", "title": "Bad", "description": "" }
        ] }] } });
        let walkthroughs = from_extension("pub.ext", Path::new("/ext"), &manifest);
        assert_eq!(walkthroughs[0].id, "pub.ext#tour");
        assert_eq!(walkthroughs[0].source, "Ext");
        let ids: Vec<&str> = walkthroughs[0].steps.iter().map(|step| step.id.as_str()).collect();
        assert_eq!(ids, ["pub.ext#tour#one", "pub.ext#tour#two"]);
        assert_eq!(walkthroughs[0].steps[0].media, Media::Image { paths: ThemePaths { dark: "/ext/d.png".into(), light: "/ext/l.png".into(), hc_dark: "/ext/h.png".into(), hc_light: "/ext/l.png".into() }, alt_text: "A".into() });
        assert_eq!(walkthroughs[0].steps[1].media, Media::Markdown { path: "/ext/docs/two.md".into(), module: None, base: "/ext/docs".into(), root: "/ext".into() });
    }
}
