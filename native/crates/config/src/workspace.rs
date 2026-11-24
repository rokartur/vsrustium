//! The workspace a window opens (platform/workspace/common/workspace.ts and platform/workspaces/common/workspaces.ts):
//! one folder, or the folders a `.code-workspace` file lists.

use std::path::{Component, Path, PathBuf};

use serde_json::Value;

use crate::json::{self, ParseOptions};

/// `WORKSPACE_EXTENSION`.
pub const WORKSPACE_EXTENSION: &str = "code-workspace";

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
    /// A folder, or a workspace file when the path has the `.code-workspace` extension (`hasWorkspaceFileExtension`).
    pub fn open(path: PathBuf) -> Workspace {
        match path.extension().is_some_and(|extension| extension == WORKSPACE_EXTENSION) {
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
            Some(file) => {
                let name = file.file_name().unwrap_or_default().to_string_lossy();
                let name = name.strip_suffix(&format!(".{WORKSPACE_EXTENSION}")).unwrap_or(&name);
                format!("{name} (Workspace)")
            }
            None => self.folders.first().map(|folder| folder.name.clone()).unwrap_or_default(),
        }
    }
}

/// `toWorkspaceFolders`: `path` entries resolved against the workspace file's folder, `file:` `uri` entries,
/// duplicates dropped.
pub fn to_workspace_folders(configured: &[Value], workspace_file: &Path) -> Vec<WorkspaceFolder> {
    let relative_to = workspace_file.parent().unwrap_or(Path::new("/"));
    let mut folders: Vec<WorkspaceFolder> = Vec::new();
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
        if folders.iter().any(|folder| comparison_key(&folder.path) == comparison_key(&path)) {
            continue;
        }
        let name = name.map(str::to_owned).unwrap_or_else(|| path.file_name().map_or_else(|| path.to_string_lossy(), |name| name.to_string_lossy()).into_owned());
        folders.push(WorkspaceFolder { path, name });
    }
    folders
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
    }
}
