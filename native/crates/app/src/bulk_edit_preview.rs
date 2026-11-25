//! The Refactor Preview's model: contrib/bulkEdit/browser/preview/bulkEditPreview.ts `BulkFileOperations` and the
//! checkable elements of bulkEditTree.ts, over the `edits` of a workspace edit DTO.

use std::cmp::Ordering;

use serde_json::Value;

/// `BulkFileOperationType`.
pub const TEXT_EDIT: u8 = 1;
pub const CREATE: u8 = 2;
pub const DELETE: u8 = 4;
pub const RENAME: u8 = 8;

/// `BulkFileOperation`: one resource's edits, by index into `Preview.edits`.
#[derive(Clone, Debug)]
pub struct FileOperation {
    pub key: String,
    pub uri: Value,
    pub kind: u8,
    pub edits: Vec<usize>,
    pub text_edits: Vec<usize>,
    pub new_uri: Option<Value>,
}

/// `BulkCategory`: the edits of one `metadata.label`.
#[derive(Debug)]
pub struct Category {
    pub label: String,
    pub description: Option<String>,
    pub icon: Value,
    pub needs_confirmation: bool,
    /// The category of edits without a label (`BulkCategory.keyOf` `<default>`).
    pub default: bool,
    pub files: Vec<FileOperation>,
}

#[derive(Debug)]
pub struct Preview {
    pub edits: Vec<Value>,
    pub checked: Vec<bool>,
    pub files: Vec<FileOperation>,
    pub categories: Vec<Category>,
}

/// `extUri.getComparisonKey(uri, true)`.
pub fn uri_key(uri: &Value) -> String {
    let part = |key: &str| uri[key].as_str().unwrap_or_default().to_string();
    format!("{}://{}{}", part("scheme"), part("authority"), part("path"))
}

fn is_file_edit(edit: &Value) -> bool {
    edit.get("oldResource").is_some() || edit.get("newResource").is_some()
}

impl Preview {
    /// `BulkFileOperations._init`; `exists` answers the file service for `ignoreIfExists`/`ignoreIfNotExists`.
    pub fn new(edits: Vec<Value>, exists: impl Fn(&Value) -> bool) -> Self {
        let checked: Vec<bool> = edits.iter().map(|edit| edit["metadata"]["needsConfirmation"] != true).collect();
        let mut files: Vec<FileOperation> = Vec::new();
        let mut categories: Vec<Category> = Vec::new();
        let mut new_to_old: Vec<(String, Value)> = Vec::new();
        for (index, edit) in edits.iter().enumerate() {
            let options = &edit["options"];
            let skip_existing = options["overwrite"].is_null() && options["ignoreIfExists"] == true;
            let (uri, kind) = if edit.get("textEdit").is_some_and(|text| !text.is_null()) {
                (edit["resource"].clone(), TEXT_EDIT)
            } else if is_file_edit(edit) {
                match (edit.get("oldResource"), edit.get("newResource")) {
                    (Some(old), Some(new)) => {
                        if skip_existing && exists(old) {
                            continue;
                        }
                        new_to_old.push((uri_key(new), old.clone()));
                        (old.clone(), RENAME)
                    }
                    (Some(old), None) => {
                        if options["ignoreIfNotExists"] == true && !exists(old) {
                            continue;
                        }
                        (old.clone(), DELETE)
                    }
                    (None, Some(new)) => {
                        if skip_existing && exists(new) {
                            continue;
                        }
                        (new.clone(), CREATE)
                    }
                    (None, None) => continue,
                }
            } else {
                continue;
            };
            insert(&mut files, &new_to_old, &uri, kind, index, edit);
            let metadata = &edit["metadata"];
            let label = metadata["label"].as_str().filter(|label| !label.is_empty());
            let position = categories.iter().position(|category| match label {
                Some(label) => category.label == label && !category.default,
                None => category.default,
            });
            let category = match position {
                Some(position) => &mut categories[position],
                None => {
                    categories.push(match label {
                        Some(label) => Category {
                            label: label.to_string(),
                            description: metadata["description"].as_str().map(str::to_string),
                            icon: metadata["iconPath"].clone(),
                            needs_confirmation: metadata["needsConfirmation"] == true,
                            default: false,
                            files: Vec::new(),
                        },
                        None => Category::default_category(),
                    });
                    categories.last_mut().unwrap()
                }
            };
            insert(&mut category.files, &new_to_old, &uri, kind, index, edit);
        }
        let mut preview = Preview { edits, checked, files, categories };
        // A file operation with an unchecked file edit starts unchecked whole.
        for file in preview.files.clone() {
            if file.kind == TEXT_EDIT {
                continue;
            }
            let checked = file.edits.iter().filter(|&&index| is_file_edit(&preview.edits[index])).all(|&index| preview.checked[index]);
            if !checked {
                for &index in &file.edits {
                    preview.checked[index] = false;
                }
            }
        }
        preview.categories.sort_by(|a, b| match (a.needs_confirmation, b.needs_confirmation) {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            _ => a.label.to_lowercase().cmp(&b.label.to_lowercase()).then(a.label.cmp(&b.label)),
        });
        // `BulkEditSorter`: files by uri.
        preview.files.sort_by(|a, b| a.key.cmp(&b.key));
        for category in &mut preview.categories {
            category.files.sort_by(|a, b| a.key.cmp(&b.key));
        }
        preview
    }

