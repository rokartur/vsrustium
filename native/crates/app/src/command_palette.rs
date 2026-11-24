//! The picks of the Command Palette (`AbstractCommandsQuickAccessProvider._getPicks` of
//! platform/quickinput/browser/commandsQuickAccess.ts): commands whose label matches the filter by words, by
//! accent-insensitive substring or by exact id, then "similar commands" by TF-IDF; recently used first.

use std::cmp::Ordering;

use exthost::scan::ScannedExtension;
use serde_json::Value;

use crate::contextkey::{Context, Expr};
use crate::filters::{Matches, matches_base_contiguous_sub_string, matches_words, try_normalize_to_base};
use crate::tf_idf::{Document, TfIdfCalculator, normalize_scores};

const TFIDF_THRESHOLD: f64 = 0.5;
const TFIDF_MAX_RESULTS: usize = 5;
/// `CommandsHistory.DEFAULT_COMMANDS_HISTORY_LENGTH`.
pub const DEFAULT_HISTORY_LENGTH: usize = 50;

/// An enabled command of `MenuId.CommandPalette`. The alias (`title.original`) equals the label in English,
/// the only language here, so it is left out.
#[derive(Clone, Debug, PartialEq)]
pub struct CommandPick {
    pub id: String,
    /// "Category: Title", or the title alone.
    pub label: String,
    pub category: Option<String>,
}

#[derive(Debug, PartialEq)]
pub enum PaletteEntry {
    Separator(&'static str),
    Pick(Pick),
}

#[derive(Debug, PartialEq)]
pub struct Pick {
    pub id: String,
    pub label: String,
    /// The command id, when another pick has the same label.
    pub description: Option<String>,
    pub label_highlights: Matches,
    pub recently_used: bool,
}

/// `CommandsHistory`: an LRU of command ids, each with the counter value of its last run.
pub struct CommandsHistory {
    /// Least recently used first.
    entries: Vec<(String, u64)>,
    counter: u64,
    limit: usize,
}

impl CommandsHistory {
    pub fn new(limit: usize) -> CommandsHistory {
        CommandsHistory { entries: Vec::new(), counter: 1, limit }
    }

    pub fn push(&mut self, id: &str) {
        self.entries.retain(|(entry, _)| entry != id);
        self.entries.push((id.to_owned(), self.counter));
        self.counter += 1;
        if self.entries.len() > self.limit {
            self.entries.drain(..self.entries.len() - self.limit);
        }
    }

    pub fn peek(&self, id: &str) -> Option<u64> {
        self.entries.iter().find(|(entry, _)| entry == id).map(|(_, counter)| *counter)
    }

    pub fn remove(&mut self, id: &str) {
        self.entries.retain(|(entry, _)| entry != id);
    }

    /// `CommandsHistory.clearHistory`.
    pub fn clear(&mut self) {
        *self = CommandsHistory::new(self.limit);
    }

    /// The LRU's `limit` setter: the oldest entries past the new limit go.
    pub fn set_limit(&mut self, limit: usize) {
        self.limit = limit;
        if self.entries.len() > limit {
            self.entries.drain(..self.entries.len() - limit);
        }
    }

    /// `CommandsHistory.load`: the `commandPalette.mru.cache` and `commandPalette.mru.counter` storage values.
    /// A cache written before `usesLRU` lists its entries in any order, so they are sorted by counter.
    pub fn load(cache: Option<&str>, counter: Option<&str>, limit: usize) -> CommandsHistory {
        let mut history = CommandsHistory::new(limit);
        let serialized = cache.and_then(|raw| match serde_json::from_str::<Value>(raw) {
            Ok(value) => Some(value),
            Err(error) => {
                eprintln!("[CommandsHistory] invalid data: {error}");
                None
            }
        });
        if let Some(serialized) = serialized {
            let mut entries: Vec<(String, u64)> = serialized["entries"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|entry| Some((entry["key"].as_str()?.to_owned(), entry["value"].as_u64()?)))
                .collect();
            if serialized["usesLRU"] != true {
                entries.sort_by_key(|(_, counter)| *counter);
            }
            for (id, counter) in entries {
                history.entries.retain(|(entry, _)| *entry != id);
                history.entries.push((id, counter));
            }
            history.set_limit(limit);
        }
        if let Some(counter) = counter.and_then(|raw| raw.parse().ok()) {
            history.counter = counter;
        }
        history
    }

