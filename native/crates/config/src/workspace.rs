//! The workspace a window opens (platform/workspace/common/workspace.ts and platform/workspaces/common/workspaces.ts):
//! one folder, or the folders a `.code-workspace` file lists.

use std::path::{Component, Path, PathBuf};

use serde_json::Value;

use crate::json::{self, ParseOptions};

/// `WORKSPACE_EXTENSION`.
pub const WORKSPACE_EXTENSION: &str = "code-workspace";
/// `UNTITLED_WORKSPACE_NAME`: the file of an untitled workspace, in its own folder under `<user data>/Workspaces`.
pub const UNTITLED_WORKSPACE_NAME: &str = "workspace.json";

#[derive(Clone, Debug, PartialEq)]
pub struct Workspace {
    /// The `.code-workspace` file; `None` for a single-folder window.
    pub file: Option<PathBuf>,
    pub folders: Vec<WorkspaceFolder>,
    /// The workspace file's `transient` key: the window does not reopen it.
    pub transient: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WorkspaceFolder {
    pub path: PathBuf,
    pub name: String,
}

impl Workspace {
    /// A folder, or a workspace file when the path has the `.code-workspace` extension (`hasWorkspaceFileExtension`)
    /// or is an untitled workspace's file.
    pub fn open(path: PathBuf) -> Workspace {
        match path.extension().is_some_and(|extension| extension == WORKSPACE_EXTENSION) || is_untitled_workspace(&path) {
            true => Workspace::from_file(path),
            false => Workspace::folder(path),
        }
    }

    pub fn folder(path: PathBuf) -> Workspace {
        let name = path.file_name().map_or_else(|| path.to_string_lossy(), |name| name.to_string_lossy()).into_owned();
        Workspace { file: None, folders: vec![WorkspaceFolder { path, name }], transient: false }
    }

    /// A workspace file that cannot be read or parsed has no folders, as `WorkspaceConfigurationModelParser` leaves them.
    pub fn from_file(file: PathBuf) -> Workspace {
        let content = std::fs::read_to_string(&file).unwrap_or_default();
        let raw = json::parse(&content, &mut Vec::new(), ParseOptions { allow_trailing_comma: true, allow_empty_content: true, ..ParseOptions::default() });
        let raw = raw.unwrap_or_default();
        let configured = raw["folders"].as_array().cloned().unwrap_or_default();
        let folders = to_workspace_folders(&configured, &file);
        Workspace { transient: raw["transient"] == true, file: Some(file), folders }
    }

    /// `Workspace.getFolder`: the folder closest to `resource`, compared ignoring case as macOS paths are.
    pub fn folder_of(&self, resource: &Path) -> Option<&WorkspaceFolder> {
        let resource = comparison_key(resource);
        self.folders
            .iter()
            .filter(|folder| resource.starts_with(&comparison_key(&folder.path)))
            .max_by_key(|folder| folder.path.components().count())
    }

