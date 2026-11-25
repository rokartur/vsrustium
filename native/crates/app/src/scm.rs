//! The SCM model of mainThreadSCM.ts (`MainThreadSCMProvider`, `MainThreadSCMResourceGroup`,
//! `MainThreadSCMResource`) and the input of scmService.ts, built from the extension host's calls. The view is
//! `workbench/scm_view.rs`.

use serde_json::Value;

/// A source control (`ISCMProvider` with its `ISCMRepository` input).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Provider {
    pub handle: u64,
    pub id: String,
    pub label: String,
    /// `rootUri` as `UriComponents`.
    pub root_uri: Value,
    pub input: Input,
    /// `SCMProviderFeatures`, merged as `$updateSourceControl` sends them.
    pub count: Option<u64>,
    pub commit_template: Option<String>,
    pub accept_input_command: Option<Value>,
    pub action_button: Option<Value>,
    pub status_bar_commands: Vec<Value>,
    pub context_value: Option<String>,
    pub has_quick_diff_provider: bool,
    pub has_history_provider: bool,
    pub groups: Vec<Group>,
    /// `$onDidChangeHistoryProviderCurrentHistoryItemRefs`: the current, remote and base refs.
    pub history_item_ref: Option<Value>,
    pub history_item_remote_ref: Option<Value>,
    pub history_item_base_ref: Option<Value>,
}

/// `ISCMInput`.
#[derive(Clone, Debug, PartialEq)]
pub struct Input {
    pub value: String,
    pub placeholder: String,
    pub enabled: bool,
    pub visible: bool,
    /// `$showValidationMessage`: the message (string or markdown value) and `InputValidationType` (0 error, 1 warning, 2 information).
    pub validation: Option<(String, u64)>,
    pub validation_provider: bool,
}

impl Default for Input {
    fn default() -> Self {
        Input { value: String::new(), placeholder: String::new(), enabled: true, visible: true, validation: None, validation_provider: false }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub handle: u64,
    pub id: String,
    pub label: String,
    pub hide_when_empty: bool,
    pub context_value: Option<String>,
    pub resources: Vec<Resource>,
}

/// `SCMRawResource`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Resource {
    pub handle: u64,
    pub uri: Value,
    /// The light and dark icons: a `ThemeIcon` (`{id}`) or a `UriComponents`.
    pub icons: [Value; 2],
    pub tooltip: String,
    pub strike_through: bool,
    pub faded: bool,
    pub context_value: Option<String>,
    pub command: Option<Value>,
    pub multi_diff_original: Option<Value>,
    pub multi_diff_modified: Option<Value>,
}

impl Provider {
    /// `MainThreadSCMProvider.count`: the features' count, else the resources of every group.
    pub fn count(&self) -> u64 {
        self.count.unwrap_or_else(|| self.groups.iter().map(|group| group.resources.len() as u64).sum())
    }

    fn group_mut(&mut self, handle: u64) -> Option<&mut Group> {
        self.groups.iter_mut().find(|group| group.handle == handle)
    }
}

/// The source controls of the window, in registration order (`ISCMService.repositories`).
#[derive(Default, Debug)]
pub struct Scm {
    pub providers: Vec<Provider>,
}

/// What a `MainThreadSCM` call changed, for the view and the extension host.
#[derive(Debug, PartialEq)]
pub enum ScmChange {
    None,
    /// A source control was added: the host learns of the selection and of an input value set before.
    Registered(u64),
    Changed,
}

impl Scm {
    pub fn provider(&self, handle: u64) -> Option<&Provider> {
        self.providers.iter().find(|provider| provider.handle == handle)
    }

    pub fn provider_mut(&mut self, handle: u64) -> Option<&mut Provider> {
        self.providers.iter_mut().find(|provider| provider.handle == handle)
    }

