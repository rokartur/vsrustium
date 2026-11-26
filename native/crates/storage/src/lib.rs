//! base/parts/storage/node/storage.ts: `SQLiteStorageDatabase` with the `Storage` cache in front of it, the
//! `state.vscdb` files of the global and workspace storage scopes. Writes go to disk on every change, where
//! upstream batches them for 100 ms; the cache is the source of truth and rebuilds a database that broke at runtime.

use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::types::{ToSql, ValueRef};
use rusqlite::{Connection, params};

pub mod profiles;
pub mod state;

pub struct Storage {
    items: BTreeMap<String, String>,
    /// `None` once dropped.
    connection: Option<Connection>,
    /// The database file; `None` for a storage that lives in memory by request.
    path: Option<PathBuf>,
    /// The file could not be opened or created and the data lives in memory until close.
    in_memory_fallback: bool,
    /// An SQLite call failed, so the file is rebuilt from `items` on close.
    erroneous: bool,
}

impl Storage {
    /// Opens or creates the database at `path`. A corrupt file is replaced by its `.backup`, else by an empty
    /// database; a path that cannot be written falls back to memory.
    pub fn open(path: &Path) -> Storage {
        let (connection, in_memory_fallback) = connect(path);
        let mut storage = Storage { items: BTreeMap::new(), connection: Some(connection), path: Some(path.to_owned()), in_memory_fallback, erroneous: false };
        storage.items = storage.stored_items();
        storage
    }

    pub fn in_memory() -> Storage {
        let connection = do_connect(Path::new(":memory:")).expect("SQLite opens an in-memory database");
        Storage { items: BTreeMap::new(), connection: Some(connection), path: None, in_memory_fallback: false, erroneous: false }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.items.get(key).map(String::as_str)
    }

    pub fn items(&self) -> impl Iterator<Item = (&str, &String)> {
        self.items.iter().map(|(key, value)| (key.as_str(), value))
    }

    pub fn set(&mut self, key: &str, value: String) {
        if self.items.get(key) == Some(&value) {
            return;
        }
        self.items.insert(key.to_owned(), value);
        let value = &self.items[key];
        let sql = "INSERT INTO ItemTable VALUES (?1, ?2) ON CONFLICT (key) DO UPDATE SET value = excluded.value WHERE value != excluded.value";
        let result = self.connection().execute(sql, params![key, value]);
        self.check(result, "updateItems()");
    }

    pub fn delete(&mut self, key: &str) {
        if self.items.remove(key).is_none() {
            return;
        }
        let result = self.connection().execute("DELETE FROM ItemTable WHERE key = ?1", params![key]);
        self.check(result, "updateItems()");
    }

    /// Rereads the cache from the file, which another process may have written.
    pub fn refresh(&mut self) {
        self.items = self.stored_items();
    }

    /// `checkIntegrity(full)`: SQLite's verdict, `ok` for a healthy file.
    pub fn check_integrity(&mut self, full: bool) -> String {
        let sql = if full { "PRAGMA integrity_check" } else { "PRAGMA quick_check" };
        let result = self.connection().query_row(sql, [], |row| row.get::<_, String>(0));
        self.check(result, "checkIntegrity()").unwrap_or_else(|| "error".into())
    }

    /// `getItems()` read from the database, not the cache.
    fn stored_items(&mut self) -> BTreeMap<String, String> {
        let result = read_items(self.connection());
        self.check(result, "getItems()").unwrap_or_default()
    }

    fn connection(&self) -> &Connection {
        self.connection.as_ref().expect("the connection lives until drop")
    }

    /// `handleSQLiteError`: logs the failure and marks the file for a rebuild on close.
    fn check<T>(&mut self, result: rusqlite::Result<T>, operation: &str) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(error) => {
                eprintln!("[storage {}] {operation}: {error}", self.name());
                self.erroneous = true;
                None
            }
        }
    }

    fn name(&self) -> String {
        self.path.as_deref().map_or(":memory:".into(), |path| path.display().to_string())
    }
}

/// `close(recovery)`: a healthy file is copied to `.backup`; a broken one, or data that only lived in memory,
/// is written to a fresh file from the cache.
impl Drop for Storage {
    fn drop(&mut self) {
        let connection = self.connection.take().expect("dropped once");
        let closed = connection.close();
        let Some(path) = self.path.clone() else { return };
        if let Err((_, error)) = closed {
            eprintln!("[storage {}] close(): {error}", path.display());
            self.erroneous = true;
        }
        let result = match self.erroneous || self.in_memory_fallback {
            false => fs::copy(&path, backup_path(&path)).map(drop).map_err(Box::from),
            true => recover(&path, &self.items),
        };
        if let Err(error) = result {
            eprintln!("[storage {}] close(): {error}", path.display());
        }
    }
}

