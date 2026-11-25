use serde_json::{Value, json};

use super::*;

/// Open VSX's answer to `/vscode/redhat/vscode-yaml/latest`: a pre-release 2.1.2026112008 and the release 2.0.0.
const VSCODE_YAML: &str = include_str!("vscode-yaml-latest.json");

fn version(number: &str, platform: Option<&str>) -> Value {
    json!({ "version": number, "targetPlatform": platform })
}

fn pre_release(number: &str, platform: Option<&str>) -> Value {
    json!({ "version": number, "targetPlatform": platform, "properties": [{ "key": "Microsoft.VisualStudio.Code.PreRelease", "value": "true" }] })
}

fn sorted(mut versions: Vec<Value>, platform: &str) -> Vec<Value> {
    sort_extension_versions(&mut versions, platform);
    versions
}

#[test]
fn sorting_extension_versions() {
    let single = vec![version("1.1.2", None)];
    assert_eq!(sorted(single.clone(), "darwin-x64"), single);
    let preferred = vec![version("1.1.2", Some("darwin-x64"))];
    assert_eq!(sorted(preferred.clone(), "darwin-x64"), preferred);
    let other = vec![version("1.1.2", Some("darwin-arm64"))];
    assert_eq!(sorted(other.clone(), "win32-x64"), other);
    let plain = vec![version("1.2.4", None), version("1.1.3", None), version("1.1.2", None), version("1.1.1", None)];
    assert_eq!(sorted(plain.clone(), "win32-arm64"), plain);

    let v = vec![version("1.2.4", Some("darwin-arm64")), version("1.2.4", Some("win32-arm64")), version("1.2.4", Some("linux-arm64")), version("1.1.3", None), version("1.1.2", None), version("1.1.1", None)];
    assert_eq!(sorted(v.clone(), "win32-arm64"), [&v[1], &v[0], &v[2], &v[3], &v[4], &v[5]].map(Clone::clone));
    let v = vec![version("1.2.4", None), version("1.2.3", Some("darwin-arm64")), version("1.2.3", Some("win32-arm64")), version("1.2.3", Some("linux-arm64")), version("1.1.2", None), version("1.1.1", None)];
    assert_eq!(sorted(v.clone(), "linux-arm64"), [&v[0], &v[3], &v[1], &v[2], &v[4], &v[5]].map(Clone::clone));
    let v = vec![version("1.2.4", None), version("1.1.2", None), version("1.1.1", None), version("1.0.0", Some("darwin-arm64")), version("1.0.0", Some("win32-arm64"))];
    assert_eq!(sorted(v.clone(), "win32-arm64"), [&v[0], &v[1], &v[2], &v[4], &v[3]].map(Clone::clone));
}

fn latest(versions: &[Value], all: &[&str]) -> Vec<Value> {
    filter_latest_extension_versions_for_target_platform(versions, "win32-x64", all)
}

#[test]
fn filter_latest_versions_keeps_one_release_and_one_pre_release_per_compatible_platform() {
    assert!(latest(&[], &["win32-x64"]).is_empty());
    let one = [version("1.0.0", Some("win32-x64"))];
    assert_eq!(latest(&one, &["win32-x64"]), one);
    let both = [version("1.0.0", Some("win32-x64")), pre_release("0.9.0", Some("win32-x64"))];
    assert_eq!(latest(&both, &["win32-x64"]), both);
    let other_platforms = [version("1.0.0", Some("win32-x64")), version("1.0.0", Some("darwin-x64")), version("1.0.0", Some("linux-x64"))];
    assert_eq!(latest(&other_platforms, &["win32-x64", "darwin-x64", "linux-x64"]), other_platforms);
    let undefined = [version("1.0.0", None), version("0.9.0", None)];
    assert_eq!(latest(&undefined, &["win32-x64"]), [undefined[0].clone()]);

    let v = [
        version("2.0.0", Some("win32-x64")),
        version("2.0.0", Some("darwin-x64")),
        pre_release("2.1.0", Some("win32-x64")),
        pre_release("2.1.0", Some("linux-x64")),
        version("2.0.0", None),
        pre_release("2.1.0", None),
    ];
    assert_eq!(latest(&v, &["win32-x64", "darwin-x64", "linux-x64"]), v[..4]);

    let v = [version("2.0.0", Some("win32-x64")), version("1.9.0", Some("darwin-x64")), version("1.8.0", None)];
    assert_eq!(latest(&v, &["win32-x64", "darwin-x64", "linux-x64"]), v[..2]);
    let v = [version("2.0.0", None), version("1.0.0", Some("win32-x64"))];
    assert_eq!(latest(&v, &["win32-x64", "darwin-x64"]), v[..1]);
}

