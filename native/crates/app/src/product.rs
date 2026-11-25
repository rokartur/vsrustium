//! platform/product: product.json of the checkout (or the app bundle's Resources), read once at startup, with
//! package.json's version when product.json has none (product.ts).

use std::path::Path;
use std::sync::OnceLock;

use serde_json::Value;

static PRODUCT: OnceLock<Value> = OnceLock::new();

/// Reads `<root>/product.json` and `<root>/package.json`; a missing or broken file leaves the built-in defaults.
pub fn load(root: &Path) {
    let read = |name: &str| std::fs::read(root.join(name)).ok().and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    let mut product = read("product.json").unwrap_or_else(|| Value::Object(Default::default()));
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
        super::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.."));
        assert_eq!(super::name_long(), "VSRustium");
        assert_eq!(super::get("urlProtocol"), "vsrustium");
        assert_eq!(super::get("darwinBundleIdentifier"), "com.rokartur.vsrustium");
        assert!(super::get("version").as_str().is_some_and(|version| version.starts_with("1.147")));
    }
}
