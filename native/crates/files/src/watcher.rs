//! The disk file watcher: FSEvents through `notify` the way upstream's parcel watcher delivers it, events batched
//! for 75 ms (`FILE_CHANGES_HANDLER_DELAY`), coalesced by watcher.ts `EventCoalescer`, matched against each
//! watch's excludes, and reported with the path the watch was asked for (not its realpath, as `normalizePath`).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use notify::event::{EventKind, ModifyKind};
use notify::{RecursiveMode, Watcher as _};

/// `FileChangeType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeType {
    Updated = 0,
    Added = 1,
    Deleted = 2,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub path: PathBuf,
    pub kind: ChangeType,
}

/// `EventCoalescer.processEvent` and `coalesce`: one event per path (keyed ignoring case, as on macOS), CREATE then
/// DELETE dropped, DELETE then CREATE turned into CHANGE, and no DELETE below a deleted folder.
pub fn coalesce(events: impl IntoIterator<Item = Change>) -> Vec<Change> {
    let mut coalesced: Vec<Option<Change>> = Vec::new();
    let mut by_key: HashMap<String, usize> = HashMap::new();
    for event in events {
        let key = event.path.to_string_lossy().to_lowercase();
        let Some(&index) = by_key.get(&key) else {
            by_key.insert(key, coalesced.len());
            coalesced.push(Some(event));
            continue;
        };
        let existing = coalesced[index].as_mut().expect("a mapped event is kept");
        let renamed_case = existing.path != event.path && event.kind != ChangeType::Updated;
        match (existing.kind, event.kind) {
            _ if renamed_case => {
                by_key.insert(key, coalesced.len());
                coalesced.push(Some(event));
            }
            (ChangeType::Added, ChangeType::Deleted) => {
                coalesced[index] = None;
                by_key.remove(&key);
            }
            (ChangeType::Deleted, ChangeType::Added) => existing.kind = ChangeType::Updated,
            (ChangeType::Added, ChangeType::Updated) => {}
            (_, kind) => existing.kind = kind,
        }
    }
    let events: Vec<Change> = coalesced.into_iter().flatten().collect();
    let mut deleted: Vec<&Change> = events.iter().filter(|event| event.kind == ChangeType::Deleted).collect();
    deleted.sort_by_key(|event| event.path.as_os_str().len());
    let mut deleted_roots: Vec<&Path> = Vec::new();
    for event in deleted {
        if !deleted_roots.iter().any(|root| event.path.starts_with(root)) {
            deleted_roots.push(&event.path);
        }
    }
    let kept_deletes: HashSet<&Path> = deleted_roots.into_iter().collect();
    let mut result: Vec<Change> = events.iter().filter(|event| event.kind == ChangeType::Deleted && kept_deletes.contains(event.path.as_path())).cloned().collect();
    result.extend(events.iter().filter(|event| event.kind != ChangeType::Deleted).cloned());
    result
}

struct Watch {
    root: PathBuf,
    real_root: PathBuf,
    recursive: bool,
    excludes: GlobSet,
}

impl Watch {
    /// The event path as seen below `root`, or `None` when the watch does not cover it.
    fn covers(&self, path: &Path) -> Option<PathBuf> {
        let relative = path.strip_prefix(&self.real_root).or_else(|_| path.strip_prefix(&self.root)).ok()?;
        if !self.recursive && relative.components().count() > 1 {
            return None;
        }
        if !relative.as_os_str().is_empty() && self.excludes.is_match(relative) {
            return None;
        }
        Some(self.root.join(relative))
    }
}

/// Watches folders and files by id; every batch of changes any watch covers goes to the callback, on the watcher's thread.
pub struct Watcher {
    fs_events: notify::RecommendedWatcher,
    watches: Arc<Mutex<HashMap<u64, Watch>>>,
}

impl Watcher {
    pub fn new(on_changes: impl Fn(Vec<Change>) + Send + 'static) -> notify::Result<Watcher> {
        let (sender, receiver) = channel::<Change>();
        let watches: Arc<Mutex<HashMap<u64, Watch>>> = Arc::default();
        let fs_events = notify::recommended_watcher(move |event: notify::Result<notify::Event>| forward(event, &sender))?;
        let batch_watches = watches.clone();
        std::thread::Builder::new().name("file-watcher".into()).spawn(move || deliver_batches(&receiver, &batch_watches, on_changes))?;
        Ok(Watcher { fs_events, watches })
    }