fn connect(path: &Path) -> (Connection, bool) {
    let error = match do_connect(path) {
        Ok(connection) => return (connection, false),
        Err(error) => error,
    };
    eprintln!("[storage {}] open(): {error}", path.display());
    let from_backup = fs::remove_file(path).map_err(Box::<dyn Error>::from).and_then(|()| {
        let _ = fs::rename(backup_path(path), path);
        do_connect(path).map_err(Box::from)
    });
    match from_backup {
        Ok(connection) => (connection, false),
        Err(error) => {
            eprintln!("[storage {}] open(): Unable to use backup due to {error}", path.display());
            (do_connect(Path::new(":memory:")).expect("SQLite opens an in-memory database"), true)
        }
    }
}

/// Opens the file and runs the statements that create the table and prove the file is a database.
fn do_connect(path: &Path) -> rusqlite::Result<Connection> {
    let connection = Connection::open(path)?;
    connection.execute_batch("PRAGMA user_version = 1;CREATE TABLE IF NOT EXISTS ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB);")?;
    Ok(connection)
}

fn read_items(connection: &Connection) -> rusqlite::Result<BTreeMap<String, String>> {
    let mut statement = connection.prepare("SELECT * FROM ItemTable")?;
    let rows = statement.query_map([], |row| Ok((text(row.get_ref(0)?), text(row.get_ref(1)?))))?;
    rows.collect()
}

/// Upstream writes strings, which SQLite stores as TEXT; a BLOB written by another tool is read as UTF-8.
fn text(value: ValueRef) -> String {
    match value {
        ValueRef::Text(bytes) | ValueRef::Blob(bytes) => String::from_utf8_lossy(bytes).into_owned(),
        ValueRef::Integer(number) => number.to_string(),
        ValueRef::Real(number) => number.to_string(),
        ValueRef::Null => String::new(),
    }
}

/// Recreates the file from `items` and backs it up, as upstream's recovery connection does on its own close.
fn recover(path: &Path, items: &BTreeMap<String, String>) -> Result<(), Box<dyn Error>> {
    fs::remove_file(path)?;
    let mut connection = do_connect(path)?;
    let transaction = connection.transaction()?;
    for (key, value) in items {
        let row: [&dyn ToSql; 2] = [key, value];
        transaction.execute("INSERT INTO ItemTable VALUES (?1, ?2)", row.as_slice())?;
    }
    transaction.commit()?;
    connection.close().map_err(|(_, error)| error)?;
    fs::copy(path, backup_path(path))?;
    Ok(())
}

fn backup_path(path: &Path) -> PathBuf {
    let mut backup = path.as_os_str().to_owned();
    backup.push(".backup");
    PathBuf::from(backup)
}