    /// `CommandsHistory.saveState`: the cache and the counter as `load` reads them.
    pub fn serialize(&self) -> (String, String) {
        let entries: Vec<Value> = self.entries.iter().map(|(key, value)| serde_json::json!({ "key": key, "value": value })).collect();
        (serde_json::json!({ "usesLRU": true, "entries": entries }).to_string(), self.counter.to_string())
    }
}

struct Candidate<'a> {
    command: &'a CommandPick,
    label_highlights: Matches,
    tf_idf_score: Option<f64>,
}

pub fn command_picks(commands: &[CommandPick], filter: &str, history: &CommandsHistory) -> Vec<PaletteEntry> {
    let mut tf_idf_scores = None;
    let mut candidates = Vec::new();
    for command in commands {
        if let Some(label_highlights) = word_filter(filter, &command.label) {
            candidates.push(Candidate { command, label_highlights, tf_idf_score: None });
        } else if filter == command.id {
            candidates.push(Candidate { command, label_highlights: Vec::new(), tf_idf_score: None });
        } else if filter.encode_utf16().count() >= 3 {
            let scores = tf_idf_scores.get_or_insert_with(|| tf_idf(commands, filter));
            if let Some(score) = scores.iter().find(|score| score.key == command.id) {
                candidates.push(Candidate { command, label_highlights: Vec::new(), tf_idf_score: Some(score.score) });
            }
        }
    }
    candidates.sort_by(|a, b| compare(a, b, history));

    let mut entries = Vec::new();
    let mut add_other_separator = false;
    let mut add_suggested_separator = true;
    for (index, candidate) in candidates.iter().enumerate() {
        let recently_used = history.peek(&candidate.command.id).is_some();
        if index == 0 && recently_used {
            entries.push(PaletteEntry::Separator("recently used"));
            add_other_separator = true;
        }
        if add_suggested_separator && candidate.tf_idf_score.is_some() {
            entries.push(PaletteEntry::Separator("similar commands"));
            add_suggested_separator = false;
        }
        if add_other_separator && candidate.tf_idf_score.is_none() && !recently_used {
            entries.push(PaletteEntry::Separator("other commands"));
            add_other_separator = false;
        }
        let label = &candidate.command.label;
        let duplicate = candidates.iter().filter(|other| other.command.label == *label).count() > 1;
        entries.push(PaletteEntry::Pick(Pick {
            id: candidate.command.id.clone(),
            label: label.clone(),
            description: duplicate.then(|| candidate.command.id.clone()),
            label_highlights: candidate.label_highlights.clone(),
            recently_used,
        }));
    }
    entries
}

/// An item of `MenuId.CommandPalette`: shown while `when` holds, enabled while `precondition` holds.
#[derive(Clone, Debug)]
pub struct MenuItem {
    pub id: String,
    pub title: String,
    pub category: Option<String>,
    pub when: Option<Expr>,
    pub precondition: Option<Expr>,
}

/// The workbench's palette items of the commands this port runs, with upstream's titles and categories:
/// `f1: true` actions and `appendToCommandPalette` (fileActions.contribution.ts), undo/redo/select all
/// (editorExtensions.ts), clipboard.ts, commandsQuickAccess.ts, preferences.contribution.ts, themes.contribution.ts,
/// stickyScrollActions.ts, mainThreadFileSystemEventService.ts. Each is the id, title, category, `when` and `precondition`.
const WORKBENCH_ITEMS: [(&str, &str, Option<&str>, Option<&str>, Option<&str>); 17] = [
    ("workbench.action.showCommands", "Show All Commands", None, None, None),
    ("workbench.action.clearCommandHistory", "Clear Command History", None, None, None),
    ("workbench.action.files.save", "Save", Some("File"), None, None),
    ("workbench.action.files.saveFiles", "Save All Files", Some("File"), Some("!isSessionsWindow"), None),
    ("workbench.action.files.revert", "Revert File", Some("File"), Some("!isSessionsWindow"), None),
    ("workbench.action.closeAllEditors", "Close All Editors", Some("View"), None, None),
    ("workbench.action.openGlobalKeybindingsFile", "Open Keyboard Shortcuts (JSON)", Some("Preferences"), None, None),
    ("workbench.action.selectTheme", "Color Theme", Some("Preferences"), None, None),
    ("files.participants.resetChoice", "Reset choice for 'File operation needs preview'", None, None, None),
    ("undo", "Undo", None, None, None),
    ("redo", "Redo", None, None, None),
    ("editor.action.selectAll", "Select All", None, None, None),
    ("editor.action.clipboardCutAction", "Cut", None, None, None),
    ("editor.action.clipboardCopyAction", "Copy", None, None, None),
    ("editor.action.clipboardPasteAction", "Paste", None, None, None),
    ("editor.action.toggleStickyScroll", "Toggle Editor Sticky Scroll", Some("View"), None, None),
    ("editor.action.focusStickyScroll", "Focus Editor Sticky Scroll", None, None, Some("config.editor.stickyScroll.enabled && stickyScrollVisible")),
];

