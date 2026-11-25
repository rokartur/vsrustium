//! platform/state/node/stateService.ts: `FileStorage`, the `storage.json` of the user data folder that main
//! process services keep their state in (profiles and their associations, the telemetry machine id).
//! Saves are written at once, as `SaveStrategy.IMMEDIATE`; upstream's delayed strategy only batches them.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

pub struct FileStorage {
    path: PathBuf,
    storage: Map<String, Value>,
    last_saved_contents: String,
    initialized: bool,
    closed: bool,
}

impl FileStorage {
    pub fn new(path: &Path) -> FileStorage {
        FileStorage { path: path.to_owned(), storage: Map::new(), last_saved_contents: String::new(), initialized: false, closed: false }
    }

    /// `new` then `init`: the state service as main.ts creates it.
    pub fn open(path: &Path) -> FileStorage {
        let mut storage = FileStorage::new(path);
        storage.init();
        storage
    }

    /// Reads the file. A missing or unreadable file keeps the items set so far; one that is not a JSON object too,
    /// as `JSON.parse` throwing leaves `this.storage` alone.
    pub fn init(&mut self) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        match fs::read_to_string(&self.path) {
            Ok(contents) => {
                if let Ok(Value::Object(storage)) = serde_json::from_str(&contents) {
                    self.storage = storage;
                }
                self.last_saved_contents = contents;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => eprintln!("[state] reading {}: {error}", self.path.display()),
        }
    }

    /// `getItem`: `None` for a missing or null item.
    pub fn get_item(&self, key: &str) -> Option<&Value> {
        self.storage.get(key).filter(|value| !value.is_null())
    }

    pub fn set_item(&mut self, key: &str, data: Option<Value>) {
        self.set_items(vec![(key.to_owned(), data)]);
    }

    /// `setItems`: `None` or null removes an item.
    pub fn set_items(&mut self, items: Vec<(String, Option<Value>)>) {
        let mut save = false;
        for (key, data) in items {
            match data.filter(|data| !data.is_null()) {
                None => save |= self.storage.shift_remove(&key).is_some(),
                Some(data) if self.storage.get(&key) == Some(&data) => {}
                Some(data) => {
                    self.storage.insert(key, data);
                    save = true;
                }
            }
        }
        if save {
            self.save();
        }
    }

    pub fn remove_item(&mut self, key: &str) {
        if self.storage.shift_remove(key).is_some() {
            self.save();
        }
    }

    /// `close`: later changes stay in memory only.
    pub fn close(&mut self) {
        self.save();
        self.closed = true;
    }

    fn save(&mut self) {
        if self.closed || !self.initialized {
            return;
        }
        let serialized = to_json_4(&self.storage);
        if serialized == self.last_saved_contents {
            return;
        }
        // `writeFile` with `atomic: { postfix: '.vsctmp' }`: a temporary file renamed over the target.
        let temporary = PathBuf::from(format!("{}.vsctmp", self.path.display()));
        let written = fs::write(&temporary, &serialized).and_then(|()| fs::rename(&temporary, &self.path));
        match written {
            Ok(()) => self.last_saved_contents = serialized,
            Err(error) => eprintln!("[state] writing {}: {error}", self.path.display()),
        }
    }
}

/// `JSON.stringify(value, null, 4)`.
fn to_json_4(value: &Map<String, Value>) -> String {
    let mut out = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"    ");
    let mut serializer = serde_json::Serializer::with_formatter(&mut out, formatter);
    serde::Serialize::serialize(value, &mut serializer).expect("a JSON map serializes");
    String::from_utf8(out).expect("serde_json writes UTF-8")
}

#[cfg(test)]
mod tests;
