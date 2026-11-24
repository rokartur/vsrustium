//! The Keyboard Shortcuts editor's model (workbench/services/preferences/browser/keybindingsEditorModel.ts):
//! one row per keybinding plus one per unbound command, sorted by title, and the search over them with
//! `@command:`, `+when:`, `@source:`, `@ext:` and `@keybinding:`.

#[cfg(test)]
mod tests;

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::command_palette::{EDITOR_ACTIONS, MenuItem, locale_compare, word_filter};
use crate::contextkey::Expr;
use crate::filters::{Matches, matches_camel_case, matches_contiguous_sub_string, matches_contiguous_words, matches_words};
use crate::keybindings::{self, Chord, ExtensionSource, Item};

static COMMAND_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)@command:\s*([^+]+)").unwrap());
static WHEN_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\+when:\s*(.+)").unwrap());
static SOURCE_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)@source:\s*(user|default|system|extension)").unwrap());
static EXTENSION_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?i)@ext:\s*((".+")|([^\s]+))"#).unwrap());
static KEYBINDING_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?i)@keybinding:\s*((".+")|(\S+))"#).unwrap());

#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    User,
    System,
    Contributed(ExtensionSource),
}

impl Source {
    /// `SOURCE_USER`, `SOURCE_SYSTEM`; `None` for an extension.
    pub fn label(&self) -> Option<&'static str> {
        match self {
            Source::User => Some("User"),
            Source::System => Some("System"),
            Source::Contributed(_) => None,
        }
    }
}

/// `IKeybindingItem`: a row of the table.
#[derive(Clone, Debug)]
pub struct Row {
    pub keybinding: Vec<Chord>,
    pub command: String,
    pub command_label: String,
    pub args: Option<Value>,
    /// The serialized `when`, empty without one.
    pub when: String,
    pub is_default: bool,
    pub source: Source,
}

impl Row {
    /// `KeybindingsEditorModel.getId`.
    pub fn id(&self) -> String {
        let source = match &self.source {
            Source::Contributed(extension) => extension.id.clone(),
            source => source.label().expect("only extensions have no label").to_string(),
        };
        format!("{}{}{}{}", self.command, keybindings::aria_label(&self.keybinding), self.when, source)
    }
}

/// Which parts of one chord a search matched (`KeybindingMatch`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ChordMatch {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
    pub key: bool,
}

impl ChordMatch {
    fn any(&self) -> bool {
        self.ctrl || self.shift || self.alt || self.meta || self.key
    }
}

/// `IKeybindingItemEntry`: a row and what the search matched in it.
#[derive(Clone, Debug, Default)]
pub struct EntryMatches {
    pub command_label: Option<Matches>,
    pub command_id: Option<Matches>,
    pub source: Option<Matches>,
    pub when: Option<Matches>,
    pub keybinding: Option<[ChordMatch; 2]>,
    pub extension_id: Option<Matches>,
    pub extension_label: Option<Matches>,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub row: Row,
    pub matches: EntryMatches,
}


#[derive(Default)]
pub struct Model {
    rows: Vec<Row>,
    rows_by_precedence: Vec<Row>,
}

impl Model {
    /// `resolve`: `keybindings` in the resolver's order, `defaults` the default keybindings.
    /// `registered` are the ids of `CommandsRegistry`.
    pub fn new(keybindings: &[Item], defaults: &[Item], menu: &[MenuItem], registered: &[&str]) -> Model {
        let commands: HashMap<&str, &MenuItem> = menu.iter().rev().map(|item| (item.id.as_str(), item)).collect();
        let mut by_precedence = Vec::new();
        let mut bound = HashSet::new();
        for item in keybindings {
            if item.command.is_empty() {
                continue;
            }
            by_precedence.push(row(&item.command, item, &commands));
            bound.insert(item.command.as_str());
        }
        let with_defaults: HashSet<&str> = defaults.iter().map(|item| item.command.as_str()).collect();
        let candidates = menu.iter().map(|item| item.id.as_str()).chain(EDITOR_ACTIONS.iter().map(|action| action.0)).chain(registered.iter().copied());
        for command in unbound_commands(candidates, &bound) {
            let item = Item::new(Vec::new(), command, None, None, !with_defaults.contains(command));
            by_precedence.push(row(command, &item, &commands));
        }
        let mut seen = HashSet::new();
        by_precedence.retain(|row| seen.insert(row.id()));
        let mut rows = by_precedence.clone();
        rows.sort_by(compare_rows);
        Model { rows, rows_by_precedence: by_precedence }
    }