    pub fn has_checked_changes(&self) -> bool {
        self.checked.iter().any(|checked| *checked)
    }

    /// `getWorkspaceEdit`: the checked edits in their order.
    pub fn checked_edits(&self) -> Vec<Value> {
        self.edits.iter().zip(&self.checked).filter(|(_, checked)| **checked).map(|(edit, _)| edit.clone()).collect()
    }

    /// `CategoryElement.isChecked`.
    pub fn category_checked(&self, category: usize) -> bool {
        self.categories[category].files.iter().flat_map(|file| &file.edits).all(|&index| self.checked[index])
    }

    pub fn set_category_checked(&mut self, category: usize, value: bool) {
        let edits: Vec<usize> = self.categories[category].files.iter().flat_map(|file| file.edits.clone()).collect();
        for index in edits {
            self.checked[index] = value;
        }
    }

    /// `FileElement.isChecked`; `in_category` when grouped by type.
    pub fn file_checked(&self, file: &FileOperation, in_category: bool) -> bool {
        let mut checked = true;
        if file.kind == TEXT_EDIT {
            checked = file.text_edits.iter().any(|&index| self.checked[index]);
        }
        checked = checked && self.file_edits_checked(file);
        if in_category && file.kind == TEXT_EDIT {
            checked = checked && self.same_uri_file_edits_checked(&file.key);
        }
        checked
    }

    pub fn set_file_checked(&mut self, file: &FileOperation, in_category: bool, value: bool) {
        for &index in &file.edits {
            self.checked[index] = value;
        }
        if in_category && file.kind != TEXT_EDIT {
            let same: Vec<usize> = self.categories.iter().flat_map(|category| &category.files).filter(|other| other.key == file.key).flat_map(|other| other.edits.clone()).collect();
            for index in same {
                self.checked[index] = value;
            }
        }
    }

    /// `FileElement.isDisabled`: a text-only file under a category whose resource's file edit is unchecked elsewhere.
    pub fn file_disabled(&self, file: &FileOperation, in_category: bool) -> bool {
        in_category && file.kind == TEXT_EDIT && !self.same_uri_file_edits_checked(&file.key)
    }

    /// `TextEditElement.setChecked`: checking a text edit checks its file's file edits.
    pub fn set_text_edit_checked(&mut self, file: &FileOperation, index: usize, value: bool) {
        self.checked[index] = value;
        if value {
            for &other in &file.edits {
                if is_file_edit(&self.edits[other]) {
                    self.checked[other] = true;
                }
            }
        }
    }

    fn file_edits_checked(&self, file: &FileOperation) -> bool {
        file.edits.iter().filter(|&&index| is_file_edit(&self.edits[index])).all(|&index| self.checked[index])
    }

    fn same_uri_file_edits_checked(&self, key: &str) -> bool {
        self.categories.iter().flat_map(|category| &category.files).filter(|file| file.key == key).all(|file| self.file_edits_checked(file))
    }
}

impl Category {
    /// `BulkCategory._defaultMetadata`.
    fn default_category() -> Self {
        Category { label: "Other".into(), description: None, icon: Value::String("symbol-file".into()), needs_confirmation: false, default: true, files: Vec::new() }
    }
}

