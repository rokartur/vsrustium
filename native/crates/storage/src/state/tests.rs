//! The cases of platform/state/test/node/state.test.ts. The delayed and immediate strategies are one strategy here.

use std::path::PathBuf;

use serde_json::{Value, json};

use super::FileStorage;

fn storage_file(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vsctests-statemainservice-{}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("storage.json");
    std::fs::write(&file, "").unwrap();
    file
}

fn item(service: &FileStorage, key: &str) -> Option<Value> {
    service.get_item(key).cloned()
}

#[test]
fn basics() {
    let file = storage_file("basics");
    let mut service = FileStorage::open(&file);
    service.set_item("some.key", Some(json!("some.value")));
    assert_eq!(item(&service, "some.key"), Some(json!("some.value")));
    service.remove_item("some.key");
    assert_eq!(item(&service, "some.key"), None);
    assert_eq!(item(&service, "some.unknonw.key"), None);
    service.set_item("some.other.key", Some(json!("some.other.value")));
    service.close();

    let mut service = FileStorage::open(&file);
    assert_eq!(item(&service, "some.other.key"), Some(json!("some.other.value")));
    service.set_item("some.other.key", Some(json!("some.other.value")));
    assert_eq!(item(&service, "some.other.key"), Some(json!("some.other.value")));
    service.set_item("some.undefined.key", None);
    assert_eq!(item(&service, "some.undefined.key"), None);
    service.set_item("some.null.key", Some(Value::Null));
    assert_eq!(item(&service, "some.null.key"), None);

    let keys = ["some.setItems.key1", "some.setItems.key2", "some.setItems.key3", "some.setItems.key4", "some.setItems.key5"];
    let data = [Some(json!("some.value")), Some(json!(0)), Some(json!(true)), Some(Value::Null), None];
    service.set_items(keys.iter().map(|key| key.to_string()).zip(data).collect());
    assert_eq!(keys.map(|key| item(&service, key)), [Some(json!("some.value")), Some(json!(0)), Some(json!(true)), None, None]);
    let data = [None, None, None, Some(Value::Null), None];
    service.set_items(keys.iter().map(|key| key.to_string()).zip(data).collect());
    assert_eq!(keys.map(|key| item(&service, key)), [None, None, None, None, None]);
    service.close();
}

#[test]
fn multiple_ops_are_applied() {
    let file = storage_file("multiple");
    let mut service = FileStorage::open(&file);
    for index in 1..=4 {
        service.set_item(&format!("some.key{index}"), Some(json!(format!("some.value{index}"))));
    }
    service.remove_item("some.key4");
    let values = |service: &FileStorage| (1..=4).map(|index| item(service, &format!("some.key{index}"))).collect::<Vec<_>>();
    let expected = vec![Some(json!("some.value1")), Some(json!("some.value2")), Some(json!("some.value3")), None];
    assert_eq!(values(&service), expected);
    service.close();
    let service = FileStorage::open(&file);
    assert_eq!(values(&service), expected);
}

#[test]
fn used_before_init() {
    let file = storage_file("before-init");
    let mut service = FileStorage::new(&file);
    for index in 1..=4 {
        service.set_item(&format!("some.key{index}"), Some(json!(format!("some.value{index}"))));
    }
    service.remove_item("some.key4");
    service.init();
    let values: Vec<_> = (1..=4).map(|index| item(&service, &format!("some.key{index}"))).collect();
    assert_eq!(values, [Some(json!("some.value1")), Some(json!("some.value2")), Some(json!("some.value3")), None]);
    service.close();
}

#[test]
fn used_after_close() {
    let file = storage_file("after-close");
    let mut service = FileStorage::open(&file);
    for index in 1..=4 {
        service.set_item(&format!("some.key{index}"), Some(json!(format!("some.value{index}"))));
    }
    service.close();
    service.set_item("some.key5", Some(json!("some.marker")));
    let contents = std::fs::read_to_string(&file).unwrap();
    assert!(contents.contains("some.value1"));
    assert!(!contents.contains("some.marker"));
}

#[test]
fn closed_before_init() {
    let file = storage_file("closed-before-init");
    let mut service = FileStorage::new(&file);
    for index in 1..=4 {
        service.set_item(&format!("some.key{index}"), Some(json!(format!("some.value{index}"))));
    }
    service.close();
    assert_eq!(std::fs::read_to_string(&file).unwrap().len(), 0);
}

#[test]
fn writes_json_stringify_with_four_spaces() {
    let file = storage_file("format");
    let mut service = FileStorage::open(&file);
    service.set_item("b", Some(json!({ "x": [1] })));
    service.set_item("a", Some(json!(true)));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "{\n    \"b\": {\n        \"x\": [\n            1\n        ]\n    },\n    \"a\": true\n}");
}
