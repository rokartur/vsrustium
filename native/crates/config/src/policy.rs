//! policy.ts, filePolicyService.ts and configurations.ts `PolicyConfiguration`: policy values by policy name,
//! and the configuration layer they make for the settings that declare a `policy`.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use serde_json::Value;

use crate::json::{self, NodeType, ParseOptions};
use crate::model::ConfigurationModel;
use crate::registry::Registry;

/// `PolicyName` to `PolicyValue` (a string, number or boolean).
pub type PolicyValues = BTreeMap<String, Value>;

/// `getPolicyConfigurations` and `getPolicyReferenceConfigurations`: each policy name and the settings it
/// applies to, the owner and its references, included or not, with each one's schema.
pub fn policy_configurations(registry: &Registry) -> BTreeMap<String, Vec<(String, &Value)>> {
    let mut names: BTreeMap<String, Vec<(String, &Value)>> = BTreeMap::new();
    let included = registry.keys().iter().map(|key| (key, &registry.property(key).expect("keys are registered").schema));
    for (key, schema) in included.chain(&registry.excluded) {
        let name = schema["policy"]["name"].as_str().or(schema["policyReference"]["name"].as_str());
        if let Some(name) = name {
            names.entry(name.to_owned()).or_default().push((key.clone(), schema));
        }
    }
    names
}

/// `FilePolicyService.read`: the values of the policies `registry` defines; a missing file means none.
pub fn read_policy_file(path: &Path, registry: &Registry) -> Result<PolicyValues, String> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(PolicyValues::new()),
        Err(error) => return Err(error.to_string()),
    };
    let Ok(Value::Object(raw)) = serde_json::from_str::<Value>(&content) else {
        return Err("Policy file isn't a JSON object".into());
    };
    let definitions = policy_configurations(registry);
    Ok(raw.into_iter().filter(|(name, _)| definitions.contains_key(name)).collect())
}

/// `PolicyConfiguration.update` over every policy-controlled setting: the policy value of each, with a string
/// parsed as JSON when the setting cannot hold a string. A value that does not parse is left out.
pub fn policy_model(registry: &Registry, values: &PolicyValues) -> ConfigurationModel {
    let mut model = ConfigurationModel::default();
    for (name, settings) in policy_configurations(registry) {
        let Some(value) = values.get(&name) else { continue };
        for (key, schema) in settings {
            let accepts_string = match &schema["type"] {
                Value::Array(types) => types.iter().any(|kind| kind == "string"),
                kind => kind == "string",
            };
            let value = match value.as_str() {
                Some(text) if !accepts_string => match parse(text) {
                    Ok(parsed) => parsed,
                    Err(error) => {
                        eprintln!("Error parsing policy value {name}: {error}");
                        continue;
                    }
                },
                _ => value.clone(),
            };
            model.set_value(&key, value);
        }
    }
    model
}

/// `PolicyConfiguration.parse`: JSON with comments, an error on a syntax error or a duplicate property.
fn parse(content: &str) -> Result<Value, String> {
    let mut errors = Vec::new();
    let tree = json::parse_tree(content, &mut errors, ParseOptions::default());
    if let Some(error) = errors.first() {
        return Err(format!("{:?}", error.error));
    }
    let mut seen = HashSet::new();
    for node in tree.nodes.iter().filter(|node| node.kind == NodeType::Property) {
        let name = &tree.nodes[node.children[0]].value;
        if !seen.insert((node.parent, name.clone().to_string())) {
            return Err(format!("Duplicate property found: {}", name.as_str().unwrap_or_default()));
        }
    }
    Ok(json::parse(content, &mut errors, ParseOptions::default()).unwrap_or(Value::Object(Default::default())))
}

#[cfg(test)]
mod tests;
