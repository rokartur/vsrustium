//! The main-thread side of `@vscode/internal-github` (workbench/api/browser/mainThreadGitHub.ts) without sign-in:
//! anonymous clients read public gists through api.github.com (githubTransport.ts headers); account clients need a
//! `github` authentication session, which the port never has, so they fail as an unavailable session does.

use std::collections::HashSet;

use exthost::request::{Request, request};
use futures::channel::oneshot;
use gpui::Context;
use serde_json::{Value, json};

use crate::commands::Outcome;
use crate::workbench::Workbench;

/// githubTransport.ts `defaultApiVersion`.
const API_VERSION: &str = "2022-11-28";

#[derive(Default)]
pub struct GitHubClients {
    anonymous: HashSet<u64>,
}

impl Workbench {
    pub fn handle_github_request(&mut self, method: &str, args: &[Value], cx: &mut Context<Self>) -> Outcome {
        let handle = args.first().and_then(Value::as_u64).unwrap_or_default();
        match method {
            "$getAnonymousClient" => {
                if !self.github_clients.anonymous.insert(handle) {
                    return Outcome::Ready(Ok(error("unknown", "The GitHub client could not be created.")));
                }
                Outcome::Ready(Ok(json!({ "kind": "ok" })))
            }
            "$releaseClient" => {
                self.github_clients.anonymous.remove(&handle);
                Outcome::done()
            }
            "$getGist" if self.github_clients.anonymous.contains(&handle) => {
                let gist_id = args.get(1).and_then(Value::as_str).unwrap_or_default().to_string();
                let (sender, receiver) = oneshot::channel();
                cx.background_executor()
                    .spawn(async move {
                        let _ = sender.send(Ok(fetch_gist(&gist_id)));
                    })
                    .detach();
                Outcome::Pending(receiver)
            }
            "$getClient" => Outcome::Ready(Ok(error("authentication", "The selected GitHub session is not available to this extension."))),
            _ => Outcome::Ready(Ok(error("disposed", "The GitHub client has been disposed."))),
        }
    }
}

/// `client.gist.get` and `toGistDto`.
fn fetch_gist(gist_id: &str) -> Value {
    // routing.ts `pathSegment`: one encoded segment.
    let segment: String = gist_id.bytes().map(|byte| if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) { (byte as char).to_string() } else { format!("%{byte:02X}") }).collect();
    let url = format!("https://api.github.com/gists/{segment}");
    let headers = [("Accept", "application/vnd.github+json"), ("X-GitHub-Api-Version", API_VERSION)];
    let response = match request(&Request { headers: &headers, ..Request::get(&url) }) {
        Ok(response) => response,
        Err(failure) => return error("network", &failure.to_string()),
    };
    if !response.is_success() {
        let code = match response.status {
            401 => "authentication",
            403 => "authorization",
            404 => "notFound",
            422 => "validation",
            429 => "rateLimit",
            500.. => "server",
            _ => "unknown",
        };
        return json!({ "kind": "error", "error": { "code": code, "message": format!("GitHub request failed with status {}", response.status), "status": response.status } });
    }
    match response.as_json() {
        Ok(gist) => json!({ "kind": "ok", "value": gist_dto(&gist) }),
        Err(failure) => error("invalidResponse", &failure.to_string()),
    }
}

fn gist_dto(gist: &Value) -> Value {
    let files: Vec<Value> = gist["files"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(_, file)| json!({ "content": file["content"].as_str().unwrap_or_default(), "isBinary": file["encoding"] == "base64", "truncated": file["truncated"] == true }))
        .collect();
    json!({ "id": gist["id"], "htmlUrl": gist["html_url"], "files": files })
}

fn error(code: &str, message: &str) -> Value {
    json!({ "kind": "error", "error": { "code": code, "message": message } })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gists_become_dtos() {
        let gist = json!({ "id": "abc", "html_url": "https://gist.github.com/abc", "files": { "profile.json": { "content": "{}", "encoding": "utf-8", "truncated": false } } });
        assert_eq!(gist_dto(&gist), json!({ "id": "abc", "htmlUrl": "https://gist.github.com/abc", "files": [{ "content": "{}", "isBinary": false, "truncated": false }] }));
    }
}
