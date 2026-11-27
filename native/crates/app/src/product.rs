//! platform/product: product.json of the VS Code tree (or the app bundle's Resources) with the port's overlay
//! (port/product.json) over it, read once at startup, with package.json's version when product.json has none (product.ts).

use std::path::Path;
use std::sync::OnceLock;

use serde_json::Value;

static PRODUCT: OnceLock<Value> = OnceLock::new();

/// Reads `<root>/product.json`, port/product.json over it, and `<root>/package.json`; a missing or broken file
/// leaves the built-in defaults.
pub fn load(root: &Path) {
    let read = |name: &str| std::fs::read(root.join(name)).ok().and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    let mut product = read("product.json").unwrap_or_else(|| Value::Object(Default::default()));
    let overlay: serde_json::Map<String, Value> = serde_json::from_str(include_str!("../../../../port/product.json")).expect("port/product.json is a JSON object");
    for (key, value) in overlay {
        product[key] = value;
    }
    if product["version"].is_null() {
        if let Some(version) = read("package.json").map(|package| package["version"].clone()) {
            product["version"] = version;
        }
    }
    let _ = PRODUCT.set(product);
}

pub fn get(key: &str) -> &'static Value {
    &PRODUCT.get_or_init(|| Value::Object(Default::default()))[key]
}

pub fn name_long() -> &'static str {
    get("nameLong").as_str().unwrap_or("VSRustium")
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_the_checkout_product() {
        super::load(std::path::Path::new(env!("VSCODE_SRC")));
        assert_eq!(super::name_long(), "VSRustium");
        assert_eq!(super::get("urlProtocol"), "vsrustium");
        assert_eq!(super::get("darwinBundleIdentifier"), "com.rokartur.vsrustium");
        assert!(super::get("version").as_str().is_some_and(|version| version.starts_with("1.147")));
    }
}