/// The SQLiteStorageDatabase and Storage cases of base/parts/storage/test/node/storage.integrationTest.ts.
/// Left out: event, flush-timing and concurrency cases (writes here are synchronous) and `optimize`.
#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("code-gpui-storage-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample_items() -> Vec<(String, String)> {
        vec![("foo".into(), "bar".into()), ("some/foo/path".into(), "some/bar/path".into()), (r#"{"foo":"bar"}"#.into(), r#"{"bar":"foo"}"#.into())]
    }

    fn insert(storage: &mut Storage, items: &[(String, String)]) {
        for (key, value) in items {
            storage.set(key, value.clone());
        }
    }

    /// `testDBBasics`, checked against the file rather than the cache.
    fn db_basics(path: &Path) -> bool {
        let mut storage = Storage::open(path);
        let opened_erroneous = storage.erroneous || storage.in_memory_fallback;
        assert_eq!(storage.stored_items().len(), 0);
        let items = sample_items();
        insert(&mut storage, &items);
        let stored = storage.stored_items();
        assert_eq!(stored.len(), 3);
        assert_eq!(stored["foo"], "bar");
        assert_eq!(stored["some/foo/path"], "some/bar/path");
        assert_eq!(stored[r#"{"foo":"bar"}"#], r#"{"bar":"foo"}"#);

        storage.delete("foo");
        let stored = storage.stored_items();
        assert_eq!(stored.len(), 2);
        assert!(!stored.contains_key("foo"));

        insert(&mut storage, &items);
        storage.set("foo", "otherbar".into());
        assert_eq!(storage.stored_items()["foo"], "otherbar");

        for key in ["foo", "bar", "some/foo/path", r#"{"foo":"bar"}"#] {
            storage.delete(key);
        }
        assert_eq!(storage.stored_items().len(), 0);
        assert!(!storage.erroneous);
        opened_erroneous
    }

    #[test]
    fn basics() {
        let dir = test_dir("basics");
        assert!(!db_basics(&dir.join("storage.db")));
        assert!(!db_basics(&dir.join("storage.db")), "open multiple times");
    }

    #[test]
    fn corrupt_db_falls_back_to_empty_db() {
        let dir = test_dir("corrupt");
        let path = dir.join("broken.db");
        fs::write(&path, "This is a broken DB").unwrap();
        let mut storage = Storage::open(&path);
        assert!(!storage.in_memory_fallback);
        assert_eq!(storage.stored_items().len(), 0);
        drop(storage);
        db_basics(&path);
    }

    #[test]
    fn corrupt_db_restores_from_previous_backup() {
        let dir = test_dir("backup");
        let path = dir.join("storage.db");
        insert(&mut Storage::open(&path), &sample_items());
        fs::write(&path, "This is now a broken DB").unwrap();
        let mut storage = Storage::open(&path);
        let stored = storage.stored_items();
        assert_eq!(stored.len(), 3);
        assert_eq!(stored["some/foo/path"], "some/bar/path");
        assert!(!storage.erroneous);
    }

    #[test]
    fn corrupt_db_falls_back_to_empty_db_if_backup_is_corrupt() {
        let dir = test_dir("backup-corrupt");
        let path = dir.join("storage.db");
        insert(&mut Storage::open(&path), &sample_items());
        fs::write(&path, "This is now a broken DB").unwrap();
        fs::write(backup_path(&path), "This is now also a broken DB").unwrap();
        assert_eq!(Storage::open(&path).stored_items().len(), 0);
        db_basics(&path);
    }

    #[test]
    fn db_that_becomes_corrupt_during_runtime_stores_all_state_from_cache_on_close() {
        let dir = test_dir("runtime-corrupt");
        let path = dir.join("storage.db");
        insert(&mut Storage::open(&path), &sample_items());
        assert!(backup_path(&path).exists());

        let mut storage = Storage::open(&path);
        fs::write(&path, "This is now a broken DB").unwrap();
        assert_ne!(storage.check_integrity(true), "ok");
        assert!(storage.erroneous);
        fs::remove_file(backup_path(&path)).unwrap();
        drop(storage);
        assert!(backup_path(&path).exists());

        let mut storage = Storage::open(&path);
        let stored = storage.stored_items();
        assert_eq!(stored.len(), 3);
        assert_eq!(stored["foo"], "bar");
        assert_eq!(stored[r#"{"foo":"bar"}"#], r#"{"bar":"foo"}"#);
        assert!(!storage.erroneous);
    }

    #[test]
    fn values_survive_reopen_and_pass_integrity_checks() {
        let dir = test_dir("reopen");
        let path = dir.join("storage.db");
        let mut storage = Storage::open(&path);
        storage.set("workbench.panel.height", "419".into());
        storage.set(&"very.long.key.".repeat(40), "is long".into());
        assert_eq!(storage.check_integrity(true), "ok");
        assert_eq!(storage.check_integrity(false), "ok");
        drop(storage);
        let storage = Storage::open(&path);
        assert_eq!(storage.get("workbench.panel.height"), Some("419"));
        assert_eq!(storage.get(&"very.long.key.".repeat(40)), Some("is long"));
    }

    #[test]
    fn very_large_item_value() {
        let dir = test_dir("large");
        let mut storage = Storage::open(&dir.join("storage.db"));
        let large: String = (0..3_600_000u32).map(|index| char::from(b'a' + (index % 26) as u8)).collect();
        storage.set("super.large.string", large.clone());
        storage.set("colorthemedata", "{}".into());
        assert_eq!(storage.stored_items()["super.large.string"], large);
        storage.delete("super.large.string");
        let stored = storage.stored_items();
        assert!(!stored.contains_key("super.large.string"));
        assert_eq!(stored["colorthemedata"], "{}");
    }

    #[test]
    fn lots_of_insert_and_delete() {
        let dir = test_dir("many");
        let mut storage = Storage::open(&dir.join("storage.db"));
        for index in 0..400 {
            storage.set(&format!("key{index}"), format!("value{index}"));
        }
        assert_eq!(storage.stored_items().len(), 400);
        for index in 0..400 {
            storage.delete(&format!("key{index}"));
        }
        assert_eq!(storage.stored_items().len(), 0);
    }

    #[test]
    fn invalid_path_falls_back_to_memory() {
        let dir = test_dir("invalid");
        let mut storage = Storage::open(&dir.join("nonexist/storage.db"));
        assert!(storage.in_memory_fallback);
        storage.set("foo", "bar".into());
        assert_eq!(storage.get("foo"), Some("bar"));
    }
}