    /// `ILabelService.getWorkspaceLabel` (medium verbosity): the folder name, or the file name without the
    /// extension and " (Workspace)".
    pub fn label(&self) -> String {
        match &self.file {
            Some(file) if is_untitled_workspace(file) => "Untitled (Workspace)".to_owned(),
            Some(file) => {
                let name = file.file_name().unwrap_or_default().to_string_lossy();
                let name = name.strip_suffix(&format!(".{WORKSPACE_EXTENSION}")).unwrap_or(&name);
                format!("{name} (Workspace)")
            }
            None => self.folders.first().map(|folder| folder.name.clone()).unwrap_or_default(),
        }
    }
}

/// `isUntitledWorkspace`: `<user data>/Workspaces/<id>/workspace.json`. Read from the path's shape, as the
/// config crate does not know the user data folder.
pub fn is_untitled_workspace(file: &Path) -> bool {
    let home = file.parent().and_then(Path::parent);
    file.file_name().is_some_and(|name| name == UNTITLED_WORKSPACE_NAME) && home.and_then(Path::file_name).is_some_and(|name| name == "Workspaces")
}

/// `toWorkspaceFolders`: `path` entries resolved against the workspace file's folder, `file:` `uri` entries,
/// duplicates dropped.
pub fn to_workspace_folders(configured: &[Value], workspace_file: &Path) -> Vec<WorkspaceFolder> {
    stored_folders(configured, workspace_file).into_iter().map(|(folder, _)| folder).collect()
}

/// `toWorkspaceFolders` keeping each folder's entry (`IWorkspaceFolder.raw`).
fn stored_folders(configured: &[Value], workspace_file: &Path) -> Vec<(WorkspaceFolder, Value)> {
    let relative_to = workspace_file.parent().unwrap_or(Path::new("/"));
    let mut folders: Vec<(WorkspaceFolder, Value)> = Vec::new();
    for configured in configured {
        let name = configured["name"].as_str().filter(|name| !name.is_empty());
        let path = match (configured["path"].as_str(), configured["uri"].as_str()) {
            (Some(""), _) => None,
            (Some(path), _) => Some(resolve(relative_to, path)),
            // shortcut: only `file:` URIs; a folder of another scheme needs the extension file system providers in the tree.
            (None, Some(uri)) => uri.strip_prefix("file://").map(|rest| PathBuf::from(percent_decode(&rest[rest.find('/').unwrap_or(rest.len())..]))),
            (None, None) => None,
        };
        let Some(path) = path else { continue };
        if folders.iter().any(|(folder, _)| comparison_key(&folder.path) == comparison_key(&path)) {
            continue;
        }
        let name = name.map(str::to_owned).unwrap_or_else(|| path.file_name().map_or_else(|| path.to_string_lossy(), |name| name.to_string_lossy()).into_owned());
        folders.push((WorkspaceFolder { path, name }, configured.clone()));
    }
    folders
}

/// configurationService.ts `doUpdateFolders` on the workspace file's `folders`: the entries of the folders left
/// after `to_remove`, then `to_add` (folder and name) at `index` or the end, skipping folders already there and
/// paths that are not folders. `None` when nothing changed.
pub fn updated_folders(configured: &[Value], workspace_file: &Path, to_add: &[(PathBuf, Option<String>)], to_remove: &[PathBuf], index: Option<usize>) -> Option<Vec<Value>> {
    let current = stored_folders(configured, workspace_file);
    let removed = |path: &Path| to_remove.iter().any(|remove| comparison_key(remove) == comparison_key(path));
    let mut entries: Vec<Value> = current.iter().filter(|(folder, _)| !removed(&folder.path)).map(|(_, entry)| entry.clone()).collect();
    let mut changed = entries.len() != current.len();
    let config_folder = workspace_file.parent().unwrap_or(Path::new("/"));
    let kept = to_workspace_folders(&entries, workspace_file);
    let mut added = Vec::new();
    for (path, name) in to_add {
        // A path that cannot be read is added, as the `stat` error is ignored.
        if kept.iter().any(|folder| comparison_key(&folder.path) == comparison_key(path)) || path.metadata().is_ok_and(|metadata| !metadata.is_dir()) {
            continue;
        }
        added.push(stored_folder(path, false, name.as_deref(), config_folder));
    }
    if !added.is_empty() {
        changed = true;
        let at = index.filter(|index| *index < entries.len()).unwrap_or(entries.len());
        entries.splice(at..at, added);
    }
    changed.then_some(entries)
}

/// `getStoredWorkspaceFolder` for a `file` folder: a path relative to the workspace file's folder unless
/// `force_absolute`, and the name when given.
pub fn stored_folder(folder: &Path, force_absolute: bool, name: Option<&str>, config_folder: &Path) -> Value {
    let path = match force_absolute {
        true => folder.to_string_lossy().into_owned(),
        false => match relative_path(config_folder, folder) {
            relative if relative.is_empty() => ".".to_owned(),
            relative => relative,
        },
    };
    let mut entry = serde_json::Map::new();
    if let Some(name) = name {
        entry.insert("name".into(), Value::String(name.to_owned()));
    }
    entry.insert("path".into(), Value::String(path));
    Value::Object(entry)
}

/// `rewriteWorkspaceFileForNewLocation`: the workspace file's text with its folders stored for `target`, relative
/// where possible when it was untitled, else absolute as they were; the rest of the text kept.
pub fn rewrite_for_new_location(content: &str, source: &Path, target: &Path) -> String {
    let raw = json::parse(content, &mut Vec::new(), ParseOptions { allow_trailing_comma: true, allow_empty_content: true, ..ParseOptions::default() }).unwrap_or_default();
    let configured: Vec<Value> = raw["folders"].as_array().cloned().unwrap_or_default();
    let from_untitled = is_untitled_workspace(source);
    let target_folder = target.parent().unwrap_or(Path::new("/"));
    let mut rewritten = Vec::new();
    for entry in &configured {
        let Some((folder, _)) = stored_folders(std::slice::from_ref(entry), source).into_iter().next() else { continue };
        let absolute = !from_untitled && entry["path"].as_str().is_none_or(|path| path.starts_with('/'));
        rewritten.push(stored_folder(&folder.path, absolute, entry["name"].as_str(), target_folder));
    }
    let options = crate::json_edit::FormattingOptions { insert_spaces: false, tab_size: 4, eol: "\n".into() };
    let edits = crate::json_edit::set_property(content, &[json::Segment::Key("folders".into())], Some(&Value::Array(rewritten)), &options);
    crate::json_edit::apply_edits(content, &edits)
}

/// Node's `path.relative(from, to)` for absolute paths.
fn relative_path(from: &Path, to: &Path) -> String {
    let from: Vec<Component> = from.components().collect();
    let to: Vec<Component> = to.components().collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<String> = vec!["..".to_owned(); from.len() - common];
    parts.extend(to[common..].iter().map(|component| component.as_os_str().to_string_lossy().into_owned()));
    parts.join("/")
}

/// Node's `path.resolve(base, path)`: lexical, `..` and `.` removed, symbolic links kept.
fn resolve(base: &Path, path: &str) -> PathBuf {
    let mut resolved = PathBuf::new();
    for component in base.join(path).components() {
        match component {
            Component::ParentDir => _ = resolved.pop(),
            Component::CurDir => {}
            other => resolved.push(other),
        }
    }
    resolved
}

/// `extUriBiasedIgnorePathCase.getComparisonKey` on macOS: the lowercase path with a trailing separator, so
/// `starts_with` checks whole segments.
fn comparison_key(path: &Path) -> String {
    let mut key = path.to_string_lossy().to_lowercase();
    if !key.ends_with('/') {
        key.push('/');
    }
    key
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let hex = bytes.get(index + 1..index + 3).and_then(|hex| std::str::from_utf8(hex).ok()).and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match (bytes[index], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                index += 3;
            }
            (byte, _) => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The `toWorkspaceFolders` and `getFolder` cases of platform/workspace/test/common/workspace.test.ts.
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn folders(configured: Value) -> Vec<(String, String)> {
        let configured = configured.as_array().unwrap().clone();
        to_workspace_folders(&configured, Path::new("/src/test.code-workspace")).into_iter().map(|folder| (folder.path.to_string_lossy().into_owned(), folder.name)).collect()
    }

    fn pairs(expected: &[(&str, &str)]) -> Vec<(String, String)> {
        expected.iter().map(|(path, name)| (path.to_string(), name.to_string())).collect()
    }

    #[test]
    fn resolves_folders_like_to_workspace_folders() {
        assert_eq!(folders(json!([{ "path": "/src/test" }])), pairs(&[("/src/test", "test")]));
        assert_eq!(folders(json!([{ "path": "./test" }])), pairs(&[("/src/test", "test")]));
        assert_eq!(folders(json!([{ "path": "/src/test", "name": "hello" }])), pairs(&[("/src/test", "hello")]));
        assert_eq!(
            folders(json!([{ "path": "/src/test2" }, { "path": "/src/test3" }, { "path": "/src/test1" }])),
            pairs(&[("/src/test2", "test2"), ("/src/test3", "test3"), ("/src/test1", "test1")])
        );
        assert_eq!(
            folders(json!([{ "path": "/src/test2" }, { "path": "/src/test3", "name": "noName" }, { "path": "/src/test1" }])),
            pairs(&[("/src/test2", "test2"), ("/src/test3", "noName"), ("/src/test1", "test1")])
        );
        assert_eq!(
            folders(json!([{ "path": "/src/test2" }, { "path": "/abc/test3", "name": "noName" }, { "path": "./test1" }])),
            pairs(&[("/src/test2", "test2"), ("/abc/test3", "noName"), ("/src/test1", "test1")])
        );
        assert_eq!(folders(json!([{ "path": "/src/test2" }, { "path": "/src/test2", "name": "noName" }, { "path": "/src/test1" }])), pairs(&[("/src/test2", "test2"), ("/src/test1", "test1")]));
        assert_eq!(
            folders(json!([{ "path": "/src/test2" }, { "path": "/src/test3", "name": "noName" }, { "path": "./test3" }, { "path": "/abc/test1" }])),
            pairs(&[("/src/test2", "test2"), ("/src/test3", "noName"), ("/abc/test1", "test1")])
        );
        assert_eq!(
            folders(json!([{ "path": "/src/test2" }, { "path": "", "name": "noName" }, { "path": "./test3" }, { "path": "/abc/test1" }])),
            pairs(&[("/src/test2", "test2"), ("/src/test3", "test3"), ("/abc/test1", "test1")])
        );
        assert_eq!(folders(json!([{ "path": "../other/./x" }, { "uri": "file:///a%20b/c" }])), pairs(&[("/other/x", "x"), ("/a b/c", "c")]));
    }

    #[test]
    fn get_folder_returns_the_closest_folder() {
        let folder = |path: &str| WorkspaceFolder { path: PathBuf::from(path), name: String::new() };
        let workspace = Workspace { file: None, folders: vec![folder("/src/main"), folder("/src/code"), folder("/src/test")], transient: false };
        assert_eq!(workspace.folder_of(Path::new("/src/test")).unwrap().path, Path::new("/src/test"));
        assert_eq!(workspace.folder_of(Path::new("/src/test/a")).unwrap().path, Path::new("/src/test"));
        assert_eq!(workspace.folder_of(Path::new("/src/main/a")).unwrap().path, Path::new("/src/main"));
        assert!(workspace.folder_of(Path::new("/src/mainly/a")).is_none());
        let nested = Workspace { file: None, folders: vec![folder("/src"), folder("/src/test")], transient: false };
        assert_eq!(nested.folder_of(Path::new("/src/test/a")).unwrap().path, Path::new("/src/test"));
    }

    #[test]
    fn labels_a_workspace_file_by_its_name() {
        assert_eq!(Workspace::from_file(PathBuf::from("/nonexistent/testworkspace.code-workspace")).label(), "testworkspace (Workspace)");
        assert_eq!(Workspace::folder(PathBuf::from("/src/test")).label(), "test");
        assert_eq!(Workspace::open(PathBuf::from("/data/Workspaces/1700000000000/workspace.json")).label(), "Untitled (Workspace)");
    }

    #[test]
    fn updates_folders_like_do_update_folders() {
        let file = Path::new("/nonexistent/src/test.code-workspace");
        let configured = json!([{ "path": "a" }, { "path": "/nonexistent/b", "name": "B" }, { "path": "a" }]);
        let configured = configured.as_array().unwrap();
        let add = |path: &str, name: Option<&str>| (PathBuf::from(path), name.map(str::to_owned));
        assert_eq!(updated_folders(configured, file, &[], &[PathBuf::from("/nonexistent/B")], None), Some(vec![json!({ "path": "a" })]), "removal compares ignoring case and drops duplicates");
        assert_eq!(
            updated_folders(configured, file, &[add("/nonexistent/c", Some("C")), add("/nonexistent/src/a", None)], &[], Some(1)),
            Some(vec![json!({ "path": "a" }), json!({ "name": "C", "path": "../c" }), json!({ "path": "/nonexistent/b", "name": "B" })])
        );
        assert_eq!(updated_folders(configured, file, &[add("/nonexistent/src", None)], &[], Some(9)).unwrap().last(), Some(&json!({ "path": "." })));
        assert_eq!(updated_folders(&configured[..2], file, &[add("/nonexistent/src/a", None)], &[], None), None, "a folder already there is not added");
    }

    #[test]
    fn rewrites_an_untitled_workspace_for_its_new_location() {
        let untitled = Path::new("/data/Workspaces/1/workspace.json");
        let content = "{\n\t\"folders\": [\n\t\t{\n\t\t\t\"path\": \"/src/a\"\n\t\t}\n\t],\n\t\"settings\": {}\n}";
        let rewritten = rewrite_for_new_location(content, untitled, Path::new("/src/w.code-workspace"));
        assert_eq!(rewritten, "{\n\t\"folders\": [\n\t\t{\n\t\t\t\"path\": \"a\"\n\t\t}\n\t],\n\t\"settings\": {}\n}");
        let saved = rewrite_for_new_location(content, Path::new("/src/w.code-workspace"), Path::new("/other/w.code-workspace"));
        assert!(saved.contains("\"path\": \"/src/a\""), "an absolute path of a saved workspace stays absolute: {saved}");
    }
}