    pub fn fetch(&self, search: &str, sort_by_precedence: bool) -> Vec<Entry> {
        let mut rows: Vec<&Row> = if sort_by_precedence { self.rows_by_precedence.iter().collect() } else { self.rows.iter().collect() };
        let mut search = search.to_string();
        if let Some(command) = COMMAND_REGEX.captures(&search).map(|captures| captures[1].trim().to_string()) {
            let mut filtered: Vec<Row> = rows.into_iter().filter(|row| row.command == command).cloned().collect();
            if !filtered.is_empty()
                && let Some(when) = WHEN_REGEX.captures(&search)
            {
                filtered = filter_by_when(filtered, &command, when[1].trim());
            }
            return filtered.into_iter().map(Entry::unmatched).collect();
        }
        if SOURCE_REGEX.is_match(&search) {
            rows = filter_by_source(rows, &search);
            search = SOURCE_REGEX.replacen(&search, 1, "").into_owned();
        } else if let Some(captures) = EXTENSION_REGEX.captures(&search) {
            let extension = match captures.get(2) {
                Some(quoted) => quoted.as_str()[1..quoted.len() - 1].to_string(),
                None => captures[3].to_string(),
            };
            rows = filter_by_extension(rows, &extension);
            search = EXTENSION_REGEX.replacen(&search, 1, "").into_owned();
        } else if let Some(captures) = KEYBINDING_REGEX.captures(&search) {
            search = match captures.get(2) {
                Some(quoted) => quoted.as_str().to_string(),
                None => format!("\"{}\"", &captures[3]),
            };
        }
        let search = search.trim();
        if search.is_empty() {
            return rows.into_iter().cloned().map(Entry::unmatched).collect();
        }
        filter_by_text(rows, search)
    }
}

impl Entry {
    fn unmatched(row: Row) -> Entry {
        Entry { row, matches: EntryMatches::default() }
    }
}

/// `toKeybindingEntry`: `commands` is `MenuRegistry.getCommand`, the first item of each id.
fn row(command: &str, item: &Item, commands: &HashMap<&str, &MenuItem>) -> Row {
    let menu_command = commands.get(command);
    // A command without keybinding takes the source of the extension that contributed it.
    let contributed = if item.keybinding.is_empty() { menu_command.and_then(|command| command.source.as_ref()) } else { None };
    let source = match item.extension.as_ref().or(contributed) {
        _ if !item.is_default => Source::User,
        Some(extension) => Source::Contributed(extension.clone()),
        None => Source::System,
    };
    let command_label = match menu_command {
        Some(MenuItem { category: Some(category), title, .. }) if !category.is_empty() => format!("{category}: {title}"),
        Some(command) => command.title.clone(),
        None => EDITOR_ACTIONS.iter().find(|action| action.0 == command).map(|action| action.1.to_string()).unwrap_or_default(),
    };
    Row {
        keybinding: item.keybinding.clone(),
        command: command.to_string(),
        command_label,
        args: item.args.clone(),
        when: item.when.as_ref().map(Expr::serialize).unwrap_or_default(),
        is_default: item.is_default,
        source,
    }
}

/// `getAllUnboundCommands`: private commands (`_`, `vscode.`) are left out.
// shortcut: the registry has no command metadata, so commands with `args` metadata are listed too; upgrade when it does.
fn unbound_commands<'a>(candidates: impl Iterator<Item = &'a str>, bound: &HashSet<&str>) -> Vec<&'a str> {
    let mut seen = HashSet::new();
    let mut unbound = Vec::new();
    for id in candidates {
        if !seen.insert(id) || id.starts_with('_') || id.starts_with("vscode.") || bound.contains(id) {
            continue;
        }
        unbound.push(id);
    }
    unbound
}

/// `compareKeybindingData`: keybindings first, then titled commands by title, a user row before the default.
fn compare_rows(a: &Row, b: &Row) -> Ordering {
    match (a.keybinding.is_empty(), b.keybinding.is_empty()) {
        (false, true) => return Ordering::Less,
        (true, false) => return Ordering::Greater,
        _ => {}
    }
    match (a.command_label.is_empty(), b.command_label.is_empty()) {
        (false, true) => return Ordering::Less,
        (true, false) => return Ordering::Greater,
        (false, false) if a.command_label != b.command_label => return locale_compare(&a.command_label, &b.command_label),
        _ => {}
    }
    if a.command == b.command {
        return if a.is_default { Ordering::Greater } else { Ordering::Less };
    }
    locale_compare(&a.command, &b.command)
}

fn filter_by_source<'a>(rows: Vec<&'a Row>, search: &str) -> Vec<&'a Row> {
    let source = SOURCE_REGEX.captures(search).expect("checked by the caller")[1].to_lowercase();
    match source.as_str() {
        "default" | "system" => rows.into_iter().filter(|row| row.source == Source::System).collect(),
        "user" => rows.into_iter().filter(|row| row.source == Source::User).collect(),
        _ => rows.into_iter().filter(|row| matches!(row.source, Source::Contributed(_))).collect(),
    }
}