    /// Applies one `MainThreadSCMShape` call.
    pub fn apply(&mut self, method: &str, args: &[Value]) -> ScmChange {
        let handle = args.first().and_then(Value::as_u64).unwrap_or_default();
        let text = |index: usize| args.get(index).and_then(Value::as_str).unwrap_or_default().to_string();
        if method == "$registerSourceControl" {
            // `$registerSourceControl(handle, parentHandle, id, label, rootUri, iconPath, isHidden, inputBoxDocumentUri)`.
            self.providers.push(Provider { handle, id: text(2), label: text(3), root_uri: args.get(4).cloned().unwrap_or(Value::Null), ..Default::default() });
            return ScmChange::Registered(handle);
        }
        if method == "$unregisterSourceControl" {
            self.providers.retain(|provider| provider.handle != handle);
            return ScmChange::Changed;
        }
        let Some(provider) = self.provider_mut(handle) else { return ScmChange::None };
        match method {
            "$updateSourceControl" => update_features(provider, &args[1]),
            "$registerGroups" => {
                for group in args[1].as_array().into_iter().flatten() {
                    provider.groups.push(Group {
                        handle: group[0].as_u64().unwrap_or_default(),
                        id: group[1].as_str().unwrap_or_default().to_string(),
                        label: group[2].as_str().unwrap_or_default().to_string(),
                        hide_when_empty: group[3]["hideWhenEmpty"] == true,
                        context_value: group[3]["contextValue"].as_str().map(str::to_string),
                        resources: Vec::new(),
                    });
                }
                splice_resource_states(provider, &args[2]);
            }
            "$updateGroup" => {
                if let Some(group) = provider.group_mut(args[1].as_u64().unwrap_or_default()) {
                    let features = &args[2];
                    if let Some(hide) = features.get("hideWhenEmpty") {
                        group.hide_when_empty = *hide == true;
                    }
                    if let Some(context) = features.get("contextValue") {
                        group.context_value = context.as_str().map(str::to_string);
                    }
                }
            }
            "$updateGroupLabel" => {
                if let Some(group) = provider.group_mut(args[1].as_u64().unwrap_or_default()) {
                    group.label = text(2);
                }
            }
            "$unregisterGroup" => {
                let group = args[1].as_u64().unwrap_or_default();
                provider.groups.retain(|existing| existing.handle != group);
            }
            "$spliceResourceStates" => splice_resource_states(provider, &args[1]),
            "$setInputBoxValue" => provider.input.value = text(1),
            "$setInputBoxPlaceholder" => provider.input.placeholder = text(1),
            "$setInputBoxEnablement" => provider.input.enabled = args[1] == true,
            "$setInputBoxVisibility" => provider.input.visible = args[1] == true,
            "$showValidationMessage" => {
                let message = match &args[1] {
                    Value::String(message) => message.clone(),
                    markdown => markdown["value"].as_str().unwrap_or_default().to_string(),
                };
                provider.input.validation = Some((message, args[2].as_u64().unwrap_or_default()));
            }
            "$setValidationProviderIsEnabled" => provider.input.validation_provider = args[1] == true,
            "$onDidChangeHistoryProviderCurrentHistoryItemRefs" => {
                let some = |index: usize| args.get(index).cloned().filter(|value| !value.is_null());
                provider.history_item_ref = some(1);
                provider.history_item_remote_ref = some(2);
                provider.history_item_base_ref = some(3);
            }
            _ => return ScmChange::None,
        }
        ScmChange::Changed
    }
}

/// `MainThreadSCMProvider.$updateSourceControl`: each feature the call carries replaces the old one.
fn update_features(provider: &mut Provider, features: &Value) {
    let Some(features) = features.as_object() else { return };
    for (key, value) in features {
        match key.as_str() {
            "count" => provider.count = value.as_u64(),
            "commitTemplate" => provider.commit_template = value.as_str().map(str::to_string),
            "acceptInputCommand" => provider.accept_input_command = Some(value.clone()).filter(|value| !value.is_null()),
            "actionButton" => provider.action_button = Some(value.clone()).filter(|value| !value.is_null()),
            "statusBarCommands" => provider.status_bar_commands = value.as_array().cloned().unwrap_or_default(),
            "contextValue" => provider.context_value = value.as_str().map(str::to_string),
            "hasQuickDiffProvider" => provider.has_quick_diff_provider = *value == true,
            "hasHistoryProvider" => provider.has_history_provider = *value == true,
            _ => {}
        }
    }
}

