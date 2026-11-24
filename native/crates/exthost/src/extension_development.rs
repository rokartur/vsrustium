//! `--extensionDevelopmentPath` / `--extensionTestsPath`: the extension under development joins the
//! scanned set and init data names it and its test runner, as upstream's extension development host does.

use std::path::PathBuf;

use serde_json::{Value, json};

use crate::scan::{ScannedExtension, file_uri, scan_extension};

pub struct ExtensionDevelopment {
    pub extension_path: PathBuf,
    pub tests_path: Option<PathBuf>,
}

impl ExtensionDevelopment {
    /// Reads `--extensionDevelopmentPath=<dir>` and `--extensionTestsPath=<path>` (the `=` form upstream's scripts pass).
    /// Paths are made absolute without resolving symlinks, as environmentService's `path.resolve` does.
    pub fn from_args(args: &[String]) -> anyhow::Result<Option<Self>> {
        let value = |flag: &str| args.iter().find_map(|arg| arg.strip_prefix(flag)?.strip_prefix('=')).map(std::path::absolute).transpose();
        let extension_path = value("--extensionDevelopmentPath")?;
        let tests_path = value("--extensionTestsPath")?;
        match (extension_path, tests_path) {
            (Some(extension_path), tests_path) => Ok(Some(ExtensionDevelopment { extension_path, tests_path })),
            (None, Some(_)) => anyhow::bail!("--extensionTestsPath needs --extensionDevelopmentPath"),
            (None, None) => Ok(None),
        }
    }

    /// Adds the extension under development; it replaces a built-in with the same id.
    pub fn add_extension(&self, extensions: &mut Vec<ScannedExtension>) -> anyhow::Result<()> {
        let mut extension = scan_extension(&self.extension_path, false)?;
        extension.description["isUnderDevelopment"] = json!(true);
        extensions.retain(|other| !other.id.eq_ignore_ascii_case(&extension.id));
        extensions.push(extension);
        Ok(())
    }

    /// Sets `environment.extensionDevelopmentLocationURI` and `extensionTestsLocationURI` of `IExtensionHostInitData`.
    pub fn add_to_init_data(&self, init: &mut Value) {
        let environment = &mut init["environment"];
        environment["extensionDevelopmentLocationURI"] = json!([file_uri(&self.extension_path)]);
        if let Some(tests_path) = &self.tests_path {
            environment["extensionTestsLocationURI"] = file_uri(tests_path);
        }
        // localProcessExtensionHost.ts: an extension development host forwards native console output, tests from the CLI forward nothing.
        let tests_from_cli = self.tests_path.is_some();
        init["consoleForward"] = json!({ "includeStack": !tests_from_cli, "logNative": !tests_from_cli });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_equals_form_flags() {
        let dir = std::env::temp_dir();
        let args = |list: &[&str]| list.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
        let development = ExtensionDevelopment::from_args(&args(&["code-gpui", &format!("--extensionDevelopmentPath={}", dir.display()), &format!("--extensionTestsPath={}", dir.display())]))
            .unwrap()
            .unwrap();
        assert_eq!((development.extension_path, development.tests_path), (dir.clone(), Some(dir.clone())));
        assert!(ExtensionDevelopment::from_args(&args(&["code-gpui", "folder"])).unwrap().is_none());
        assert!(ExtensionDevelopment::from_args(&args(&["code-gpui", &format!("--extensionTestsPath={}", dir.display())])).is_err());
    }

    #[test]
    fn replaces_the_builtin_with_the_development_copy() {
        let dir = std::env::temp_dir().join(format!("extension-development-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("package.json"), r#"{"name":"vscode-api-tests","publisher":"vscode"}"#).unwrap();
        let mut extensions = vec![scan_extension(&dir, true).unwrap()];
        let development = ExtensionDevelopment { extension_path: dir.clone(), tests_path: Some(dir.join("out")) };
        development.add_extension(&mut extensions).unwrap();
        let mut init = json!({ "environment": {} });
        development.add_to_init_data(&mut init);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(extensions.len(), 1);
        assert_eq!((&extensions[0].description["isBuiltin"], &extensions[0].description["isUnderDevelopment"]), (&json!(false), &json!(true)));
        assert_eq!(init["environment"]["extensionTestsLocationURI"]["path"], json!(dir.join("out").to_string_lossy()));
    }
}