/// `MenuRegistry.getMenuItems(MenuId.CommandPalette)`: the workbench's items, the extensions'
/// `contributes.menus.commandPalette` entries, then every other contributed command (`_appendImplicitItems`).
pub fn menu(extensions: &[ScannedExtension]) -> Vec<MenuItem> {
    let mut items: Vec<MenuItem> = WORKBENCH_ITEMS
        .iter()
        .map(|(id, title, category, when, precondition)| MenuItem {
            id: id.to_string(),
            title: title.to_string(),
            category: category.map(str::to_string),
            when: when.and_then(Expr::deserialize),
            precondition: precondition.and_then(Expr::deserialize),
        })
        .collect();
    let commands: Vec<MenuItem> = extensions.iter().flat_map(|extension| contributed_commands(&extension.description)).collect();
    for extension in extensions {
        for entry in extension.description["contributes"]["menus"]["commandPalette"].as_array().into_iter().flatten() {
            // menusExtensionPoint.ts skips entries of commands nobody contributes.
            let Some(command) = entry["command"].as_str().and_then(|id| commands.iter().find(|command| command.id == id)) else { continue };
            let when = entry["when"].as_str().and_then(Expr::deserialize);
            items.push(MenuItem { when, ..command.clone() });
        }
    }
    for command in commands {
        if !items.iter().any(|item| item.id == command.id) {
            items.push(command);
        }
    }
    items
}

/// `contributes.commands` as `MenuRegistry.addCommand` gets them: the `enablement` is the precondition.
fn contributed_commands(description: &Value) -> Vec<MenuItem> {
    let commands = match &description["contributes"]["commands"] {
        Value::Array(commands) => commands.iter().collect(),
        command @ Value::Object(_) => vec![command],
        _ => Vec::new(),
    };
    let localized = |value: &Value| value.as_str().or_else(|| value["value"].as_str()).map(str::to_string);
    let mut items = Vec::new();
    for command in commands {
        let (Some(id), Some(title)) = (command["command"].as_str(), localized(&command["title"])) else { continue };
        items.push(MenuItem {
            id: id.to_string(),
            title,
            category: localized(&command["category"]),
            when: None,
            precondition: command["enablement"].as_str().and_then(Expr::deserialize),
        });
    }
    items
}

/// `getGlobalCommandPicks`: the items whose `when` holds and that are enabled, labelled "Category: Title".
pub fn enabled_commands(menu: &[MenuItem], context: &dyn Context) -> Vec<CommandPick> {
    let holds = |expr: &Option<Expr>| expr.as_ref().is_none_or(|expr| expr.evaluate(context));
    let mut picks = Vec::new();
    for item in menu {
        if !holds(&item.when) || !holds(&item.precondition) {
            continue;
        }
        let label = match &item.category {
            // Upstream tests the category for truthiness, so emmet's `"category": ""` shows the title alone.
            Some(category) if !category.is_empty() => format!("{category}: {}", item.title),
            _ => item.title.clone(),
        };
        picks.push(CommandPick { id: item.id.clone(), label: strip_icons(&label), category: item.category.clone() });
    }
    picks
}

