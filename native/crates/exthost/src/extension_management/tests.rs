use super::*;
use crate::zip::write_stored;

struct Fixture {
    root: PathBuf,
    extensions: UserExtensions,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("extension-management-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let extensions = UserExtensions { folder: root.join("extensions"), product_version: "1.147.0".into() };
        Fixture { root, extensions }
    }

    fn vsix(&self, version: &str, engine: &str) -> PathBuf {
        let manifest = json!({ "name": "Sample", "publisher": "Port", "version": version, "engines": { "vscode": engine }, "main": "./main.js" }).to_string();
        let file = self.root.join(format!("sample-{version}.vsix"));
        let archive = write_stored(&[("extension.vsixmanifest", b"<xml/>"), ("extension/package.json", manifest.as_bytes()), ("extension/main.js", b"exports.activate = () => {};")]);
        std::fs::write(&file, archive).unwrap();
        file
    }

    fn profile_ids(&self) -> Vec<(String, String)> {
        let entries: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(self.extensions.folder.join("extensions.json")).unwrap()).unwrap();
        entries.iter().map(|entry| (entry_id(entry).to_owned(), entry["relativeLocation"].as_str().unwrap().to_owned())).collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn installs_a_vsix_into_the_profile() {
    let fixture = Fixture::new("install");
    let installed = fixture.extensions.install_vsix(&fixture.vsix("1.0.0", "^1.80.0")).unwrap();
    assert_eq!(installed.id, "Port.Sample");
    assert_eq!(installed.location, fixture.extensions.folder.join("port.sample-1.0.0"));
    assert!(installed.location.join("main.js").is_file());
    assert_eq!(installed.description["isBuiltin"], json!(false));
    assert_eq!(fixture.profile_ids(), [("port.sample".to_owned(), "port.sample-1.0.0".to_owned())]);
    let metadata = &fixture.extensions.profile().unwrap()[0]["metadata"];
    assert_eq!(metadata["source"], json!("vsix"));
    let scanned = fixture.extensions.scan().unwrap();
    assert_eq!(scanned.iter().map(|extension| extension.id.as_str()).collect::<Vec<_>>(), ["Port.Sample"]);
}

#[test]
fn refuses_an_incompatible_engine() {
    let fixture = Fixture::new("engine");
    let Err(error) = fixture.extensions.install_vsix(&fixture.vsix("1.0.0", "^1.200.0")) else { panic!("installed") };
    assert_eq!(error.to_string(), "Unable to install extension 'port.sample' as it is not compatible with VS Code '1.147.0'.");
    assert!(!fixture.extensions.folder.join("port.sample-1.0.0").exists());
}

#[test]
fn refuses_a_folder_name_outside_the_folder() {
    let fixture = Fixture::new("escape");
    let victim = fixture.root.join("victim");
    std::fs::create_dir_all(&victim).unwrap();
    std::fs::create_dir_all(fixture.extensions.folder.join("port.sample-1")).unwrap();
    let manifest = json!({ "name": "sample", "publisher": "port", "version": "1/../../victim", "engines": { "vscode": "^1.80.0" } }).to_string();
    let vsix = fixture.root.join("escape.vsix");
    std::fs::write(&vsix, write_stored(&[("extension/package.json", manifest.as_bytes())])).unwrap();
    let Err(error) = fixture.extensions.install_vsix(&vsix) else { panic!("installed") };
    assert_eq!(error.to_string(), "VSIX invalid: 'port.sample-1/../../victim' is not a valid extension folder name.");
    assert!(victim.exists());
}

#[test]
fn an_update_marks_the_old_version_for_removal() {
    let fixture = Fixture::new("update");
    fixture.extensions.install_vsix(&fixture.vsix("1.0.0", "^1.80.0")).unwrap();
    fixture.extensions.install_vsix(&fixture.vsix("1.1.0", "^1.80.0")).unwrap();
    assert_eq!(fixture.profile_ids(), [("port.sample".to_owned(), "port.sample-1.1.0".to_owned())]);
    assert_eq!(fixture.extensions.removed().unwrap(), json!({ "port.sample-1.0.0": true }).as_object().unwrap().clone());
    fixture.extensions.clean_up().unwrap();
    assert!(!fixture.extensions.folder.join("port.sample-1.0.0").exists());
    assert!(fixture.extensions.removed().unwrap().is_empty());
}

#[test]
fn uninstall_and_reinstall() {
    let fixture = Fixture::new("uninstall");
    let vsix = fixture.vsix("1.0.0", "^1.80.0");
    fixture.extensions.install_vsix(&vsix).unwrap();
    fixture.extensions.uninstall("port.sample").unwrap();
    assert!(fixture.extensions.scan().unwrap().is_empty());
    assert!(fixture.extensions.uninstall("port.sample").is_err());
    // Reinstalling before a restart unmarks and replaces the folder.
    fixture.extensions.install_vsix(&vsix).unwrap();
    assert!(fixture.extensions.removed().unwrap().is_empty());
    assert_eq!(fixture.extensions.scan().unwrap().len(), 1);
    let leftovers: Vec<_> = std::fs::read_dir(&fixture.extensions.folder).unwrap().flatten().map(|child| child.file_name()).collect();
    assert_eq!(leftovers.len(), 2, "{leftovers:?}");
}

#[test]
fn initializes_the_profile_from_the_folder() {
    let fixture = Fixture::new("initialize");
    let folder = fixture.extensions.folder.join("port.other-2.0.0");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("package.json"), r#"{"name":"other","publisher":"port","version":"2.0.0","engines":{"vscode":"^1.0.0"}}"#).unwrap();
    assert_eq!(fixture.extensions.scan().unwrap()[0].id, "port.other");
    assert_eq!(fixture.profile_ids(), [("port.other".to_owned(), "port.other-2.0.0".to_owned())]);
}

#[test]
fn skips_an_invalid_extension() {
    let fixture = Fixture::new("invalid");
    let folder = fixture.extensions.folder.join("port.bad-1.0.0");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("package.json"), r#"{"name":"bad","publisher":"port","version":"1.0.0","engines":{"vscode":"^9.0.0"},"main":"x"}"#).unwrap();
    assert!(fixture.extensions.scan().unwrap().is_empty());
}