fn filter_by_extension<'a>(rows: Vec<&'a Row>, extension: &str) -> Vec<&'a Row> {
    let extension = extension.trim().to_lowercase();
    rows.into_iter()
        .filter(|row| match &row.source {
            Source::Contributed(source) => source.id.to_lowercase() == extension || source.display_name.as_ref().is_some_and(|name| name.to_lowercase() == extension),
            _ => false,
        })
        .collect()
}

/// `filterByWhen`: the rows with that `when`, else a new row of the command with it, which is not in the model.
fn filter_by_when(rows: Vec<Row>, command: &str, when: &str) -> Vec<Row> {
    let with_when: Vec<Row> = rows.iter().filter(|row| row.when == when).cloned().collect();
    if !with_when.is_empty() {
        return with_when;
    }
    let when = Expr::deserialize(when);
    let row = Row { keybinding: Vec::new(), command: command.to_string(), args: None, when: when.as_ref().map(Expr::serialize).unwrap_or_default(), is_default: false, source: Source::User, ..rows[0].clone() };
    vec![row]
}

fn filter_by_text(rows: Vec<&Row>, search: &str) -> Vec<Entry> {
    let complete_match = search.starts_with('"') && search.ends_with('"');
    let search = search.strip_prefix('"').unwrap_or(search);
    let search = search.strip_suffix('"').unwrap_or(search).trim();
    let words: Vec<&str> = search.split(' ').collect();
    let keybinding_words: Vec<&str> = words.iter().flat_map(|word| word.split('+')).filter(|word| !word.is_empty()).collect();
    let mut entries = Vec::new();
    for row in rows {
        let matches = item_matches(row, search, &words, &keybinding_words, complete_match);
        let any = matches.command_id.is_some()
            || matches.command_label.is_some()
            || matches.source.is_some()
            || matches.when.is_some()
            || matches.keybinding.is_some()
            || matches.extension_id.is_some()
            || matches.extension_label.is_some();
        if any {
            entries.push(Entry { row: row.clone(), matches });
        }
    }
    entries
}

/// `KeybindingItemMatches`.
fn item_matches(row: &Row, search: &str, words: &[&str], keybinding_words: &[&str], complete_match: bool) -> EntryMatches {
    let mut matches = EntryMatches::default();
    if !complete_match {
        let words_or_camel_case = |word: &str, target: &str| matches_words(word, target).or_else(|| matches_camel_case(word, target));
        matches.command_id = text_matches(Some(search), &row.command, words_or_camel_case, words);
        if !row.command_label.is_empty() {
            matches.command_label = text_matches(Some(search), &row.command_label, matches_contiguous_words, words);
        }
        if !row.when.is_empty() {
            matches.when = text_matches(None, &row.when, words_or_camel_case, words);
        }
        match &row.source {
            Source::Contributed(extension) => {
                // Upstream matches the words against the command label here, not the extension's name.
                if let Some(display_name) = &extension.display_name {
                    matches.extension_label = text_matches(Some(search), display_name, |word, _| matches_contiguous_words(word, &row.command_label), words);
                }
            }
            source => {
                let label = source.label().expect("only extensions have no label");
                matches.source = text_matches(Some(search), label, matches_contiguous_words, words);
            }
        }
    }
    if !row.keybinding.is_empty() {
        matches.keybinding = keybinding_matches(&row.keybinding, search, keybinding_words, complete_match);
    }
    matches
}

/// `KeybindingItemMatches.matches`: the whole search first, then every word on its own.
fn text_matches(search: Option<&str>, target: &str, word_filter_of: impl Fn(&str, &str) -> Option<Matches>, words: &[&str]) -> Option<Matches> {
    let whole = search.and_then(|search| word_filter(search, target));
    let matches = whole.or_else(|| {
        let mut all = Vec::new();
        for word in words {
            all.extend(word_filter_of(word, target)?);
        }
        Some(all)
    })?;
    Some(filter_and_sort(matches))
}

/// Distinct ranges, without those inside another range, by start.
fn filter_and_sort(matches: Matches) -> Matches {
    let mut distinct: Matches = Vec::new();
    for range in &matches {
        if !distinct.contains(range) {
            distinct.push(range.clone());
        }
    }
    let mut kept: Matches = distinct.iter().filter(|range| !matches.iter().any(|other| other != *range && other.start <= range.start && other.end >= range.end)).cloned().collect();
    kept.sort_by_key(|range| range.start);
    kept
}

