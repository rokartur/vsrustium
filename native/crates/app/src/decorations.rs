//! decorationsService.ts with mainThreadDecorations.ts: the file decorations extensions provide (git's status
//! letters and colors), fetched per resource on demand, cached, merged across providers and bubbled to folders.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

/// `IDecorationData` as `$provideDecorations` answers it: `[bubble, tooltip, letter, themeColor]`.
#[derive(Clone, Debug, PartialEq)]
pub struct DecorationData {
    /// The merge order, heaviest first; mainThreadDecorations.ts gives extension data 10.
    pub weight: u32,
    pub bubble: bool,
    pub tooltip: Option<String>,
    pub letter: Option<String>,
    pub color: Option<String>,
}

impl DecorationData {
    pub fn from_reply(value: &Value) -> Option<DecorationData> {
        let data = value.as_array()?;
        let text = |index: usize| data.get(index).and_then(Value::as_str).filter(|text| !text.is_empty()).map(str::to_owned);
        Some(DecorationData { weight: 10, bubble: data.first() == Some(&Value::Bool(true)), tooltip: text(1), letter: text(2), color: data.get(3).and_then(|color| color["id"].as_str()).map(str::to_owned) })
    }
}

/// `IDecoration`: the merged color, badge and tooltip of a resource.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Decoration {
    pub color: Option<String>,
    pub letter: Option<String>,
    pub tooltip: Option<String>,
    /// A descendant bubbles: the badge is the dot `\u{ea71}` at 0.4 opacity instead of the letter.
    pub bubbled: bool,
}

/// One provider's answer for a resource; `None` while its request is pending.
type Entry = Option<Option<DecorationData>>;

/// The handle of markersFileDecorations.ts `MarkersDecorationsProvider`, which answers synchronously.
pub const MARKERS_PROVIDER: u64 = u64::MAX;

#[derive(Default)]
pub struct Decorations {
    /// Provider handles, the newest first (`_provider.unshift`).
    providers: Vec<u64>,
    /// By resource path (case-insensitive, as the TernarySearchTree's `ignorePathCasingKeyFn`), then provider.
    cache: HashMap<String, HashMap<u64, Entry>>,
    /// Resources a label asked for that are not requested yet, per provider.
    wanted: HashSet<(u64, String)>,
    next_id: u64,
}

fn key(path: &str) -> String {
    path.to_lowercase()
}

impl Decorations {
    pub fn register(&mut self, handle: u64) {
        self.providers.insert(0, handle);
    }

    pub fn is_registered(&self, handle: u64) -> bool {
        self.providers.contains(&handle)
    }

    pub fn unregister(&mut self, handle: u64) {
        self.providers.retain(|provider| *provider != handle);
        self.forget(handle, None);
    }

    /// A synchronous provider's data for a resource (`_fetchData` of a provider that answers at once).
    pub fn set(&mut self, handle: u64, path: &str, data: Option<DecorationData>) {
        self.cache.entry(key(path)).or_default().insert(handle, Some(data));
    }

    /// `onDidChange`: the resources' entries are dropped and asked for again at once (so folders keep their
    /// bubbles), or every entry of the provider for `None`.
    pub fn changed(&mut self, handle: u64, paths: Option<Vec<String>>) {
        self.forget(handle, paths.as_deref());
        for path in paths.into_iter().flatten() {
            self.wanted.insert((handle, path));
        }
    }

    fn forget(&mut self, handle: u64, paths: Option<&[String]>) {
        match paths {
            None => self.cache.values_mut().for_each(|entries| drop(entries.remove(&handle))),
            Some(paths) => {
                for path in paths {
                    if let Some(entries) = self.cache.get_mut(&key(path)) {
                        entries.remove(&handle);
                    }
                }
            }
        }
    }

    /// `getDecoration`: the merged decoration of `path`, with the bubbling descendants for a folder. Providers
    /// that have not answered for it are noted for `take_requests`.
    pub fn decoration(&mut self, path: &str, include_children: bool) -> Option<Decoration> {
        let entries = self.cache.get(&key(path));
        for &provider in &self.providers {
            if provider != MARKERS_PROVIDER && !entries.is_some_and(|entries| entries.contains_key(&provider)) {
                self.wanted.insert((provider, path.to_owned()));
            }
        }
        let mut data: Vec<(usize, &DecorationData)> = Vec::new();
        let rank = |handle: &u64| self.providers.iter().position(|provider| provider == handle).unwrap_or(usize::MAX);
        if let Some(entries) = self.cache.get(&key(path)) {
            data.extend(entries.iter().filter_map(|(handle, entry)| entry.as_ref()?.as_ref().map(|data| (rank(handle), data))));
        }
        let mut bubbled = false;
        if include_children {
            let prefix = format!("{}/", key(path).trim_end_matches('/'));
            for (child, entries) in &self.cache {
                if !child.starts_with(&prefix) {
                    continue;
                }
                for (handle, entry) in entries {
                    if let Some(Some(child_data)) = entry.as_ref().map(Option::as_ref).filter(|data| data.is_some_and(|data| data.bubble)) {
                        bubbled = true;
                        data.push((rank(handle), child_data));
                    }
                }
            }
        }
        if data.is_empty() {
            return None;
        }
        // `asDecoration`: by weight, heaviest first; the stable sort keeps provider order (newest first) within a weight.
        data.sort_by_key(|(rank, data)| (std::cmp::Reverse(data.weight), *rank));
        let color = data.iter().find_map(|(_, data)| data.color.clone());
        let letters: Vec<&str> = data.iter().filter(|(_, data)| !bubbled || !data.bubble).filter_map(|(_, data)| data.letter.as_deref()).collect();
        let mut tooltips: Vec<&str> = Vec::new();
        for tooltip in data.iter().filter_map(|(_, data)| data.tooltip.as_deref()).filter(|tooltip| !tooltip.trim().is_empty()) {
            if !tooltips.contains(&tooltip) {
                tooltips.push(tooltip);
            }
        }
        let tooltip = match bubbled {
            true => Some("Contains emphasized items".to_owned()),
            false => Some(tooltips.join(" \u{2022} ")).filter(|tooltip| !tooltip.is_empty()),
        };
        let letter = match bubbled {
            true => None,
            false => Some(letters.join(", ")).filter(|letter| !letter.is_empty()),
        };
        Some(Decoration { color, letter, tooltip, bubbled })
    }