/// `$spliceGroupResourceStates`: each group's splices applied last first, so earlier indices stay valid.
fn splice_resource_states(provider: &mut Provider, splices: &Value) {
    for group_splices in splices.as_array().into_iter().flatten() {
        let Some(group) = provider.group_mut(group_splices[0].as_u64().unwrap_or_default()) else { continue };
        let mut splices: Vec<&Value> = group_splices[1].as_array().into_iter().flatten().collect();
        splices.reverse();
        for splice in splices {
            let start = (splice[0].as_u64().unwrap_or_default() as usize).min(group.resources.len());
            let delete = (splice[1].as_u64().unwrap_or_default() as usize).min(group.resources.len() - start);
            let resources = splice[2].as_array().into_iter().flatten().map(resource);
            group.resources.splice(start..start + delete, resources);
        }
    }
}

fn resource(raw: &Value) -> Resource {
    let some = |value: &Value| Some(value.clone()).filter(|value| !value.is_null());
    let light = raw[2][0].clone();
    let dark = if raw[2][1].is_null() { light.clone() } else { raw[2][1].clone() };
    Resource {
        handle: raw[0].as_u64().unwrap_or_default(),
        uri: raw[1].clone(),
        icons: [light, dark],
        tooltip: raw[3].as_str().unwrap_or_default().to_string(),
        strike_through: raw[4] == true,
        faded: raw[5] == true,
        context_value: raw[6].as_str().filter(|value| !value.is_empty()).map(str::to_string),
        command: some(&raw[7]),
        multi_diff_original: some(&raw[8]),
        multi_diff_modified: some(&raw[9]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn raw(handle: u64, path: &str) -> Value {
        json!([handle, { "scheme": "file", "path": path }, [{ "id": "diff-modified" }, null], "Modified", false, false, "", null, null, null])
    }

    #[test]
    fn groups_and_splices_build_the_resource_lists() {
        let mut scm = Scm::default();
        assert_eq!(scm.apply("$registerSourceControl", &[json!(0), json!(null), json!("git"), json!("Git"), json!({ "scheme": "file", "path": "/r" })]), ScmChange::Registered(0));
        scm.apply("$registerGroups", &[json!(0), json!([[1, "index", "Staged Changes", { "hideWhenEmpty": true }, false], [2, "workingTree", "Changes", {}, false]]), json!([[2, [[0, 0, [raw(1, "/r/a"), raw(2, "/r/c")]]]]])]);
        // Two splices of one group: applied from the last, so both indices refer to the old list.
        scm.apply("$spliceResourceStates", &[json!(0), json!([[2, [[1, 0, [raw(3, "/r/b")]], [2, 0, [raw(4, "/r/d")]]]]])]);
        let provider = scm.provider(0).unwrap();
        let paths: Vec<&str> = provider.groups[1].resources.iter().map(|resource| resource.uri["path"].as_str().unwrap()).collect();
        assert_eq!(paths, ["/r/a", "/r/b", "/r/c", "/r/d"]);
        assert!(provider.groups[0].hide_when_empty);
        assert_eq!(provider.count(), 4);
        scm.apply("$updateSourceControl", &[json!(0), json!({ "count": 7, "statusBarCommands": [{ "id": "git.checkout", "title": "$(git-branch) main" }] })]);
        assert_eq!(scm.provider(0).unwrap().count(), 7);
        assert_eq!(scm.provider(0).unwrap().status_bar_commands.len(), 1);
        scm.apply("$spliceResourceStates", &[json!(0), json!([[2, [[0, 4, []]]]])]);
        assert!(scm.provider(0).unwrap().groups[1].resources.is_empty());
        scm.apply("$unregisterSourceControl", &[json!(0)]);
        assert!(scm.providers.is_empty());
    }
}