/// `BulkFileOperations._init`'s `insert`: an edit to a rename's new resource joins the rename's operation.
fn insert(files: &mut Vec<FileOperation>, new_to_old: &[(String, Value)], uri: &Value, kind: u8, index: usize, edit: &Value) {
    let mut key = uri_key(uri);
    let mut uri = uri.clone();
    let mut position = files.iter().position(|file| file.key == key);
    if position.is_none()
        && let Some((_, old)) = new_to_old.iter().find(|(new, _)| *new == key)
    {
        uri = old.clone();
        key = uri_key(&uri);
        position = files.iter().position(|file| file.key == key);
    }
    let position = position.unwrap_or_else(|| {
        files.push(FileOperation { key, uri, kind: 0, edits: Vec::new(), text_edits: Vec::new(), new_uri: None });
        files.len() - 1
    });
    let file = &mut files[position];
    file.kind |= kind;
    file.edits.push(index);
    if kind == TEXT_EDIT {
        file.text_edits.push(index);
    } else if kind == RENAME {
        file.new_uri = edit.get("newResource").cloned();
    }
}

/// bulkEditTree.ts `getChildren` of a file: the text before the edit, the replaced text and the text after, both
/// grown to token boundaries (`prefixLen` from 23 up to 50, `suffixLen` up to 50). Columns are 1-based chars;
/// `start_tokens` and `end_tokens` are the char starts of the start and end lines' tokens.
pub fn text_edit_parts(lines: &[String], start: (usize, usize), end: (usize, usize), start_tokens: &[usize], end_tokens: &[usize]) -> (String, String, String) {
    let line = |index: usize| -> Vec<char> { lines.get(index.saturating_sub(1)).map(|line| line.chars().collect()).unwrap_or_default() };
    let token_index = |tokens: &[usize], offset: usize| tokens.iter().rposition(|&start| start <= offset).unwrap_or(0);
    let start_line = line(start.0);
    let mut prefix_len = 23;
    let mut index = token_index(start_tokens, start.1.saturating_sub(1)) as isize - 1;
    while prefix_len < 50 && index >= 0 {
        prefix_len = start.1 - start_tokens[index as usize];
        index -= 1;
    }
    let from = start.1.saturating_sub(prefix_len).max(1) - 1;
    let prefix: String = start_line[from.min(start_line.len())..(start.1 - 1).min(start_line.len())].iter().collect();
    let selecting = if start.0 == end.0 {
        start_line[(start.1 - 1).min(start_line.len())..(end.1 - 1).min(start_line.len())].iter().collect()
    } else {
        let mut text: String = start_line[(start.1 - 1).min(start_line.len())..].iter().collect();
        for index in start.0 + 1..end.0 {
            text.push('\n');
            text.extend(line(index));
        }
        text.push('\n');
        text.extend(line(end.0)[..(end.1 - 1).min(line(end.0).len())].iter());
        text
    };
    let end_line = line(end.0);
    let mut suffix_len = 0;
    let mut index = token_index(end_tokens, end.1.saturating_sub(1));
    while suffix_len < 50 && index < end_tokens.len() {
        let token_end = end_tokens.get(index + 1).copied().unwrap_or(end_line.len());
        suffix_len += token_end - end_tokens[index];
        index += 1;
    }
    let after = (end.1 - 1).min(end_line.len());
    let suffix: String = end_line[after..(after + suffix_len).min(end_line.len())].iter().collect();
    (prefix, selecting, suffix)
}

/// `getFileEdits` applied to `text`: the checked text edits of one file (1-based line and char columns, start and
/// end) from the last to the first, as `BulkEditPreviewProvider` builds the preview model.
pub fn apply_text_edits(text: &str, mut edits: Vec<((usize, usize), (usize, usize), String)>) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let offset = |(line, column): (usize, usize)| -> usize {
        let line = line.clamp(1, lines.len());
        let before: usize = lines[..line - 1].iter().map(|line| line.chars().count() + 1).sum();
        before + (column.max(1) - 1).min(lines[line - 1].chars().count())
    };
    edits.sort_by_key(|(start, end, _)| (*start, *end));
    let mut chars: Vec<char> = text.chars().collect();
    for (start, end, replacement) in edits.into_iter().rev() {
        let (start, end) = (offset(start), offset(end).max(offset(start)));
        chars.splice(start..end, replacement.chars());
    }
    chars.into_iter().collect()
}

#[cfg(test)]
mod tests;
