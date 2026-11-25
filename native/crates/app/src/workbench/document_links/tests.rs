use serde_json::{Value, json};

use super::{Link, union};

fn link(handle: u64, range: [u64; 4], url: &str) -> Link {
    let [start_line, start_column, end_line, end_column] = range;
    let dto = json!({ "range": { "startLineNumber": start_line, "startColumn": start_column, "endLineNumber": end_line, "endColumn": end_column }, "url": url, "cacheId": [1, 2] });
    Link { handle, resolves: false, dto }
}

fn urls(links: &[Link]) -> Vec<&str> {
    links.iter().map(|link| link.dto["url"].as_str().unwrap()).collect()
}

#[test]
fn a_later_provider_replaces_links_it_touches_and_keeps_the_order_by_start() {
    let old = vec![link(1, [1, 1, 1, 5], "a"), link(1, [2, 1, 2, 5], "b"), link(1, [4, 1, 4, 5], "c")];
    let new = vec![link(2, [1, 5, 1, 9], "x"), link(2, [3, 1, 3, 5], "y")];
    assert_eq!(urls(&union(old, new)), ["x", "b", "y", "c"]);
}

#[test]
fn the_command_result_drops_the_cache_id() {
    assert_eq!(link(1, [1, 1, 1, 2], "a").to_json().as_object().unwrap().keys().collect::<Vec<_>>(), ["range", "url"]);
    let without_url = Link { handle: 1, resolves: true, dto: json!({ "range": {}, "cacheId": [1, 2] }) };
    assert_eq!(without_url.to_json(), json!({ "range": {} }));
    assert_eq!(without_url.to_json()["url"], Value::Null);
}