#[test]
fn filter_latest_versions_prefers_the_platform_build_of_the_same_version() {
    let v = [version("1.0.0", None), version("1.0.0", Some("win32-x64"))];
    assert_eq!(latest(&v, &["win32-x64"]), [v[1].clone()]);
    let v = [pre_release("1.0.0", None), pre_release("1.0.0", Some("win32-x64"))];
    assert_eq!(latest(&v, &["win32-x64"]), [v[1].clone()]);
    let v = [version("1.0.0", Some("universal")), version("1.0.0", Some("win32-x64"))];
    assert_eq!(latest(&v, &["win32-x64"]), [v[1].clone()]);
    let v = [pre_release("1.1.0", None), pre_release("1.1.0", Some("win32-x64")), version("1.0.0", None), version("1.0.0", Some("win32-x64"))];
    assert_eq!(latest(&v, &["win32-x64"]), [v[1].clone(), v[3].clone()]);
    let v = [version("1.0.0", None), version("1.0.0", Some("darwin-x64"))];
    assert_eq!(latest(&v, &["win32-x64", "darwin-x64"]), v);
    let v = [version("1.0.0", None), version("1.0.0", Some("win32-x64")), version("0.9.0", Some("linux-arm64"))];
    assert_eq!(latest(&v, &["win32-x64", "darwin-x64"]), [v[1].clone(), v[2].clone()]);
    let v = [pre_release("0.15.0", None), version("0.14.0", None), version("0.6.0", Some("linux-x64")), pre_release("0.5.1", Some("linux-x64"))];
    assert_eq!(filter_latest_extension_versions_for_target_platform(&v, "linux-x64", &["linux-x64"]), v[..2]);
}

#[test]
fn blocked_is_a_whole_flag_token_or_a_blocking_object() {
    assert!(!is_blocked(&json!({})));
    assert!(is_blocked(&json!({ "flags": "validated, Blocked" })));
    assert!(is_blocked(&json!({ "blocking": { "origin": "policy" } })));
    assert!(!is_blocked(&json!({ "flags": "unblocked" })));
}

#[test]
fn open_vsx_latest_resource_becomes_a_gallery_extension() {
    let raw: Value = serde_json::from_str(VSCODE_YAML).unwrap();
    let all = all_target_platforms(&raw);
    assert_eq!(all, ["universal", "web"]);
    let extension = to_extension(&raw, &raw["versions"][1], &all);
    assert_eq!(extension.id, "redhat.vscode-yaml");
    assert_eq!(extension.uuid, "2061917f-f76a-458a-8da9-f162de22b97e");
    assert_eq!(extension.version, "2.0.0");
    assert_eq!(extension.display_name, "YAML");
    assert_eq!(extension.publisher_display_name, "Red Hat");
    assert_eq!(extension.install_count, 9442034.0);
    assert_eq!(extension.categories, ["Programming Languages", "Linters", "Snippets", "Formatters"]);
    assert_eq!(extension.target_platform, "universal");
    assert_eq!(extension.engine, "^1.91.0");
    assert!(!extension.is_pre_release_version);
    assert!(extension.has_pre_release_version);
    assert_eq!(extension.repository.as_deref(), Some("https://github.com/redhat-developer/vscode-yaml.git"));
    assert_eq!(
        extension.download.uri,
        "https://open-vsx.org/vscode/asset/redhat/vscode-yaml/2.0.0/Microsoft.VisualStudio.Services.VSIXPackage?redirect=true&targetPlatform=universal"
    );
    assert_eq!(extension.readme.unwrap().uri, "https://open-vsx.org/vscode/asset/redhat/vscode-yaml/2.0.0/Microsoft.VisualStudio.Services.Content.Details?targetPlatform=universal");
    assert!(extension.is_signed);
    assert!(extension.public_key.unwrap().starts_with("https://open-vsx.org/api/-/public-key/"));
    assert_eq!(extension.detail_link, "https://open-vsx.org/vscode/item?itemName=redhat.vscode-yaml");
    assert_eq!(extension_key(&to_extension(&raw, &raw["versions"][0], &all)), "redhat.vscode-yaml-2.1.2026112008-universal");
}