/// `matchesKeybinding`: modifier words and the key match the first chord, the next words the second.
fn keybinding_matches(keybinding: &[Chord], search: &str, words: &[&str], complete_match: bool) -> Option<[ChordMatch; 2]> {
    let first = keybinding.first();
    let second = keybinding.get(1);
    let labels = [keybindings::user_settings_label(keybinding), keybindings::aria_label(keybinding), keybindings::label(keybinding)];
    if labels.iter().any(|label| !label.is_empty() && label.to_lowercase() == search.to_lowercase()) {
        return Some([complete(first), complete(second)]);
    }
    let mut first_match = ChordMatch::default();
    let mut second_match = ChordMatch::default();
    let mut matched_words: Vec<usize> = Vec::new();
    let mut first_matched_words: Vec<usize> = Vec::new();
    let mut second_matched_words: Vec<usize> = Vec::new();
    let mut match_first = true;
    for (index, word) in words.iter().enumerate() {
        let mut first_matched = false;
        let mut second_matched = false;
        match_first = match_first && !first_match.key;
        let mut match_second = !second_match.key;
        if match_first {
            first_matched = match_part(first, &mut first_match, word, complete_match);
            if first_match.key {
                for word_index in &second_matched_words {
                    if !first_matched_words.contains(word_index) {
                        let position = matched_words.iter().position(|matched| matched == word_index).expect("a chord's words are matched words");
                        matched_words.remove(position);
                    }
                }
                second_match = ChordMatch::default();
                second_matched_words.clear();
                match_second = false;
            }
        }
        if match_second {
            second_matched = match_part(second, &mut second_match, word, complete_match);
        }
        if first_matched {
            first_matched_words.push(index);
        }
        if second_matched {
            second_matched_words.push(index);
        }
        if first_matched || second_matched {
            matched_words.push(index);
        }
        match_first = match_first && is_modifier(word);
    }
    if matched_words.len() != words.len() {
        return None;
    }
    if complete_match && (!is_complete_match(first, &first_match) || (second_match.any() && !is_complete_match(second, &second_match))) {
        return None;
    }
    (first_match.any() || second_match.any()).then_some([first_match, second_match])
}

fn match_part(chord: Option<&Chord>, matched: &mut ChordMatch, word: &str, complete_match: bool) -> bool {
    let Some(chord) = chord else { return false };
    let mut any = false;
    if chord.meta && is_meta(word) {
        matched.meta = true;
        any = true;
    }
    if chord.ctrl && is_ctrl(word) {
        matched.ctrl = true;
        any = true;
    }
    if chord.shift && is_shift(word) {
        matched.shift = true;
        any = true;
    }
    if chord.alt && is_alt(word) {
        matched.alt = true;
        any = true;
    }
    if matches_key(chord, word, complete_match) {
        matched.key = true;
        any = true;
    }
    any
}

/// `matchesKeyCode` against the key's aria label.
fn matches_key(chord: &Chord, word: &str, complete_match: bool) -> bool {
    let label = chord.key_aria_label();
    if complete_match || label.chars().count() == 1 || word.chars().count() == 1 {
        return label.to_lowercase() == word.to_lowercase();
    }
    matches_contiguous_sub_string(word, label).is_some()
}

fn is_complete_match(chord: Option<&Chord>, matched: &ChordMatch) -> bool {
    let Some(chord) = chord else { return true };
    matched.key && (!chord.meta || matched.meta) && (!chord.alt || matched.alt) && (!chord.ctrl || matched.ctrl) && (!chord.shift || matched.shift)
}

fn complete(chord: Option<&Chord>) -> ChordMatch {
    let Some(chord) = chord else { return ChordMatch::default() };
    ChordMatch { ctrl: chord.ctrl, shift: chord.shift, alt: chord.alt, meta: chord.meta, key: true }
}

fn is_modifier(word: &str) -> bool {
    is_alt(word) || is_ctrl(word) || is_meta(word) || is_shift(word)
}

/// The macOS modifier labels of the UI, aria and user settings label providers, plus upstream's extra words.
fn is_one_of(word: &str, labels: &[&str]) -> bool {
    labels.iter().any(|label| label.to_lowercase() == word.to_lowercase())
}

fn is_alt(word: &str) -> bool {
    is_one_of(word, &["⌥", "Option", "alt", "option"])
}

fn is_ctrl(word: &str) -> bool {
    is_one_of(word, &["\u{2303}", "Control", "ctrl"])
}

fn is_meta(word: &str) -> bool {
    is_one_of(word, &["⌘", "Command", "cmd", "meta"])
}

fn is_shift(word: &str) -> bool {
    is_one_of(word, &["⇧", "Shift", "shift"])
}
