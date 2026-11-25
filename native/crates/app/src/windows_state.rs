//! windowsStateHandler.ts: the windows of the last session in `storage.json` (`windowsState`), the ones a start
//! without paths opens again (`window.restoreWindows`), and their size and place.

use std::path::{Path, PathBuf};

use gpui::{App, Bounds, DisplayId, Pixels, Window, WindowBounds, point, px, size};
use serde_json::{Value, json};
use storage::state::FileStorage;

use crate::uri::Uri;

const WINDOWS_STATE_KEY: &str = "windowsState";

/// `WindowMode`.
pub const MAXIMIZED: u64 = 0;
pub const NORMAL: u64 = 1;
pub const FULLSCREEN: u64 = 3;

/// `IWindowState` of one window: its workspace file, folder or empty window backup folder, and `uiState`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WindowState {
    pub workspace: Option<PathBuf>,
    pub folder: Option<PathBuf>,
    pub backup_path: Option<PathBuf>,
    pub ui: Option<UiState>,
}

/// `IWindowUIState`: the mode, and the bounds in points from the top left of the display.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiState {
    pub mode: u64,
    pub display: Option<u64>,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// `restoreWindowsState`: the last active window and the opened windows.
pub fn load(user_data: &Path) -> (Option<WindowState>, Vec<WindowState>) {
    let storage = FileStorage::open(&storage_file(user_data));
    let Some(state) = storage.get_item(WINDOWS_STATE_KEY) else { return (None, Vec::new()) };
    let opened = state["openedWindows"].as_array().into_iter().flatten().map(parse).collect();
    (state.get("lastActiveWindow").filter(|window| window.is_object()).map(parse), opened)
}

/// `getWindowsStateStoreData` written as the app quits.
pub fn save(user_data: &Path, last_active: Option<&WindowState>, opened: &[WindowState]) {
    let mut storage = FileStorage::open(&storage_file(user_data));
    let mut state = json!({ "openedWindows": opened.iter().map(serialize).collect::<Vec<_>>() });
    if let Some(last_active) = last_active {
        state["lastActiveWindow"] = serialize(last_active);
    }
    storage.set_item(WINDOWS_STATE_KEY, Some(state));
}

/// `doGetPathsFromLastSession`: `none` nothing, `one` the last active window, `folders` the opened windows and the
/// last active one with a folder or workspace, `all` and `preserve` every one; each window once.
pub fn to_restore(setting: &str, last_active: Option<WindowState>, opened: Vec<WindowState>) -> Vec<WindowState> {
    if setting == "none" {
        return Vec::new();
    }
    let mut windows = match setting {
        "one" => Vec::new(),
        _ => opened,
    };
    windows.extend(last_active);
    let mut restore: Vec<WindowState> = Vec::new();
    for window in windows {
        let has_workspace = window.workspace.as_ref().is_some_and(|file| file.is_file()) || window.folder.as_ref().is_some_and(|folder| folder.is_dir());
        let empty_with_backups = window.backup_path.is_some() && setting != "folders";
        if !has_workspace && !empty_with_backups {
            continue;
        }
        let same = |other: &WindowState| (other.workspace.is_some() && other.workspace == window.workspace) || (other.folder.is_some() && other.folder == window.folder) || (other.backup_path.is_some() && other.backup_path == window.backup_path);
        if let Some(index) = restore.iter().position(same) {
            restore.remove(index);
        }
        restore.push(window);
    }
    restore
}

/// `serializeWindowState` of an open window.
pub fn ui_state(window: &Window, cx: &App) -> UiState {
    let (mode, bounds) = match window.window_bounds() {
        WindowBounds::Fullscreen(bounds) => (FULLSCREEN, bounds),
        WindowBounds::Maximized(bounds) => (MAXIMIZED, bounds),
        WindowBounds::Windowed(bounds) => (if window.is_maximized() { MAXIMIZED } else { NORMAL }, bounds),
    };
    let display = window.display(cx).map(|display| u64::from(display.id()));
    let points = |pixels: Pixels| f64::from(f32::from(pixels));
    UiState { mode, display, x: points(bounds.origin.x), y: points(bounds.origin.y), width: points(bounds.size.width), height: points(bounds.size.height) }
}