    /// The requests to send, per provider: `[{ id, uri }]` with the ids' paths; their entries become pending.
    pub fn take_requests(&mut self) -> Vec<(u64, Vec<(u64, String)>)> {
        let mut requests: HashMap<u64, Vec<(u64, String)>> = HashMap::new();
        for (handle, path) in std::mem::take(&mut self.wanted) {
            if !self.providers.contains(&handle) {
                continue;
            }
            let entries = self.cache.entry(key(&path)).or_default();
            if entries.contains_key(&handle) {
                continue;
            }
            entries.insert(handle, None);
            self.next_id += 1;
            requests.entry(handle).or_default().push((self.next_id, path));
        }
        requests.into_iter().collect()
    }

    /// The answer to a request: each id's data, `None` for a resource the provider does not decorate.
    pub fn answered(&mut self, handle: u64, requests: &[(u64, String)], reply: &Value) {
        for (id, path) in requests {
            let Some(entries) = self.cache.get_mut(&key(path)) else { continue };
            // A change dropped the pending entry; its newer request answers instead.
            if entries.get(&handle) != Some(&None) {
                continue;
            }
            entries.insert(handle, Some(DecorationData::from_reply(&reply[id.to_string()])));
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn answer(decorations: &mut Decorations, replies: &[(&str, Value)]) {
        for (handle, requests) in decorations.take_requests() {
            let mut reply = serde_json::Map::new();
            for (id, path) in &requests {
                if let Some((_, data)) = replies.iter().find(|(reply_path, _)| reply_path == path) {
                    reply.insert(id.to_string(), data.clone());
                }
            }
            decorations.answered(handle, &requests, &Value::Object(reply));
        }
    }

    #[test]
    fn a_file_shows_its_letter_and_its_folder_the_bubble() {
        let mut decorations = Decorations::default();
        decorations.register(1);
        assert_eq!(decorations.decoration("/w/src/a.ts", false), None, "asked, not answered yet");
        answer(&mut decorations, &[("/w/src/a.ts", json!([true, "Modified", "M", { "id": "gitDecoration.modifiedResourceForeground" }]))]);
        let file = decorations.decoration("/w/src/a.ts", false).unwrap();
        assert_eq!((file.letter.as_deref(), file.tooltip.as_deref(), file.color.as_deref()), (Some("M"), Some("Modified"), Some("gitDecoration.modifiedResourceForeground")));
        assert!(decorations.take_requests().is_empty(), "cached");
        let folder = decorations.decoration("/w/src", true).unwrap();
        assert!(folder.bubbled);
        assert_eq!((folder.letter, folder.tooltip.as_deref(), folder.color.as_deref()), (None, Some("Contains emphasized items"), Some("gitDecoration.modifiedResourceForeground")));
    }

    #[test]
    fn providers_merge_newest_first_and_a_change_asks_again() {
        let mut decorations = Decorations::default();
        decorations.register(1);
        decorations.register(2);
        decorations.decoration("/w/a.ts", false);
        let mut requests = decorations.take_requests();
        requests.sort();
        assert_eq!(requests.iter().map(|(handle, _)| *handle).collect::<Vec<_>>(), [1, 2]);
        for (handle, requests) in &requests {
            let data = if *handle == 1 { json!([false, "Untracked", "U", { "id": "one" }]) } else { json!([false, "Other", "O", null]) };
            decorations.answered(*handle, requests, &json!({ requests[0].0.to_string(): data }));
        }
        let merged = decorations.decoration("/w/a.ts", false).unwrap();
        assert_eq!((merged.letter.as_deref(), merged.tooltip.as_deref(), merged.color.as_deref()), (Some("O, U"), Some("Other \u{2022} Untracked"), Some("one")));
        decorations.changed(1, Some(vec!["/w/a.ts".into()]));
        assert_eq!(decorations.take_requests().len(), 1);
        decorations.unregister(2);
        assert_eq!(decorations.decoration("/w/a.ts", false), None, "provider 1 is pending again");
    }
}