/// The `EditorAction`s of the active editor this port runs, by id, label and precondition (findController.ts,
/// folding.ts, multicursor.ts, linesOperations.ts).
const EDITOR_ACTIONS: &[(&str, &str, Option<&str>)] = &[
    ("editor.unfold", "Unfold", Some("foldingEnabled")),
    ("editor.unfoldRecursively", "Unfold Recursively", Some("foldingEnabled")),
    ("editor.fold", "Fold", Some("foldingEnabled")),
    ("editor.toggleFold", "Toggle Fold", Some("foldingEnabled")),
    ("editor.foldRecursively", "Fold Recursively", Some("foldingEnabled")),
    ("editor.toggleFoldRecursively", "Toggle Fold Recursively", Some("foldingEnabled")),
    ("editor.foldAllBlockComments", "Fold All Block Comments", Some("foldingEnabled")),
    ("editor.foldAllMarkerRegions", "Fold All Regions", Some("foldingEnabled")),
    ("editor.unfoldAllMarkerRegions", "Unfold All Regions", Some("foldingEnabled")),
    ("editor.foldAllExcept", "Fold All Except Selected", Some("foldingEnabled")),
    ("editor.unfoldAllExcept", "Unfold All Except Selected", Some("foldingEnabled")),
    ("editor.foldAll", "Fold All", Some("foldingEnabled")),
    ("editor.unfoldAll", "Unfold All", Some("foldingEnabled")),
    ("editor.gotoParentFold", "Go to Parent Fold", Some("foldingEnabled")),
    ("editor.gotoPreviousFold", "Go to Previous Folding Range", Some("foldingEnabled")),
    ("editor.gotoNextFold", "Go to Next Folding Range", Some("foldingEnabled")),
    ("editor.createFoldingRangeFromSelection", "Create Folding Range from Selection", Some("foldingEnabled")),
    ("editor.removeManualFoldingRanges", "Remove Manual Folding Ranges", Some("foldingEnabled")),
    ("editor.toggleImportFold", "Toggle Import Fold", Some("foldingEnabled")),
    ("editor.foldLevel1", "Fold Level 1", Some("foldingEnabled")),
    ("editor.foldLevel2", "Fold Level 2", Some("foldingEnabled")),
    ("editor.foldLevel3", "Fold Level 3", Some("foldingEnabled")),
    ("editor.foldLevel4", "Fold Level 4", Some("foldingEnabled")),
    ("editor.foldLevel5", "Fold Level 5", Some("foldingEnabled")),
    ("editor.foldLevel6", "Fold Level 6", Some("foldingEnabled")),
    ("editor.foldLevel7", "Fold Level 7", Some("foldingEnabled")),
    ("editor.action.insertCursorAbove", "Add Cursor Above", None),
    ("editor.action.insertCursorBelow", "Add Cursor Below", None),
    ("editor.action.insertCursorAtEndOfEachLineSelected", "Add Cursors to Line Ends", None),
    ("editor.action.addCursorsToBottom", "Add Cursors to Bottom", None),
    ("editor.action.addCursorsToTop", "Add Cursors to Top", None),
    ("deleteAllLeft", "Delete All Left", Some("!editorReadonly")),
    ("deleteAllRight", "Delete All Right", Some("!editorReadonly")),
    ("actions.find", "Find", Some("editorFocus || editorIsOpen")),
    ("editor.actions.findWithArgs", "Find with Arguments", None),
    ("actions.findWithSelection", "Find with Selection", None),
    ("editor.action.nextMatchFindAction", "Find Next", None),
    ("editor.action.previousMatchFindAction", "Find Previous", None),
    ("editor.action.goToMatchFindAction", "Go to Match...", Some("findWidgetVisible")),
    ("editor.action.nextSelectionMatchFindAction", "Find Next Selection", None),
    ("editor.action.previousSelectionMatchFindAction", "Find Previous Selection", None),
    ("editor.action.startFindReplaceAction", "Replace", Some("editorFocus || editorIsOpen")),
];

/// `getCodeEditorCommandPicks`: the active editor's supported actions, those whose precondition holds.
pub fn editor_action_picks(context: &dyn Context) -> Vec<CommandPick> {
    let mut picks = Vec::new();
    for (id, label, precondition) in EDITOR_ACTIONS {
        if precondition.and_then(Expr::deserialize).is_none_or(|expr| expr.evaluate(context)) {
            picks.push(CommandPick { id: id.to_string(), label: label.to_string(), category: None });
        }
    }
    picks
}