/// `validateWindowState`: the display and bounds to open a restored window with, none when its display is gone or
/// the bounds lie off it.
pub fn window_bounds(ui: UiState, cx: &App) -> Option<(DisplayId, WindowBounds)> {
    let display = cx.find_display(DisplayId::new(ui.display?))?;
    let bounds = Bounds::new(point(px(ui.x as f32), px(ui.y as f32)), size(px(ui.width as f32), px(ui.height as f32)));
    if !Bounds::new(point(px(0.), px(0.)), display.bounds().size).intersects(&bounds) {
        return None;
    }
    let window_bounds = match ui.mode {
        MAXIMIZED => WindowBounds::Maximized(bounds),
        FULLSCREEN => WindowBounds::Fullscreen(bounds),
        _ => WindowBounds::Windowed(bounds),
    };
    Some((display.id(), window_bounds))
}

fn storage_file(user_data: &Path) -> PathBuf {
    user_data.join("User/globalStorage/storage.json")
}

fn parse(window: &Value) -> WindowState {
    let path = |uri: &Value| uri.as_str().and_then(|uri| Uri::parse(uri).ok()).filter(|uri| uri.scheme == "file").map(|uri| PathBuf::from(uri.path));
    let ui = &window["uiState"];
    let number = |key: &str| ui[key].as_f64();
    let ui = match (number("x"), number("y"), number("width"), number("height")) {
        (Some(x), Some(y), Some(width), Some(height)) => Some(UiState { mode: ui["mode"].as_u64().unwrap_or(NORMAL), display: ui["display"].as_u64(), x, y, width, height }),
        _ => None,
    };
    WindowState { workspace: path(&window["workspaceIdentifier"]["configURIPath"]), folder: path(&window["folder"]), backup_path: window["backupPath"].as_str().map(PathBuf::from), ui }
}

fn serialize(window: &WindowState) -> Value {
    let uri = |path: &PathBuf| Uri::new("file", "", &path.to_string_lossy(), "", "").map_or(Value::Null, |uri| json!(uri.to_string(false)));
    let mut state = json!({});
    if let Some(workspace) = &window.workspace {
        let id = window.backup_path.as_ref().and_then(|path| path.file_name()).map(|id| id.to_string_lossy().into_owned());
        state["workspaceIdentifier"] = json!({ "id": id, "configURIPath": uri(workspace) });
    }
    if let Some(folder) = &window.folder {
        state["folder"] = uri(folder);
    }
    if let Some(backup_path) = &window.backup_path {
        state["backupPath"] = json!(backup_path.to_string_lossy());
    }
    if let Some(ui) = window.ui {
        state["uiState"] = json!({ "mode": ui.mode, "display": ui.display, "x": ui.x, "y": ui.y, "width": ui.width, "height": ui.height });
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vsrustium-windows-state-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("folder")).unwrap();
        std::fs::create_dir_all(dir.join("User/globalStorage")).unwrap();
        dir
    }

    #[test]
    fn the_state_round_trips_through_storage_json() {
        let dir = scratch("save");
        let ui = Some(UiState { mode: NORMAL, display: Some(1), x: 10., y: 20., width: 1024., height: 768. });
        let folder = WindowState { folder: Some(dir.join("folder")), backup_path: Some(dir.join("Backups/abc")), ui, ..WindowState::default() };
        let empty = WindowState { backup_path: Some(dir.join("Backups/123")), ..WindowState::default() };
        save(&dir, Some(&folder), &[empty.clone(), folder.clone()]);
        assert_eq!(load(&dir), (Some(folder), vec![empty, WindowState { folder: Some(dir.join("folder")), backup_path: Some(dir.join("Backups/abc")), ui, ..WindowState::default() }]));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn restore_follows_window_restore_windows() {
        let dir = scratch("restore");
        let folder = WindowState { folder: Some(dir.join("folder")), ..WindowState::default() };
        let gone = WindowState { folder: Some(dir.join("gone")), ..WindowState::default() };
        let empty = WindowState { backup_path: Some(dir.join("Backups/1")), ..WindowState::default() };
        let opened = vec![folder.clone(), gone, empty.clone()];
        assert_eq!(to_restore("all", Some(folder.clone()), opened.clone()), vec![empty.clone(), folder.clone()], "each window once, the last active last");
        assert_eq!(to_restore("one", Some(empty.clone()), opened.clone()), vec![empty]);
        assert_eq!(to_restore("folders", None, opened.clone()), vec![folder]);
        assert!(to_restore("none", None, opened).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
