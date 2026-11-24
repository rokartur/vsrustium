//! MainThreadWorkspace search (workbench/api/browser/mainThreadWorkspace.ts `$startFileSearch`, `$startTextSearch`):
//! `workspace.findFiles` and `workspace.findTextInFiles` over ripgrep, as the local search service runs them.
//! Each search runs on its own thread so the host keeps being served and a `Cancel` can stop it. The encoding
//! requests (`$resolveDecoding`, `$validateDetectedEncoding`, `$resolveEncoding`) answer right away.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use config::configuration::Overrides;
use config::service::Settings;
use files::encoding;
use search::ripgrep::Cancellation;
use serde_json::{Value, json};

use crate::rpc::{Arg, Message, Reply};
use crate::scan::file_uri;

/// What a search thread hands back to the session thread, in the order the host must see it.
pub enum Outgoing {
    /// `ExtHostWorkspace.$handleTextSearchResult(fileMatch, requestId)`.
    TextSearchResult(Vec<Value>),
    Reply(Message),
}

pub struct MainThreadWorkspace {
    folders: Vec<PathBuf>,
    settings: Arc<Mutex<Settings>>,
    rg: PathBuf,
    /// Running searches by the request that started them.
    searches: HashMap<u32, Arc<Cancellation>>,
    sender: Sender<(u32, Outgoing)>,
    receiver: Receiver<(u32, Outgoing)>,
}

impl MainThreadWorkspace {
    pub fn new(folders: Vec<PathBuf>, settings: Arc<Mutex<Settings>>, app_root: &Path) -> Self {
        let (sender, receiver) = channel();
        MainThreadWorkspace { folders, settings, rg: search::ripgrep::rg_path(app_root), searches: HashMap::new(), sender, receiver }
    }

    /// The methods this module answers; other MainThreadWorkspace calls keep their default reply.
    pub fn serves(method: &str) -> bool {
        matches!(method, "$startFileSearch" | "$startTextSearch" | "$resolveDecoding" | "$validateDetectedEncoding" | "$resolveEncoding")
    }

    /// Starts request `req`; its reply comes later from `take_outgoing`.
    pub fn start(&mut self, req: u32, method: &str, args: &[Arg]) {
        let json = |index: usize| match args.get(index) {
            Some(Arg::Json(value)) => value.clone(),
            _ => Value::Null,
        };
        if !method.starts_with("$start") {
            let value = self.encoding(method, &json(0), &json(1), &json(2));
            let _ = self.sender.send((req, Outgoing::Reply(Message::ReplyOk { req, value: Reply::Json(value) })));
            return;
        }
        let cancellation = Arc::new(Cancellation::default());
        self.searches.insert(req, cancellation.clone());
        let sender = self.sender.clone();
        let rg = self.rg.clone();
        if method == "$startFileSearch" {
            let query = search::query::file_query(&self.search_folders(&json(0)), &json(1), &|folder, key| self.setting(folder, key));
            std::thread::spawn(move || {
                let mut resources = Vec::new();
                let result = search::file_search::search(&query, &rg, &cancellation, &mut |folder, relative_path| resources.push(file_uri(&folder.join(relative_path))));
                let _ = sender.send((req, Outgoing::Reply(reply(req, &cancellation, result.map(|_| Value::Array(resources))))));
            });
            return;
        }
        let query = search::query::text_query(&json(0), &self.search_folders(&json(1)), &json(2), &|folder, key| self.setting(folder, key));
        let request_id = json(3);
        std::thread::spawn(move || {
            let result = search::text_search::search(&query, &rg, &cancellation, &mut |path, results| {
                let file_match = json!({ "resource": file_uri(path), "results": results });
                let _ = sender.send((req, Outgoing::TextSearchResult(vec![file_match, request_id.clone()])));
            });
            let _ = sender.send((req, Outgoing::Reply(reply(req, &cancellation, result.map(|limit_hit| json!({ "limitHit": limit_hit }))))));
        });
    }

    /// A `Cancel` from the host: ripgrep is killed and the request answers `null`.
    pub fn cancel(&self, req: u32) {
        if let Some(cancellation) = self.searches.get(&req) {
            cancellation.cancel();
        }
    }

    /// Results and replies the search threads produced since the last call.
    pub fn take_outgoing(&mut self) -> Vec<Outgoing> {
        let mut outgoing = Vec::new();
        for (req, message) in self.receiver.try_iter() {
            if matches!(message, Outgoing::Reply(_)) {
                self.searches.remove(&req);
            }
            outgoing.push(message);
        }
        outgoing
    }

    /// The encodings textFileService.ts resolves for `workspace.decode` and `workspace.encode`; the extension host
    /// decodes and encodes with them.
    fn encoding(&self, method: &str, resource: &Value, second: &Value, third: &Value) -> Value {
        let path = (resource["scheme"] == "file").then(|| PathBuf::from(resource["path"].as_str().unwrap_or_default()));
        let path = path.as_deref();
        let settings = self.settings.lock().unwrap();
        let setting = |key| settings.get_value(Some(key), &Overrides { resource: path, ..Overrides::default() }).unwrap_or(Value::Null);
        match method {
            "$resolveDecoding" => json!({
                "preferredEncoding": encoding::read_encoding(&settings, path, second["encoding"].as_str(), None),
                "guessEncoding": setting("files.autoGuessEncoding"),
                "candidateGuessEncodings": setting("files.candidateGuessEncodings"),
            }),
            "$validateDetectedEncoding" => json!(encoding::read_encoding(&settings, path, third["encoding"].as_str(), second.as_str())),
            _ => {
                let (encoding, add_bom) = encoding::write_encoding(&settings, path, second["encoding"].as_str());
                json!({ "encoding": encoding, "addBOM": add_bom })
            }
        }
    }

    /// The folder the extension named, else every workspace folder.
    // shortcut: folders of other schemes are skipped, the port has no extension search providers
    // (`$registerFileSearchProvider`); add them with that ledger row.
    fn search_folders(&self, folder: &Value) -> Vec<PathBuf> {
        if folder.is_null() {
            return self.folders.clone();
        }
        match (folder["scheme"].as_str(), folder["path"].as_str()) {
            (Some("file"), Some(path)) => vec![PathBuf::from(path)],
            _ => Vec::new(),
        }
    }

    fn setting(&self, folder: &Path, key: &str) -> Value {
        self.settings.lock().unwrap().get_value(Some(key), &Overrides { resource: Some(folder), ..Overrides::default() }).unwrap_or(Value::Null)
    }
}

/// A cancelled search answers `null`, as upstream maps a cancellation error.
fn reply(req: u32, cancellation: &Cancellation, result: Result<Value, String>) -> Message {
    match result {
        _ if cancellation.is_cancelled() => Message::ReplyOk { req, value: Reply::Json(Value::Null) },
        Ok(value) => Message::ReplyOk { req, value: Reply::Json(value) },
        Err(message) => Message::ReplyErr { req, error: Some(json!({ "$isError": true, "name": "Error", "message": message, "stack": format!("Error: {message}") })) },
    }
}