    /// `excludes` are glob patterns relative to `path` (`files.watcherExclude`, `IWatchOptions.excludes`).
    pub fn watch(&mut self, id: u64, path: &Path, recursive: bool, excludes: &[String]) -> notify::Result<()> {
        let mut builder = GlobSetBuilder::new();
        for pattern in excludes {
            if let Ok(glob) = GlobBuilder::new(pattern).literal_separator(true).case_insensitive(true).build() {
                builder.add(glob);
            }
        }
        let excludes = builder.build().map_err(|err| notify::Error::generic(&err.to_string()))?;
        let real_root = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let mode = if recursive { RecursiveMode::Recursive } else { RecursiveMode::NonRecursive };
        self.fs_events.watch(path, mode)?;
        self.watches.lock().unwrap().insert(id, Watch { root: path.to_path_buf(), real_root, recursive, excludes });
        Ok(())
    }

    pub fn unwatch(&mut self, id: u64) {
        let Some(watch) = self.watches.lock().unwrap().remove(&id) else { return };
        if !self.watches.lock().unwrap().values().any(|other| other.root == watch.root) {
            let _ = self.fs_events.unwatch(&watch.root);
        }
    }
}

/// parcel's FSEvents handling: a create, remove or rename is an add when the path exists afterwards, else a delete.
fn forward(event: notify::Result<notify::Event>, sender: &Sender<Change>) {
    let Ok(event) = event else { return };
    for path in event.paths {
        let kind = match event.kind {
            EventKind::Create(_) | EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(_)) => match path.symlink_metadata().is_ok() {
                true => ChangeType::Added,
                false => ChangeType::Deleted,
            },
            EventKind::Modify(_) => ChangeType::Updated,
            _ => continue,
        };
        let _ = sender.send(Change { path, kind });
    }
}

fn deliver_batches(receiver: &Receiver<Change>, watches: &Mutex<HashMap<u64, Watch>>, on_changes: impl Fn(Vec<Change>)) {
    const DELAY: Duration = Duration::from_millis(75);
    while let Ok(first) = receiver.recv() {
        let mut batch = vec![first];
        let deadline = Instant::now() + DELAY;
        loop {
            match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(change) => batch.push(change),
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
        let mut covered = Vec::new();
        {
            let watches = watches.lock().unwrap();
            for change in batch {
                if let Some(path) = watches.values().find_map(|watch| watch.covers(&change.path)) {
                    covered.push(Change { path, kind: change.kind });
                }
            }
        }
        let changes = coalesce(covered);
        if !changes.is_empty() {
            on_changes(changes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ChangeType::*;

    fn change(path: &str, kind: ChangeType) -> Change {
        Change { path: PathBuf::from(path), kind }
    }

    /// Cases from src/vs/platform/files/test/common/watcher.test.ts `EventCoalescer`.
    #[test]
    fn coalesces_like_the_event_coalescer() {
        assert_eq!(
            coalesce([change("/a/b.txt", Added), change("/a/b.txt", Deleted), change("/a/c.txt", Deleted), change("/a/c.txt", Added)]),
            [change("/a/c.txt", Updated)]
        );
        assert_eq!(coalesce([change("/a/d.txt", Added), change("/a/d.txt", Updated)]), [change("/a/d.txt", Added)]);
        assert_eq!(
            coalesce([change("/a/f/g.txt", Deleted), change("/a/f", Deleted), change("/a/x.txt", Updated)]),
            [change("/a/f", Deleted), change("/a/x.txt", Updated)]
        );
        assert_eq!(coalesce([change("/a/B.txt", Deleted), change("/a/b.txt", Added)]), [change("/a/B.txt", Deleted), change("/a/b.txt", Added)]);
    }

    #[test]
    fn reports_changes_below_the_requested_path_minus_excludes() {
        let dir = std::env::temp_dir().join(format!("files-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".git/objects")).unwrap();
        let (sender, receiver) = channel();
        let mut watcher = Watcher::new(move |changes| sender.send(changes).unwrap()).unwrap();
        watcher.watch(1, &dir, true, &[".git/objects/**".to_string()]).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        std::fs::write(dir.join(".git/objects/ignored"), "").unwrap();
        std::fs::write(dir.join("seen.txt"), "").unwrap();
        let mut seen = Vec::new();
        while let Ok(changes) = receiver.recv_timeout(Duration::from_secs(3)) {
            seen.extend(changes);
            if seen.iter().any(|change| change.path == dir.join("seen.txt")) {
                break;
            }
        }
        // /var/folders is a link to /private/var/folders; the path comes back as requested.
        assert!(seen.contains(&change(dir.join("seen.txt").to_str().unwrap(), Added)), "{seen:?}");
        assert!(!seen.iter().any(|change| change.path.starts_with(dir.join(".git"))), "{seen:?}");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
