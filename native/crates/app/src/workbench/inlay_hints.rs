//! inlayHints.ts `InlayHintsFragments` and the `_executeInlayHintProvider` command of inlayHintsController.ts.
// shortcut: the hints are not drawn in the editor; that needs injected text in the view lines, which place every
// column at `visible_column * char_width` today.

use futures::channel::oneshot;
use gpui::Context;
use serde_json::{Value, json};

use super::Workbench;
use crate::commands::Outcome;
use crate::language_features::Candidate;

const REGISTER: &str = "$registerInlayHintsProvider";

impl Workbench {
    pub(super) fn register_inlay_hints(&mut self) {
        self.commands.register("_executeInlayHintProvider", |workbench, args, _, cx| workbench.execute_inlay_hint_provider(&args, cx));
    }

    /// `_executeInlayHintProvider`: every provider's hints in a range of a model, by position.
    fn execute_inlay_hint_provider(&mut self, args: &[Value], cx: &mut Context<Self>) -> Outcome {
        let Some(editor) = super::host_requests::uri_parts(&args.first().cloned().unwrap_or_default()).ok().and_then(|(scheme, path)| self.editors.model(scheme, &path)) else {
            return Outcome::Ready(Err(crate::commands::error_value("Illegal argument")));
        };
        let uri = editor.document().uri();
        let range = args.get(1).cloned().unwrap_or_default();
        let language = editor.document().language_id.clone();
        let candidate = Candidate { uri: &uri, language: &language };
        // `registry.ordered(model).reverse()`.
        let calls: Vec<_> = self
            .language_features
            .ordered(REGISTER, &candidate)
            .iter()
            .rev()
            .map(|registration| (registration.handle, self.host.call("ExtHostLanguageFeatures", "$provideInlayHints", vec![json!(registration.handle), uri.clone(), range.clone()])))
            .collect();
        let (sender, receiver) = oneshot::channel();
        cx.spawn(async move |workbench, cx| {
            let mut hints = Vec::new();
            let mut caches = Vec::new();
            for (handle, call) in calls {
                let Ok(Ok(list)) = call.await else { continue };
                if let Some(cache_id) = list["cacheId"].as_u64() {
                    caches.push((handle, cache_id));
                }
                hints.extend(list["hints"].as_array().into_iter().flatten().cloned());
            }
            let position = |hint: &Value| (hint["position"]["lineNumber"].as_u64().unwrap_or(0), hint["position"]["column"].as_u64().unwrap_or(0));
            hints.sort_by_key(position);
            let _ = sender.send(Ok(Value::Array(hints)));
            let _ = workbench.update(cx, |workbench, _| {
                for (handle, cache_id) in caches {
                    workbench.host.send("ExtHostLanguageFeatures", "$releaseInlayHints", vec![json!(handle), json!(cache_id)]);
                }
            });
        })
        .detach();
        Outcome::Pending(receiver)
    }
}