#[test]
fn version_choice_follows_the_requested_kind_and_the_engine() {
    let raw: Value = serde_json::from_str(VSCODE_YAML).unwrap();
    let all = all_target_platforms(&raw);
    let gallery = Gallery::open_vsx("1.147.0");
    let choose = |kind, compatible, gallery: &Gallery| {
        let mut versions = raw["versions"].as_array().unwrap().clone();
        let criteria = VersionCriteria { target_platform: "darwin-arm64", compatible, version: kind };
        gallery.valid_version("redhat.vscode-yaml", &mut versions, &criteria, &all).map(|index| versions[index]["version"].clone())
    };
    assert_eq!(choose(VersionKind::Release, true, &gallery), Some(json!("2.0.0")));
    assert_eq!(choose(VersionKind::Prerelease, true, &gallery), Some(json!("2.1.2026112008")));
    assert_eq!(choose(VersionKind::Latest, true, &gallery), Some(json!("2.1.2026112008")));
    assert_eq!(choose(VersionKind::Exact("2.0.0".into()), true, &gallery), Some(json!("2.0.0")));
    assert_eq!(choose(VersionKind::Exact("1.0.0".into()), false, &gallery), None);
    let old = Gallery::open_vsx("1.90.0");
    assert_eq!(choose(VersionKind::Release, true, &old), None);
    // Without the compatibility check the latest unblocked version is the fallback.
    assert_eq!(choose(VersionKind::Release, false, &old), Some(json!("2.0.0")));
}

#[test]
fn query_text_filters_and_body() {
    let (rest, filters) = take_query_filters("yaml category:\"Programming Languages\" tag:k8s featured lint");
    assert_eq!(rest, "yaml lint");
    assert_eq!(filters, [(filter::CATEGORY, Some("Programming Languages".into())), (filter::TAG, Some("k8s".into())), (filter::FEATURED, None)]);

    let query = Query::new(50).with_filter(filter::SEARCH_TEXT, &["yaml".into()]);
    assert_eq!(
        query.body(),
        json!({
            "filters": [{
                "criteria": [{ "filterType": 10, "value": "yaml" }, { "filterType": 8, "value": "Microsoft.VisualStudio.Code" }, { "filterType": 12, "value": "4096" }],
                "pageNumber": 1, "pageSize": 50, "sortBy": 0, "sortOrder": 0,
            }],
            "assetTypes": [],
            "flags": 0x20,
        })
    );
}

#[test]
fn extension_identifiers() {
    assert!(is_extension_identifier("redhat.vscode-yaml"));
    assert!(!is_extension_identifier("redhat"));
    assert!(!is_extension_identifier("-a.b"));
    assert!(!is_extension_identifier("a.b/c"));
}

#[test]
fn verification_failures_refuse_the_install() {
    assert!(check_verification(None, false).is_ok());
    assert!(check_verification(Some(VerificationCode::Success), false).is_ok());
    assert!(check_verification(Some(VerificationCode::NotSigned), true).is_ok());
    assert_eq!(check_verification(Some(VerificationCode::NotSigned), false).unwrap_err().to_string(), "Signature verification failed with 'NotSigned' error.");
    assert!(check_verification(Some(VerificationCode::EntryIsTampered), false).is_err());
}
