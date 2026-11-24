//! workbench/services/keybinding/common/keybindingEditing.ts: the edits the Keyboard Shortcuts editor makes to
//! `keybindings.json`. Each change is computed on the file's text with `setProperty`, so comments survive.

#[cfg(test)]
mod tests;

use config::json::{self, ParseOptions, Segment};
use config::json_edit::{FormattingOptions, apply_edit, set_property};
use serde_json::{Map, Value, json};

use crate::contextkey::Expr;
use crate::keybindings;
use crate::keybindings_editor::Row;

const EMPTY_CONTENT: &str = "// Place your key bindings in this file to override the defaults\n[\n]";

pub enum Change<'a> {
    /// `addKeybinding`: a new user keybinding of the row's command.
    Add { key: &'a str, when: Option<&'a str> },
    /// `editKeybinding`: changes the user's keybinding, or replaces a default one with a user's.
    Edit { key: &'a str, when: Option<&'a str> },
    Remove,
    /// `resetKeybinding`: drops the user's keybinding and the removals of the command's defaults.
    Reset,
}

/// `doEditKeybinding`, `doRemoveKeybinding` and `doResetKeybinding` on the text of `keybindings.json`; `None` when
/// the file does not exist.
pub fn apply(content: Option<&str>, row: &Row, change: Change, options: &FormattingOptions) -> Result<String, String> {
    let mut content = valid_content(content.unwrap_or(""), options)?;
    match change {
        Change::Add { key, when } => update(&mut content, row, key, when, None, options),
        Change::Edit { key, when } => {
            let index = user_entry_index(&content, row);
            update(&mut content, row, key, when, index, options);
            if row.is_default && !row.keybinding.is_empty() {
                remove_default(&mut content, row, options);
            }
        }
        Change::Remove if row.is_default => remove_default(&mut content, row, options),
        Change::Remove => remove_user(&mut content, row, options),
        Change::Reset if row.is_default => {}
        Change::Reset => {
            remove_user(&mut content, row, options);
            remove_unassigned_defaults(&mut content, row, options);
        }
    }
    Ok(content)
}

/// `resolveAndValidate`: an empty file gets the header and an array, one with only comments an array.
fn valid_content(content: &str, options: &FormattingOptions) -> Result<String, String> {
    if content.is_empty() {
        return Ok(EMPTY_CONTENT.to_string());
    }
    let mut errors = Vec::new();
    let parsed = json::parse(content, &mut errors, ParseOptions { allow_trailing_comma: true, allow_empty_content: true, ..ParseOptions::default() });
    if !errors.is_empty() {
        return Err("Unable to write to the keybindings configuration file. Please open it to correct errors/warnings in the file and try again.".into());
    }
    match parsed {
        Some(Value::Array(_)) => Ok(content.to_string()),
        Some(_) => Err("Unable to write to the keybindings configuration file. It has an object which is not of type Array. Please open the file to clean up and try again.".into()),
        None => Ok(format!("{content}{}[]", options.eol)),
    }
}

fn entries(content: &str) -> Vec<Value> {
    match json::parse(content, &mut Vec::new(), ParseOptions::default()) {
        Some(Value::Array(entries)) => entries,
        _ => Vec::new(),
    }
}

fn set(content: &mut String, path: &[Segment], value: Option<&Value>, options: &FormattingOptions) {
    if let Some(edit) = set_property(content, path, value, options).first() {
        *content = apply_edit(content, edit);
    }
}

/// `updateKeybinding`: `index` is the user's entry to change, `None` appends one.
fn update(content: &mut String, row: &Row, key: &str, when: Option<&str>, index: Option<usize>, options: &FormattingOptions) {
    match index {
        Some(index) => {
            set(content, &[Segment::Index(index), Segment::Key("key".into())], Some(&json!(key)), options);
            set(content, &[Segment::Index(index), Segment::Key("when".into())], when.map(|when| json!(when)).as_ref(), options);
        }
        None => set(content, &[Segment::Index(usize::MAX)], Some(&entry(key, &row.command, when, false, row.args.as_ref())), options),
    }
}

fn remove_user(content: &mut String, row: &Row, options: &FormattingOptions) {
    if let Some(index) = user_entry_index(content, row) {
        set(content, &[Segment::Index(index)], None, options);
    }
}

/// `removeDefaultKeybinding`: a `-command` entry for the default keybinding, unless the file has one.
fn remove_default(content: &mut String, row: &Row, options: &FormattingOptions) {
    if row.keybinding.is_empty() {
        return;
    }
    let when = (!row.when.is_empty()).then_some(row.when.as_str());
    let removal = entry(&keybindings::user_settings_label(&row.keybinding), &row.command, when, true, None);
    if entries(content).iter().all(|existing| !same_entry(existing, &removal)) {
        set(content, &[Segment::Index(usize::MAX)], Some(&removal), options);
    }
}

fn remove_unassigned_defaults(content: &mut String, row: &Row, options: &FormattingOptions) {
    let removal = format!("-{}", row.command);
    let indices: Vec<usize> = entries(content).iter().enumerate().filter(|(_, entry)| entry["command"] == removal.as_str()).map(|(index, _)| index).collect();
    for index in indices.into_iter().rev() {
        set(content, &[Segment::Index(index)], None, options);
    }
}

/// `findUserKeybindingEntryIndex`: the first entry of the command with the same `when`.
fn user_entry_index(content: &str, row: &Row) -> Option<usize> {
    entries(content).iter().position(|entry| {
        if entry["command"] != row.command.as_str() {
            return false;
        }
        match entry["when"].as_str().filter(|when| !when.is_empty()) {
            None => row.when.is_empty(),
            Some(when) => !row.when.is_empty() && Expr::deserialize(when).is_some_and(|when| when.serialize() == row.when),
        }
    })
}

/// `asObject`.
fn entry(key: &str, command: &str, when: Option<&str>, negate: bool, args: Option<&Value>) -> Value {
    let mut object = Map::new();
    object.insert("key".into(), json!(key));
    if !command.is_empty() {
        object.insert("command".into(), json!(if negate { format!("-{command}") } else { command.to_string() }));
    }
    if let Some(when) = when.filter(|when| !when.is_empty()) {
        object.insert("when".into(), json!(when));
    }
    if let Some(args) = args.filter(|args| !args.is_null()) {
        object.insert("args".into(), args.clone());
    }
    Value::Object(object)
}

/// `areSame`.
fn same_entry(a: &Value, b: &Value) -> bool {
    if a["command"] != b["command"] || a["key"] != b["key"] || a["args"] != b["args"] {
        return false;
    }
    let when = |entry: &Value| entry["when"].as_str().and_then(Expr::deserialize);
    match (when(a), when(b)) {
        (Some(a), Some(b)) => a.equals(&b),
        (a, b) => a.is_none() && b.is_none(),
    }
}
