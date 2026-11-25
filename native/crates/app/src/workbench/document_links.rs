//! getLinks.ts: the providers' document links of a model (`getLinks`, `LinksList._union`) and its
//! `_executeLinkProvider` command, over MainThreadLanguageFeatures' `$registerDocumentLinkProvider`.

use futures::channel::oneshot;
use gpui::Context;
use serde_json::{Map, Value, json};

use super::Workbench;
use crate::commands::Outcome;
use crate::language_features::Candidate;

const REGISTER: &str = "$registerDocumentLinkProvider";

impl Workbench {
    pub(super) fn register_document_links(&mut self) {
        self.commands.register("_executeLinkProvider", |workbench, args, _, cx| workbench.execute_link_provider(&args, cx));
    }

    /// `_executeLinkProvider(uri, resolveCount)`: the links of the model, the first `resolveCount` resolved.
    fn execute_link_provider(&mut self, args: &[Value], cx: &mut Context<Self>) -> Outcome {
        let Some(editor) = super::host_requests::uri_parts(&args.first().cloned().unwrap_or_default()).ok().and_then(|(scheme, path)| self.editors.model(scheme, &path)) else {
            return Outcome::Ready(Err(crate::commands::error_value("Illegal argument")));
        };
        let uri = editor.document.uri();
        let candidate = Candidate { uri: &uri, language: &editor.document.language_id };
        // `providers.ordered(model).reverse()`: the best provider's links are merged last and win.
        let mut providers: Vec<(u64, bool)> = self.language_features.ordered(REGISTER, &candidate).iter().map(|registration| (registration.handle, registration.args.get(2) == Some(&json!(true)))).collect();
        providers.reverse();
        let calls: Vec<_> = providers.iter().map(|&(handle, resolves)| (handle, resolves, self.host.call("ExtHostLanguageFeatures", "$provideDocumentLinks", vec![json!(handle), uri.clone()]))).collect();
        let resolve_count = args.get(1).and_then(Value::as_u64).unwrap_or(0) as usize;
        let (sender, receiver) = oneshot::channel();
        cx.spawn(async move |workbench, cx| {
            let mut links: Vec<Link> = Vec::new();
            let mut caches = Vec::new();
            for (handle, resolves, call) in calls {
                let Ok(Ok(list)) = call.await else { continue };
                if let Some(cache_id) = list["cacheId"].as_u64() {
                    caches.push((handle, cache_id));
                }
                let new_links = list["links"].as_array().into_iter().flatten().map(|dto| Link { handle, resolves, dto: dto.clone() }).collect();
                links = union(links, new_links);
            }
            for link in links.iter_mut().take(resolve_count) {
                // `Link.resolve`: a link without a url asks its provider once; a failure leaves it as it is.
                // The link's `cacheId` is a `ChainedCacheId`, `[list, link]`.
                let cache_id = link.dto["cacheId"].clone();
                if !link.dto["url"].is_null() || !link.resolves || cache_id.is_null() {
                    continue;
                }
                let Ok(call) = workbench.update(cx, |workbench, _| workbench.host.call("ExtHostLanguageFeatures", "$resolveDocumentLink", vec![json!(link.handle), cache_id])) else { return };
                if let Ok(Ok(resolved)) = call.await
                    && resolved.is_object()
                {
                    link.dto = resolved;
                }
            }
            let result: Vec<Value> = links.iter().map(Link::to_json).collect();
            let _ = sender.send(Ok(Value::Array(result)));
            let _ = workbench.update(cx, |workbench, _| {
                for (handle, cache_id) in caches {
                    workbench.host.send("ExtHostLanguageFeatures", "$releaseDocumentLinks", vec![json!(handle), json!(cache_id)]);
                }
            });
        })
        .detach();
        Outcome::Pending(receiver)
    }
}

/// `Link`: an `ILinkDto` with the provider it came from.
#[derive(Clone, Debug, PartialEq)]
struct Link {
    handle: u64,
    resolves: bool,
    dto: Value,
}

impl Link {
    /// `Link.toJSON`: the range, url and tooltip, without the cache id.
    fn to_json(&self) -> Value {
        let mut json = Map::new();
        for key in ["range", "url", "tooltip"] {
            if !self.dto[key].is_null() {
                json.insert(key.into(), self.dto[key].clone());
            }
        }
        Value::Object(json)
    }

    fn start(&self) -> (u64, u64) {
        let range = &self.dto["range"];
        (range["startLineNumber"].as_u64().unwrap_or(0), range["startColumn"].as_u64().unwrap_or(0))
    }

    fn end(&self) -> (u64, u64) {
        let range = &self.dto["range"];
        (range["endLineNumber"].as_u64().unwrap_or(0), range["endColumn"].as_u64().unwrap_or(0))
    }
}

/// `LinksList._union`: both lists by start, an old link that intersects or touches a new one dropped.
fn union(old: Vec<Link>, new: Vec<Link>) -> Vec<Link> {
    let mut result = Vec::with_capacity(old.len() + new.len());
    let mut old = old.into_iter().peekable();
    let mut new = new.into_iter().peekable();
    while let (Some(old_link), Some(new_link)) = (old.peek(), new.peek()) {
        // `Range.areIntersectingOrTouching`.
        if !(old_link.end() < new_link.start() || new_link.end() < old_link.start()) {
            old.next();
            continue;
        }
        // `Range.compareRangesUsingStarts`: by start, then by end.
        if (old_link.start(), old_link.end()) < (new_link.start(), new_link.end()) {
            result.push(old.next().expect("peeked"));
        } else {
            result.push(new.next().expect("peeked"));
        }
    }
    result.extend(old);
    result.extend(new);
    result
}

#[cfg(test)]
mod tests;