/// `stripIcons`: replaces each `(\s)?(\\)?$(name~modifier)(\s)?` by the whitespace before it, else the one
/// after it; an escaped `\$(name)` stays.
pub fn strip_icons(label: &str) -> String {
    let mut stripped = String::new();
    let mut rest = label;
    while let Some(start) = rest.find("$(") {
        let Some(length) = icon_length(&rest[start..]) else {
            stripped.push_str(&rest[..start + 2]);
            rest = &rest[start + 2..];
            continue;
        };
        if rest[..start].ends_with('\\') {
            stripped.push_str(&rest[..start + length]);
            rest = &rest[start + length..];
            continue;
        }
        let before = rest[..start].chars().next_back().filter(|c| c.is_whitespace());
        stripped.push_str(&rest[..start - before.map_or(0, char::len_utf8)]);
        rest = &rest[start + length..];
        let after = rest.chars().next().filter(|c| c.is_whitespace());
        rest = &rest[after.map_or(0, char::len_utf8)..];
        if let Some(whitespace) = before.or(after) {
            stripped.push(whitespace);
        }
    }
    stripped.push_str(rest);
    stripped
}

/// Length of the `$([A-Za-z0-9-]+(~[A-Za-z]+)?)` at the start of `text`.
fn icon_length(text: &str) -> Option<usize> {
    let inner = &text[2..];
    let end = inner.find(')')?;
    let (name, modifier) = inner[..end].split_once('~').map_or((&inner[..end], None), |(name, modifier)| (name, Some(modifier)));
    let name_ok = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    let modifier_ok = modifier.is_none_or(|modifier| !modifier.is_empty() && modifier.chars().all(|c| c.is_ascii_alphabetic()));
    (name_ok && modifier_ok).then_some(2 + end + 1)
}

/// `or(matchesBaseContiguousSubString, matchesWords)`.
fn word_filter(filter: &str, label: &str) -> Option<Matches> {
    matches_base_contiguous_sub_string(filter, label).or_else(|| matches_words(filter, label))
}

fn tf_idf(commands: &[CommandPick], filter: &str) -> Vec<crate::tf_idf::Score> {
    let mut calculator = TfIdfCalculator::default();
    calculator.update_documents(commands.iter().map(|command| Document { key: command.id.clone(), text_chunks: vec![command.label.clone()] }).collect());
    let mut scores = normalize_scores(calculator.calculate_scores(filter));
    scores.retain(|score| score.score > TFIDF_THRESHOLD);
    scores.truncate(TFIDF_MAX_RESULTS);
    scores
}

/// TF-IDF matches last, then most recently used, then not Developer commands, then by label.
fn compare(a: &Candidate, b: &Candidate, history: &CommandsHistory) -> Ordering {
    match (a.tf_idf_score, b.tf_idf_score) {
        (Some(a_score), Some(b_score)) if a_score == b_score => return locale_compare(&a.command.label, &b.command.label),
        (Some(a_score), Some(b_score)) => return b_score.total_cmp(&a_score),
        (Some(_), None) => return Ordering::Greater,
        (None, Some(_)) => return Ordering::Less,
        (None, None) => {}
    }
    match (history.peek(&a.command.id), history.peek(&b.command.id)) {
        (Some(a_counter), Some(b_counter)) => return b_counter.cmp(&a_counter),
        (Some(_), None) => return Ordering::Less,
        (None, Some(_)) => return Ordering::Greater,
        (None, None) => {}
    }
    let is_developer = |candidate: &Candidate| candidate.command.category.as_deref() == Some("Developer");
    match (is_developer(a), is_developer(b)) {
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        _ => locale_compare(&a.command.label, &b.command.label),
    }
}

/// `String.prototype.localeCompare` in English: accents and case only break ties, lower case first, and
/// whitespace, punctuation and symbols sort before digits and letters.
/// shortcut: a few rules of the root collation of ICU, which Rust std lacks; labels here are English.
pub fn locale_compare(a: &str, b: &str) -> Ordering {
    let primary = |text: &str| -> Vec<(u8, char)> {
        try_normalize_to_base(text)
            .chars()
            .map(|c| {
                let class = if c.is_whitespace() {
                    0
                } else if c.is_ascii_punctuation() {
                    1
                } else if c.is_numeric() {
                    3
                } else if c.is_alphabetic() {
                    4
                } else {
                    2
                };
                (class, c)
            })
            .collect()
    };
    let case = |text: &str| -> Vec<bool> { text.chars().map(char::is_uppercase).collect() };
    primary(a).cmp(&primary(b)).then_with(|| case(a).cmp(&case(b))).then_with(|| a.cmp(b))
}

#[cfg(test)]
mod tests;
