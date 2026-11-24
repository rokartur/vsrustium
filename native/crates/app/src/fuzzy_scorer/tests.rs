//! base/test/common/fuzzyScorer.test.ts. Resources are macOS file paths, so the Windows branches
//! of upstream's tests are left out.

use super::*;

#[derive(Clone, Debug, PartialEq)]
struct Resource {
    /// `basename(fsPath)`.
    label: String,
    /// `dirname(fsPath)`.
    description: String,
    /// `fsPath`.
    path: String,
}

/// `URI.file(path)`: a relative path gets a leading slash.
fn file(path: &str) -> Resource {
    let path = if path.starts_with('/') { path.to_owned() } else { format!("/{path}") };
    resource(&path)
}

fn resource(path: &str) -> Resource {
    let (description, label) = match path.rfind('/') {
        Some(0) => ("/", &path[1..]),
        Some(index) => (&path[..index], &path[index + 1..]),
        None => (".", path),
    };
    Resource { label: label.to_owned(), description: description.to_owned(), path: path.to_owned() }
}

#[derive(Clone, Copy)]
enum Accessor<'a> {
    Resource,
    Null,
    Slash,
    Backslash,
    Description(Option<&'a str>),
}

const RESOURCE: Accessor<'static> = Accessor::Resource;
const NULL: Accessor<'static> = Accessor::Null;
const SLASH: Accessor<'static> = Accessor::Slash;
const BACKSLASH: Accessor<'static> = Accessor::Backslash;

fn scorer_item(resource: &Resource, accessor: Accessor) -> (String, Option<String>, Option<String>) {
    match accessor {
        Accessor::Resource | Accessor::Slash => (resource.label.clone(), Some(resource.description.clone()), Some(resource.path.clone())),
        Accessor::Null => (String::new(), None, None),
        Accessor::Backslash => (resource.label.clone(), Some(resource.description.replace('/', "\\")), Some(resource.path.replace('/', "\\"))),
        Accessor::Description(description) => (resource.label.clone(), description.map(str::to_owned), Some(resource.path.clone())),
    }
}

fn score_item(resource: Option<&Resource>, query: &str, allow_non_contiguous_matches: bool, accessor: Accessor) -> ItemScore {
    score_item_cached(resource, query, allow_non_contiguous_matches, accessor, &mut FuzzyScorerCache::default())
}

fn score_item_cached(resource: Option<&Resource>, query: &str, allow_non_contiguous_matches: bool, accessor: Accessor, cache: &mut FuzzyScorerCache) -> ItemScore {
    score_prepared(resource, &prepare_query(query), allow_non_contiguous_matches, accessor, cache)
}

fn score_prepared(resource: Option<&Resource>, query: &PreparedQuery, allow_non_contiguous_matches: bool, accessor: Accessor, cache: &mut FuzzyScorerCache) -> ItemScore {
    let fields = resource.map(|resource| scorer_item(resource, accessor));
    let item = fields.as_ref().map(|(label, description, path)| ScorerItem { label, description: description.as_deref(), path: path.as_deref() });
    score_item_fuzzy(item.as_ref(), query, allow_non_contiguous_matches, cache)
}

/// `[...].sort((r1, r2) => compareItemsByScore(r1, r2, query, true, ResourceAccessor))`.
fn sorted(resources: &[&Resource], query: &str) -> Vec<Resource> {
    let query = prepare_query(query);
    let mut cache = FuzzyScorerCache::default();
    let mut resources: Vec<Resource> = resources.iter().map(|resource| (*resource).clone()).collect();
    resources.sort_by(|a, b| compare_items_by_fuzzy_score(&item(a), &item(b), &query, true, &mut cache));
    resources
}

fn item(resource: &Resource) -> ScorerItem<'_> {
    ScorerItem { label: &resource.label, description: Some(&resource.description), path: Some(&resource.path) }
}

fn do_score(target: &str, query: &str, allow_non_contiguous_matches: Option<bool>) -> FuzzyScore {
    let query = prepare_query(query);
    score_fuzzy(target, &query.piece.normalized, &query.piece.normalized_lowercase, allow_non_contiguous_matches.unwrap_or(!query.piece.expect_contiguous_match))
}

fn do_score2(target: &str, query: &str, match_offset: usize) -> FuzzyScore2 {
    score_fuzzy2(target, &prepare_query(query), 0, match_offset)
}

fn values(query: &PreparedQuery) -> &[PreparedQueryPiece] {
    query.values.as_deref().unwrap_or_default()
}

#[test]
fn score_fuzzy_orders_scores() {
        let target = "HelLo-World";

        let scores = [
            do_score(target, "HelLo-World", Some(true)), // direct case match
            do_score(target, "hello-world", Some(true)), // direct mix-case match
            do_score(target, "HW", Some(true)),          // direct case prefix (multiple)
            do_score(target, "hw", Some(true)),          // direct mix-case prefix (multiple)
            do_score(target, "H", Some(true)),           // direct case prefix
            do_score(target, "h", Some(true)),           // direct mix-case prefix
            do_score(target, "W", Some(true)),           // direct case word prefix
            do_score(target, "Ld", Some(true)),          // in-string case match (multiple)
            do_score(target, "ld", Some(true)),          // in-string mix-case match (consecutive, avoids scattered hit)
            do_score(target, "w", Some(true)),           // direct mix-case word prefix
            do_score(target, "L", Some(true)),           // in-string case match
            do_score(target, "l", Some(true)),           // in-string mix-case match
            do_score(target, "4", Some(true)),           // no match
        ];

        let mut sorted_scores = scores.clone();
        sorted_scores.sort_by(|a, b| b.0.cmp(&a.0));
        assert_eq!(scores, sorted_scores);
}

#[test]
fn score_non_fuzzy() {
        let target = "HelLo-World";

        assert!(do_score(target, "HelLo-World", Some(false)).0 > 0);
        assert_eq!(do_score(target, "HelLo-World", Some(false)).1.len(), "HelLo-World".chars().count());

        assert!(do_score(target, "hello-world", Some(false)).0 > 0);
        assert_eq!(do_score(target, "HW", Some(false)).0, 0);
        assert!(do_score(target, "h", Some(false)).0 > 0);
        assert!(do_score(target, "ello", Some(false)).0 > 0);
        assert!(do_score(target, "ld", Some(false)).0 > 0);
        assert_eq!(do_score(target, "eo", Some(false)).0, 0);
}

#[test]
fn scoreitem_matches_are_proper() {
        let mut res = score_item(None, "something", true, RESOURCE);
        assert_eq!(res.score, 0);

        let resource = file("/xyz/some/path/someFile123.txt");

        res = score_item(Some(&resource), "something", true, NULL);
        assert_eq!(res.score, 0);

        // Path Identity
        let identity_res = score_item(Some(&resource), &resource.path, true, RESOURCE);
        assert!(identity_res.score != 0);
        assert_eq!(identity_res.description_match.as_deref().unwrap_or_default().len(), 1);
        assert_eq!(identity_res.label_match.as_deref().unwrap_or_default().len(), 1);
        assert_eq!(identity_res.description_match.as_deref().unwrap()[0].start, 0);
        assert_eq!(identity_res.description_match.as_deref().unwrap()[0].end, resource.description.chars().count());
        assert_eq!(identity_res.label_match.as_deref().unwrap()[0].start, 0);
        assert_eq!(identity_res.label_match.as_deref().unwrap()[0].end, resource.label.chars().count());

        // Basename Prefix
        let basename_prefix_res = score_item(Some(&resource), "som", true, RESOURCE);
        assert!(basename_prefix_res.score != 0);
        assert!(basename_prefix_res.description_match.is_none());
        assert_eq!(basename_prefix_res.label_match.as_deref().unwrap_or_default().len(), 1);
        assert_eq!(basename_prefix_res.label_match.as_deref().unwrap()[0].start, 0);
        assert_eq!(basename_prefix_res.label_match.as_deref().unwrap()[0].end, "som".chars().count());

        // Basename Camelcase
        let basename_camelcase_res = score_item(Some(&resource), "sF", true, RESOURCE);
        assert!(basename_camelcase_res.score != 0);
        assert!(basename_camelcase_res.description_match.is_none());
        assert_eq!(basename_camelcase_res.label_match.as_deref().unwrap_or_default().len(), 2);
        assert_eq!(basename_camelcase_res.label_match.as_deref().unwrap()[0].start, 0);
        assert_eq!(basename_camelcase_res.label_match.as_deref().unwrap()[0].end, 1);
        assert_eq!(basename_camelcase_res.label_match.as_deref().unwrap()[1].start, 4);
        assert_eq!(basename_camelcase_res.label_match.as_deref().unwrap()[1].end, 5);

        // Basename Match
        let basename_res = score_item(Some(&resource), "of", true, RESOURCE);
        assert!(basename_res.score != 0);
        assert!(basename_res.description_match.is_none());
        assert_eq!(basename_res.label_match.as_deref().unwrap_or_default().len(), 2);
        assert_eq!(basename_res.label_match.as_deref().unwrap()[0].start, 1);
        assert_eq!(basename_res.label_match.as_deref().unwrap()[0].end, 2);
        assert_eq!(basename_res.label_match.as_deref().unwrap()[1].start, 4);
        assert_eq!(basename_res.label_match.as_deref().unwrap()[1].end, 5);

        // Path Match
        let path_res = score_item(Some(&resource), "xyz123", true, RESOURCE);
        assert!(path_res.score != 0);
        assert!(path_res.description_match.is_some());
        assert!(path_res.label_match.is_some());
        assert_eq!(path_res.label_match.as_deref().unwrap_or_default().len(), 1);
        assert_eq!(path_res.label_match.as_deref().unwrap()[0].start, 8);
        assert_eq!(path_res.label_match.as_deref().unwrap()[0].end, 11);
        assert_eq!(path_res.description_match.as_deref().unwrap_or_default().len(), 1);
        assert_eq!(path_res.description_match.as_deref().unwrap()[0].start, 1);
        assert_eq!(path_res.description_match.as_deref().unwrap()[0].end, 4);

        // Ellipsis Match
        let ellipsis_res = score_item(Some(&resource), "…me/path/someFile123.txt", true, RESOURCE);
        assert!(ellipsis_res.score != 0);
        assert!(path_res.description_match.is_some());
        assert!(path_res.label_match.is_some());
        assert_eq!(path_res.label_match.as_deref().unwrap_or_default().len(), 1);
        assert_eq!(path_res.label_match.as_deref().unwrap()[0].start, 8);
        assert_eq!(path_res.label_match.as_deref().unwrap()[0].end, 11);
        assert_eq!(path_res.description_match.as_deref().unwrap_or_default().len(), 1);
        assert_eq!(path_res.description_match.as_deref().unwrap()[0].start, 1);
        assert_eq!(path_res.description_match.as_deref().unwrap()[0].end, 4);

        // No Match
        let no_res = score_item(Some(&resource), "987", true, RESOURCE);
        assert_eq!(no_res.score, 0);
        assert!(no_res.label_match.is_none());
        assert!(no_res.description_match.is_none());

        // No Exact Match
        let no_exact_res = score_item(Some(&resource), "\"sF\"", true, RESOURCE);
        assert_eq!(no_exact_res.score, 0);
        assert!(no_exact_res.label_match.is_none());
        assert!(no_exact_res.description_match.is_none());
        assert_eq!(no_res.score, no_exact_res.score);

        // Verify Scores
        assert!(identity_res.score > basename_prefix_res.score);
        assert!(basename_prefix_res.score > basename_res.score);
        assert!(basename_res.score > path_res.score);
        assert!(path_res.score > no_res.score);
}

#[test]
fn scoreitem_multiple() {
        let resource = file("/xyz/some/path/someFile123.txt");

        let res1 = score_item(Some(&resource), "xyz some", true, RESOURCE);
        assert!(res1.score != 0);
        assert_eq!(res1.label_match.as_deref().unwrap_or_default().len(), 1);
        assert_eq!(res1.label_match.as_deref().unwrap()[0].start, 0);
        assert_eq!(res1.label_match.as_deref().unwrap()[0].end, 4);
        assert_eq!(res1.description_match.as_deref().unwrap_or_default().len(), 1);
        assert_eq!(res1.description_match.as_deref().unwrap()[0].start, 1);
        assert_eq!(res1.description_match.as_deref().unwrap()[0].end, 4);

        let res2 = score_item(Some(&resource), "some xyz", true, RESOURCE);
        assert!(res2.score != 0);
        assert_eq!(res1.score, res2.score);
        assert_eq!(res2.label_match.as_deref().unwrap_or_default().len(), 1);
        assert_eq!(res2.label_match.as_deref().unwrap()[0].start, 0);
        assert_eq!(res2.label_match.as_deref().unwrap()[0].end, 4);
        assert_eq!(res2.description_match.as_deref().unwrap_or_default().len(), 1);
        assert_eq!(res2.description_match.as_deref().unwrap()[0].start, 1);
        assert_eq!(res2.description_match.as_deref().unwrap()[0].end, 4);

        let res3 = score_item(Some(&resource), "some xyz file file123", true, RESOURCE);
        assert!(res3.score != 0);
        assert!(res3.score > res2.score);
        assert_eq!(res3.label_match.as_deref().unwrap_or_default().len(), 1);
        assert_eq!(res3.label_match.as_deref().unwrap()[0].start, 0);
        assert_eq!(res3.label_match.as_deref().unwrap()[0].end, 11);
        assert_eq!(res3.description_match.as_deref().unwrap_or_default().len(), 1);
        assert_eq!(res3.description_match.as_deref().unwrap()[0].start, 1);
        assert_eq!(res3.description_match.as_deref().unwrap()[0].end, 4);

        let res4 = score_item(Some(&resource), "path z y", true, RESOURCE);
        assert!(res4.score != 0);
        assert!(res4.score < res2.score);
        assert_eq!(res4.label_match.as_deref().unwrap_or_default().len(), 0);
        assert_eq!(res4.description_match.as_deref().unwrap_or_default().len(), 2);
        assert_eq!(res4.description_match.as_deref().unwrap()[0].start, 2);
        assert_eq!(res4.description_match.as_deref().unwrap()[0].end, 4);
        assert_eq!(res4.description_match.as_deref().unwrap()[1].start, 10);
        assert_eq!(res4.description_match.as_deref().unwrap()[1].end, 14);
}

#[test]
fn scoreitem_multiple_with_cache_yields_different_results() {
        let resource = file("/xyz/some/path/someFile123.txt");
        let mut cache = FuzzyScorerCache::default();
        let res1 = score_item_cached(Some(&resource), "xyz sm", true, RESOURCE, &mut cache);
        assert!(res1.score != 0);

        // from the cache's perspective this should be a totally different query
        let res2 = score_item_cached(Some(&resource), "xyz \"sm\"", true, RESOURCE, &mut cache);
        assert_eq!(res2.score, 0);
}

#[test]
fn scoreitem_cache_key_matches_the_hash_of_the_scoring_inputs() {
        let resource = file("/xyz/some/path/someFile123.txt");

        // The port keys its cache by the same inputs upstream hashes, so one non-empty query adds
        // exactly one entry. Lone surrogates ("\uD800") cannot occur in a Rust string.
        for query_value in ["some", "xyz sm", "xyz \"sm\"", "", "ünïcödé", "null\u{0000}char", "tab\there", "0", "01", "4294967295", "__proto__", "constructor", "\u{1F680}"] {
            for description in [None, Some(""), Some(resource.description.as_str()), Some("\u{0000}:[]"), Some("\u{1F680}")] {
                for allow_non_contiguous_matches in [true, false] {
                    let mut cache = FuzzyScorerCache::default();
                    score_item_cached(Some(&resource), query_value, allow_non_contiguous_matches, Accessor::Description(description), &mut cache);
                    assert_eq!(cache.len(), usize::from(!query_value.is_empty()), "unexpected cache keys for {query_value:?} {description:?} {allow_non_contiguous_matches}");
                }
            }
        }
}

#[test]
fn scoreitem_cache_keys_separate_items_queries_and_match_modes() {
        let mut cache = FuzzyScorerCache::default();

        // same label, different description
        score_item_cached(Some(&file("/xyz/some/path/someFile123.txt")), "some", true, RESOURCE, &mut cache);
        score_item_cached(Some(&file("/xyz/other/path/someFile123.txt")), "some", true, RESOURCE, &mut cache);

        // same item, different queries (incl. multi piece and quoted/contiguous)
        let resource = file("/xyz/some/path/someFile123.txt");
        score_item_cached(Some(&resource), "somefile", true, RESOURCE, &mut cache);
        score_item_cached(Some(&resource), "xyz sm", true, RESOURCE, &mut cache);
        score_item_cached(Some(&resource), "xyz \"sm\"", true, RESOURCE, &mut cache);
        score_item_cached(Some(&resource), "sm xyz", true, RESOURCE, &mut cache);

        // same item and query, different match mode
        score_item_cached(Some(&resource), "somefile", false, RESOURCE, &mut cache);

        // an item without description
        score_item_cached(Some(&file("/someFile123.txt")), "some", true, Accessor::Description(None), &mut cache);
        score_item_cached(Some(&file("/someFile123.txt")), "some", true, Accessor::Description(Some("")), &mut cache);

        assert_eq!(cache.len(), 9, "each distinct scoring input needs its own cache entry");
}

#[test]
fn scoreitem_repeated_scoring_hits_the_cache_and_preserves_the_result() {
        let resource = file("/xyz/some/path/someFile123.txt");

        for query_value in ["somefile", "xyz sm", "xyz \"sm\"", "ünïcödé"] {
            let mut cache = FuzzyScorerCache::default();
            let cold = score_item_cached(Some(&resource), query_value, true, RESOURCE, &mut cache);
            let warm = score_item_cached(Some(&resource), query_value, true, RESOURCE, &mut cache);

            assert_eq!(cache.len(), 1);
            assert_eq!(warm, cold, "warm scoring should return the cached result");
        }
}

#[test]
fn scoreitem_mutating_a_prepared_query_is_reflected_in_the_cache_key() {
        let resource = file("/xyz/some/path/someFile123.txt");
        let mut cache = FuzzyScorerCache::default();

        let mut query = prepare_query("xyz sm");
        let before = score_prepared(Some(&resource), &query, true, RESOURCE, &mut cache);
        assert!(before.score != 0);

        // callers own the prepared query object, so a mutated query must not reuse the previous entry
        query.values.as_mut().unwrap()[1].expect_contiguous_match = true;
        let after = score_prepared(Some(&resource), &query, true, RESOURCE, &mut cache);

        assert_eq!(cache.len(), 2);
        assert_eq!(after, score_prepared(Some(&resource), &prepare_query("xyz \"sm\""), true, RESOURCE, &mut FuzzyScorerCache::default()));
}

#[test]
fn scoreitem_invalid_input() {

        let mut res = score_item(None, "", true, RESOURCE);
        assert_eq!(res.score, 0);

        res = score_item(None, "null", true, RESOURCE);
        assert_eq!(res.score, 0);
}

#[test]
fn scoreitem_optimize_for_file_paths() {
        let resource = file("/xyz/others/spath/some/xsp/file123.txt");

        // xsp is more relevant to the end of the file path even though it matches
        // fuzzy also in the beginning. we verify the more relevant match at the
        // end gets returned.
        let path_res = score_item(Some(&resource), "xspfile123", true, RESOURCE);
        assert!(path_res.score != 0);
        assert!(path_res.description_match.is_some());
        assert!(path_res.label_match.is_some());
        assert_eq!(path_res.label_match.as_deref().unwrap_or_default().len(), 1);
        assert_eq!(path_res.label_match.as_deref().unwrap()[0].start, 0);
        assert_eq!(path_res.label_match.as_deref().unwrap()[0].end, 7);
        assert_eq!(path_res.description_match.as_deref().unwrap_or_default().len(), 1);
        assert_eq!(path_res.description_match.as_deref().unwrap()[0].start, 23);
        assert_eq!(path_res.description_match.as_deref().unwrap()[0].end, 26);
}

#[test]
fn scoreitem_avoid_match_scattering_bug_36119() {
        let resource = file("projects/ui/cula/ats/target.mk");

        let path_res = score_item(Some(&resource), "tcltarget.mk", true, RESOURCE);
        assert!(path_res.score != 0);
        assert!(path_res.description_match.is_some());
        assert!(path_res.label_match.is_some());
        assert_eq!(path_res.label_match.as_deref().unwrap_or_default().len(), 1);
        assert_eq!(path_res.label_match.as_deref().unwrap()[0].start, 0);
        assert_eq!(path_res.label_match.as_deref().unwrap()[0].end, 9);
}

#[test]
fn scoreitem_prefers_more_compact_matches() {
        let resource = file("/1a111d1/11a1d1/something.txt");

        // expect "ad" to be matched towards the end of the file because the
        // match is more compact
        let res = score_item(Some(&resource), "ad", true, RESOURCE);
        assert!(res.score != 0);
        assert!(res.description_match.is_some());
        assert!(res.label_match.as_deref().unwrap_or_default().is_empty());
        assert_eq!(res.description_match.as_deref().unwrap_or_default().len(), 2);
        assert_eq!(res.description_match.as_deref().unwrap()[0].start, 11);
        assert_eq!(res.description_match.as_deref().unwrap()[0].end, 12);
        assert_eq!(res.description_match.as_deref().unwrap()[1].start, 13);
        assert_eq!(res.description_match.as_deref().unwrap()[1].end, 14);
}

#[test]
fn scoreitem_proper_target_offset() {
        let resource = file("etem");

        let res = score_item(Some(&resource), "teem", true, RESOURCE);
        assert_eq!(res.score, 0);
}

#[test]
fn scoreitem_proper_target_offset_2() {
        let resource = file("ede");

        let res = score_item(Some(&resource), "de", true, RESOURCE);

        assert_eq!(res.label_match.as_deref().unwrap_or_default().len(), 1);
        assert_eq!(res.label_match.as_deref().unwrap()[0].start, 1);
        assert_eq!(res.label_match.as_deref().unwrap()[0].end, 3);
}

#[test]
fn scoreitem_proper_target_offset_3() {
        let resource = file("/src/vs/editor/browser/viewParts/lineNumbers/flipped-cursor-2x.svg");

        let res = score_item(Some(&resource), "debug", true, RESOURCE);

        assert_eq!(res.description_match.as_deref().unwrap_or_default().len(), 3);
        assert_eq!(res.description_match.as_deref().unwrap()[0].start, 9);
        assert_eq!(res.description_match.as_deref().unwrap()[0].end, 10);
        assert_eq!(res.description_match.as_deref().unwrap()[1].start, 36);
        assert_eq!(res.description_match.as_deref().unwrap()[1].end, 37);
        assert_eq!(res.description_match.as_deref().unwrap_or_default()[2].start, 40);
        assert_eq!(res.description_match.as_deref().unwrap_or_default()[2].end, 41);

        assert_eq!(res.label_match.as_deref().unwrap_or_default().len(), 2);
        assert_eq!(res.label_match.as_deref().unwrap()[0].start, 9);
        assert_eq!(res.label_match.as_deref().unwrap()[0].end, 10);
        assert_eq!(res.label_match.as_deref().unwrap()[1].start, 20);
        assert_eq!(res.label_match.as_deref().unwrap()[1].end, 21);
}

#[test]
fn scoreitem_no_match_unless_query_contained_in_sequence() {
        let resource = file("abcde");

        let res = score_item(Some(&resource), "edcda", true, RESOURCE);
        assert_eq!(res.score, 0);
}

#[test]
fn scoreitem_match_if_using_slash_or_backslash_local_remote_resource() {
        let local_resource = file("abcde/super/duper");
        // a vscode-remote URI keeps its relative path as fsPath
        let remote_resource = resource("abcde/super/duper");

        for resource in [local_resource, remote_resource] {
            let mut res = score_item(Some(&resource), "abcde\\super\\duper", true, RESOURCE);
            assert!(res.score != 0);

            res = score_item(Some(&resource), "abcde\\super\\duper", true, SLASH);
            assert!(res.score != 0);

            res = score_item(Some(&resource), "abcde\\super\\duper", true, BACKSLASH);
            assert!(res.score != 0);

            res = score_item(Some(&resource), "abcde/super/duper", true, RESOURCE);
            assert!(res.score != 0);

            res = score_item(Some(&resource), "abcde/super/duper", true, SLASH);
            assert!(res.score != 0);

            res = score_item(Some(&resource), "abcde/super/duper", true, BACKSLASH);
            assert!(res.score != 0);
        }
}

#[test]
fn scoreitem_ensure_upper_case_bonus_only_applies_on_non_consecutive_matches_bug_134723() {
        let resource_with_upper = file("ASDFasdfasdf");
        let resource_all_lower = file("asdfasdfasdf");

        assert!(score_item(Some(&resource_all_lower), "asdf", true, RESOURCE).score > score_item(Some(&resource_with_upper), "asdf", true, RESOURCE).score);
}

#[test]
fn compareitemsbyscore_identity() {
        let resource_a = file("/some/path/fileA.txt");
        let resource_b = file("/some/path/other/fileB.txt");
        let resource_c = file("/unrelated/some/path/other/fileC.txt");

        // Full resource A path
        let mut query = resource_a.path.clone();

        let mut res = sorted(&[&resource_a, &resource_b, &resource_c], &query);
        assert_eq!(res[0], resource_a);
        assert_eq!(res[1], resource_b);
        assert_eq!(res[2], resource_c);

        res = sorted(&[&resource_c, &resource_b, &resource_a], &query);
        assert_eq!(res[0], resource_a);
        assert_eq!(res[1], resource_b);
        assert_eq!(res[2], resource_c);

        // Full resource B path
        query = resource_b.path.clone();

        res = sorted(&[&resource_a, &resource_b, &resource_c], &query);
        assert_eq!(res[0], resource_b);
        assert_eq!(res[1], resource_a);
        assert_eq!(res[2], resource_c);

        res = sorted(&[&resource_c, &resource_b, &resource_a], &query);
        assert_eq!(res[0], resource_b);
        assert_eq!(res[1], resource_a);
        assert_eq!(res[2], resource_c);
}

#[test]
fn comparefilesbyscore_basename_prefix() {
        let resource_a = file("/some/path/fileA.txt");
        let resource_b = file("/some/path/other/fileB.txt");
        let resource_c = file("/unrelated/some/path/other/fileC.txt");

        // Full resource A basename
        let mut query = resource_a.label.clone();

        let mut res = sorted(&[&resource_a, &resource_b, &resource_c], &query);
        assert_eq!(res[0], resource_a);
        assert_eq!(res[1], resource_b);
        assert_eq!(res[2], resource_c);

        res = sorted(&[&resource_c, &resource_b, &resource_a], &query);
        assert_eq!(res[0], resource_a);
        assert_eq!(res[1], resource_b);
        assert_eq!(res[2], resource_c);

        // Full resource B basename
        query = resource_b.label.clone();

        res = sorted(&[&resource_a, &resource_b, &resource_c], &query);
        assert_eq!(res[0], resource_b);
        assert_eq!(res[1], resource_a);
        assert_eq!(res[2], resource_c);

        res = sorted(&[&resource_c, &resource_b, &resource_a], &query);
        assert_eq!(res[0], resource_b);
        assert_eq!(res[1], resource_a);
        assert_eq!(res[2], resource_c);
}

#[test]
fn comparefilesbyscore_basename_camelcase() {
        let resource_a = file("/some/path/fileA.txt");
        let resource_b = file("/some/path/other/fileB.txt");
        let resource_c = file("/unrelated/some/path/other/fileC.txt");

        // resource A camelcase
        let mut query = "fA".to_string();

        let mut res = sorted(&[&resource_a, &resource_b, &resource_c], &query);
        assert_eq!(res[0], resource_a);
        assert_eq!(res[1], resource_b);
        assert_eq!(res[2], resource_c);

        res = sorted(&[&resource_c, &resource_b, &resource_a], &query);
        assert_eq!(res[0], resource_a);
        assert_eq!(res[1], resource_b);
        assert_eq!(res[2], resource_c);

        // resource B camelcase
        query = "fB".to_string();

        res = sorted(&[&resource_a, &resource_b, &resource_c], &query);
        assert_eq!(res[0], resource_b);
        assert_eq!(res[1], resource_a);
        assert_eq!(res[2], resource_c);

        res = sorted(&[&resource_c, &resource_b, &resource_a], &query);
        assert_eq!(res[0], resource_b);
        assert_eq!(res[1], resource_a);
        assert_eq!(res[2], resource_c);
}

#[test]
fn comparefilesbyscore_basename_scores() {
        let resource_a = file("/some/path/fileA.txt");
        let resource_b = file("/some/path/other/fileB.txt");
        let resource_c = file("/unrelated/some/path/other/fileC.txt");

        // Resource A part of basename
        let mut query = "fileA".to_string();

        let mut res = sorted(&[&resource_a, &resource_b, &resource_c], &query);
        assert_eq!(res[0], resource_a);
        assert_eq!(res[1], resource_b);
        assert_eq!(res[2], resource_c);

        res = sorted(&[&resource_c, &resource_b, &resource_a], &query);
        assert_eq!(res[0], resource_a);
        assert_eq!(res[1], resource_b);
        assert_eq!(res[2], resource_c);

        // Resource B part of basename
        query = "fileB".to_string();

        res = sorted(&[&resource_a, &resource_b, &resource_c], &query);
        assert_eq!(res[0], resource_b);
        assert_eq!(res[1], resource_a);
        assert_eq!(res[2], resource_c);

        res = sorted(&[&resource_c, &resource_b, &resource_a], &query);
        assert_eq!(res[0], resource_b);
        assert_eq!(res[1], resource_a);
        assert_eq!(res[2], resource_c);
}

#[test]
fn comparefilesbyscore_path_scores() {
        let resource_a = file("/some/path/fileA.txt");
        let resource_b = file("/some/path/other/fileB.txt");
        let resource_c = file("/unrelated/some/path/other/fileC.txt");

        // Resource A part of path
        let mut query = "pathfileA".to_string();

        let mut res = sorted(&[&resource_a, &resource_b, &resource_c], &query);
        assert_eq!(res[0], resource_a);
        assert_eq!(res[1], resource_b);
        assert_eq!(res[2], resource_c);

        res = sorted(&[&resource_c, &resource_b, &resource_a], &query);
        assert_eq!(res[0], resource_a);
        assert_eq!(res[1], resource_b);
        assert_eq!(res[2], resource_c);

        // Resource B part of path
        query = "pathfileB".to_string();

        res = sorted(&[&resource_a, &resource_b, &resource_c], &query);
        assert_eq!(res[0], resource_b);
        assert_eq!(res[1], resource_a);
        assert_eq!(res[2], resource_c);

        res = sorted(&[&resource_c, &resource_b, &resource_a], &query);
        assert_eq!(res[0], resource_b);
        assert_eq!(res[1], resource_a);
        assert_eq!(res[2], resource_c);
}

#[test]
fn comparefilesbyscore_prefer_shorter_basenames() {
        let resource_a = file("/some/path/fileA.txt");
        let resource_b = file("/some/path/other/fileBLonger.txt");
        let resource_c = file("/unrelated/the/path/other/fileC.txt");

        // Resource A part of path
        let query = "somepath".to_string();

        let mut res = sorted(&[&resource_a, &resource_b, &resource_c], &query);
        assert_eq!(res[0], resource_a);
        assert_eq!(res[1], resource_b);
        assert_eq!(res[2], resource_c);

        res = sorted(&[&resource_c, &resource_b, &resource_a], &query);
        assert_eq!(res[0], resource_a);
        assert_eq!(res[1], resource_b);
        assert_eq!(res[2], resource_c);
}

#[test]
fn comparefilesbyscore_prefer_shorter_basenames_match_on_basename() {
        let resource_a = file("/some/path/fileA.txt");
        let resource_b = file("/some/path/other/fileBLonger.txt");
        let resource_c = file("/unrelated/the/path/other/fileC.txt");

        // Resource A part of path
        let query = "file".to_string();

        let mut res = sorted(&[&resource_a, &resource_b, &resource_c], &query);
        assert_eq!(res[0], resource_a);
        assert_eq!(res[1], resource_c);
        assert_eq!(res[2], resource_b);

        res = sorted(&[&resource_c, &resource_b, &resource_a], &query);
        assert_eq!(res[0], resource_a);
        assert_eq!(res[1], resource_c);
        assert_eq!(res[2], resource_b);
}

#[test]
fn comparefilesbyscore_prefer_shorter_paths() {
        let resource_a = file("/some/path/fileA.txt");
        let resource_b = file("/some/path/other/fileB.txt");
        let resource_c = file("/unrelated/some/path/other/fileC.txt");

        // Resource A part of path
        let query = "somepath".to_string();

        let mut res = sorted(&[&resource_a, &resource_b, &resource_c], &query);
        assert_eq!(res[0], resource_a);
        assert_eq!(res[1], resource_b);
        assert_eq!(res[2], resource_c);

        res = sorted(&[&resource_c, &resource_b, &resource_a], &query);
        assert_eq!(res[0], resource_a);
        assert_eq!(res[1], resource_b);
        assert_eq!(res[2], resource_c);
}

#[test]
fn comparefilesbyscore_prefer_shorter_paths_bug_17443() {
        let resource_a = file("config/test/t1.js");
        let resource_b = file("config/test.js");
        let resource_c = file("config/test/t2.js");

        let query = "co/te".to_string();

        let res = sorted(&[&resource_a, &resource_b, &resource_c], &query);
        assert_eq!(res[0], resource_b);
        assert_eq!(res[1], resource_a);
        assert_eq!(res[2], resource_c);
}

#[test]
fn comparefilesbyscore_prefer_matches_in_label_over_description_if_scores_are_otherwise_equal() {
        let resource_a = file("parts/quick/arrow-left-dark.svg");
        let resource_b = file("parts/quickopen/quickopen.ts");

        let query = "partsquick".to_string();

        let res = sorted(&[&resource_a, &resource_b], &query);
        assert_eq!(res[0], resource_b);
        assert_eq!(res[1], resource_a);
}

#[test]
fn comparefilesbyscore_prefer_camel_case_matches() {
        let resource_a = file("config/test/NullPointerException.java");
        let resource_b = file("config/test/nopointerexception.java");

        for query in ["npe", "NPE"] {
            let mut res = sorted(&[&resource_a, &resource_b], &query);
            assert_eq!(res[0], resource_a);
            assert_eq!(res[1], resource_b);

            res = sorted(&[&resource_b, &resource_a], &query);
            assert_eq!(res[0], resource_a);
            assert_eq!(res[1], resource_b);
        }
}

#[test]
fn comparefilesbyscore_prefer_more_compact_camel_case_matches() {
        let resource_a = file("config/test/openthisAnythingHandler.js");
        let resource_b = file("config/test/openthisisnotsorelevantforthequeryAnyHand.js");

        let query = "AH".to_string();

        let mut res = sorted(&[&resource_a, &resource_b], &query);
        assert_eq!(res[0], resource_b);
        assert_eq!(res[1], resource_a);

        res = sorted(&[&resource_b, &resource_a], &query);
        assert_eq!(res[0], resource_b);
        assert_eq!(res[1], resource_a);
}

#[test]
fn comparefilesbyscore_prefer_more_compact_matches_label() {
        let resource_a = file("config/test/examasdaple.js");
        let resource_b = file("config/test/exampleasdaasd.ts");

        let query = "xp".to_string();

        let mut res = sorted(&[&resource_a, &resource_b], &query);
        assert_eq!(res[0], resource_b);
        assert_eq!(res[1], resource_a);

        res = sorted(&[&resource_b, &resource_a], &query);
        assert_eq!(res[0], resource_b);
        assert_eq!(res[1], resource_a);
}

#[test]
fn comparefilesbyscore_prefer_more_compact_matches_path() {
        let resource_a = file("config/test/examasdaple/file.js");
        let resource_b = file("config/test/exampleasdaasd/file.ts");

        let query = "xp".to_string();

        let mut res = sorted(&[&resource_a, &resource_b], &query);
        assert_eq!(res[0], resource_b);
        assert_eq!(res[1], resource_a);

        res = sorted(&[&resource_b, &resource_a], &query);
        assert_eq!(res[0], resource_b);
        assert_eq!(res[1], resource_a);
}

#[test]
fn comparefilesbyscore_prefer_more_compact_matches_label_and_path() {
        let resource_a = file("config/example/thisfile.ts");
        let resource_b = file("config/24234243244/example/file.js");

        let query = "exfile".to_string();

        let mut res = sorted(&[&resource_a, &resource_b], &query);
        assert_eq!(res[0], resource_b);
        assert_eq!(res[1], resource_a);

        res = sorted(&[&resource_b, &resource_a], &query);
        assert_eq!(res[0], resource_b);
        assert_eq!(res[1], resource_a);
}

#[test]
fn comparefilesbyscore_avoid_match_scattering_bug_34210() {
        let resource_a = file("node_modules1/bundle/lib/model/modules/ot1/index.js");
        let resource_b = file("node_modules1/bundle/lib/model/modules/un1/index.js");
        let resource_c = file("node_modules1/bundle/lib/model/modules/modu1/index.js");
        let resource_d = file("node_modules1/bundle/lib/model/modules/oddl1/index.js");

        let mut query = "modu1/index.js";

        let mut res = sorted(&[&resource_a, &resource_b, &resource_c, &resource_d], &query);
        assert_eq!(res[0], resource_c);

        res = sorted(&[&resource_c, &resource_b, &resource_a, &resource_d], &query);
        assert_eq!(res[0], resource_c);

        query = "un1/index.js";

        res = sorted(&[&resource_a, &resource_b, &resource_c, &resource_d], &query);
        assert_eq!(res[0], resource_b);

        res = sorted(&[&resource_c, &resource_b, &resource_a, &resource_d], &query);
        assert_eq!(res[0], resource_b);
}

#[test]
fn comparefilesbyscore_avoid_match_scattering_bug_21019_1() {
        let resource_a = file("app/containers/Services/NetworkData/ServiceDetails/ServiceLoad/index.js");
        let resource_b = file("app/containers/Services/NetworkData/ServiceDetails/ServiceDistribution/index.js");
        let resource_c = file("app/containers/Services/NetworkData/ServiceDetailTabs/ServiceTabs/StatVideo/index.js");

        let query = "StatVideoindex".to_string();

        let mut res = sorted(&[&resource_a, &resource_b, &resource_c], &query);
        assert_eq!(res[0], resource_c);

        res = sorted(&[&resource_c, &resource_b, &resource_a], &query);
        assert_eq!(res[0], resource_c);
}

#[test]
fn comparefilesbyscore_avoid_match_scattering_bug_21019_2() {
        let resource_a = file("src/build-helper/store/redux.ts");
        let resource_b = file("src/repository/store/redux.ts");

        let query = "reproreduxts".to_string();

        let mut res = sorted(&[&resource_a, &resource_b], &query);
        assert_eq!(res[0], resource_b);

        res = sorted(&[&resource_b, &resource_a], &query);
        assert_eq!(res[0], resource_b);
}

#[test]
fn comparefilesbyscore_avoid_match_scattering_bug_26649() {
        let resource_a = file("photobook/src/components/AddPagesButton/index.js");
        let resource_b = file("photobook/src/components/ApprovalPageHeader/index.js");
        let resource_c = file("photobook/src/canvasComponents/BookPage/index.js");

        let query = "bookpageIndex".to_string();

        let mut res = sorted(&[&resource_a, &resource_b, &resource_c], &query);
        assert_eq!(res[0], resource_c);

        res = sorted(&[&resource_c, &resource_b, &resource_a], &query);
        assert_eq!(res[0], resource_c);
}

#[test]
fn comparefilesbyscore_avoid_match_scattering_bug_33247() {
        let resource_a = file("ui/src/utils/constants.js");
        let resource_b = file("ui/src/ui/Icons/index.js");

        let query = "ui/icons";

        let mut res = sorted(&[&resource_a, &resource_b], &query);
        assert_eq!(res[0], resource_b);

        res = sorted(&[&resource_b, &resource_a], &query);
        assert_eq!(res[0], resource_b);
}

#[test]
fn comparefilesbyscore_avoid_match_scattering_bug_33247_comment() {
        let resource_a = file("ui/src/components/IDInput/index.js");
        let resource_b = file("ui/src/ui/Input/index.js");

        let query = "ui/input/index";

        let mut res = sorted(&[&resource_a, &resource_b], &query);
        assert_eq!(res[0], resource_b);

        res = sorted(&[&resource_b, &resource_a], &query);
        assert_eq!(res[0], resource_b);
}

#[test]
fn comparefilesbyscore_avoid_match_scattering_bug_36166() {
        let resource_a = file("django/contrib/sites/locale/ga/LC_MESSAGES/django.mo");
        let resource_b = file("django/core/signals.py");

        let query = "djancosig".to_string();

        let mut res = sorted(&[&resource_a, &resource_b], &query);
        assert_eq!(res[0], resource_b);

        res = sorted(&[&resource_b, &resource_a], &query);
        assert_eq!(res[0], resource_b);
}

#[test]
fn comparefilesbyscore_avoid_match_scattering_bug_32918() {
        let resource_a = file("adsys/protected/config.php");
        let resource_b = file("adsys/protected/framework/smarty/sysplugins/smarty_internal_config.php");
        let resource_c = file("duowanVideo/wap/protected/config.php");

        let query = "protectedconfig.php".to_string();

        let mut res = sorted(&[&resource_a, &resource_b, &resource_c], &query);
        assert_eq!(res[0], resource_a);
        assert_eq!(res[1], resource_c);
        assert_eq!(res[2], resource_b);

        res = sorted(&[&resource_c, &resource_b, &resource_a], &query);
        assert_eq!(res[0], resource_a);
        assert_eq!(res[1], resource_c);
        assert_eq!(res[2], resource_b);
}

#[test]
fn comparefilesbyscore_avoid_match_scattering_bug_14879() {
        let resource_a = file("pkg/search/gradient/testdata/constraint_attrMatchString.yml");
        let resource_b = file("cmd/gradient/main.go");

        let query = "gradientmain".to_string();

        let mut res = sorted(&[&resource_a, &resource_b], &query);
        assert_eq!(res[0], resource_b);

        res = sorted(&[&resource_b, &resource_a], &query);
        assert_eq!(res[0], resource_b);
}

#[test]
fn comparefilesbyscore_avoid_match_scattering_bug_14727_1() {
        let resource_a = file("alpha-beta-cappa.txt");
        let resource_b = file("abc.txt");

        let query = "abc".to_string();

        let mut res = sorted(&[&resource_a, &resource_b], &query);
        assert_eq!(res[0], resource_b);

        res = sorted(&[&resource_b, &resource_a], &query);
        assert_eq!(res[0], resource_b);
}

#[test]
fn comparefilesbyscore_avoid_match_scattering_bug_14727_2() {
        let resource_a = file("xerxes-yak-zubba/index.js");
        let resource_b = file("xyz/index.js");

        let query = "xyz".to_string();

        let mut res = sorted(&[&resource_a, &resource_b], &query);
        assert_eq!(res[0], resource_b);

        res = sorted(&[&resource_b, &resource_a], &query);
        assert_eq!(res[0], resource_b);
}

#[test]
fn comparefilesbyscore_avoid_match_scattering_bug_18381() {
        let resource_a = file("AssymblyInfo.cs");
        let resource_b = file("IAsynchronousTask.java");

        let query = "async".to_string();

        let mut res = sorted(&[&resource_a, &resource_b], &query);
        assert_eq!(res[0], resource_b);

        res = sorted(&[&resource_b, &resource_a], &query);
        assert_eq!(res[0], resource_b);
}

#[test]
fn comparefilesbyscore_avoid_match_scattering_bug_35572() {
        let resource_a = file("static/app/source/angluar/-admin/-organization/-settings/layout/layout.js");
        let resource_b = file("static/app/source/angular/-admin/-project/-settings/_settings/settings.js");

        let query = "partisettings".to_string();

        let mut res = sorted(&[&resource_a, &resource_b], &query);
        assert_eq!(res[0], resource_b);

        res = sorted(&[&resource_b, &resource_a], &query);
        assert_eq!(res[0], resource_b);
}

#[test]
fn comparefilesbyscore_avoid_match_scattering_bug_36810() {
        let resource_a = file("Trilby.TrilbyTV.Web.Portal/Views/Systems/Index.cshtml");
        let resource_b = file("Trilby.TrilbyTV.Web.Portal/Areas/Admins/Views/Tips/Index.cshtml");

        let query = "tipsindex.cshtml".to_string();

        let mut res = sorted(&[&resource_a, &resource_b], &query);
        assert_eq!(res[0], resource_b);

        res = sorted(&[&resource_b, &resource_a], &query);
        assert_eq!(res[0], resource_b);
}

#[test]
fn comparefilesbyscore_prefer_shorter_hit_bug_20546() {
        let resource_a = file("editor/core/components/tests/list-view-spec.js");
        let resource_b = file("editor/core/components/list-view.js");

        let query = "listview".to_string();

        let mut res = sorted(&[&resource_a, &resource_b], &query);
        assert_eq!(res[0], resource_b);

        res = sorted(&[&resource_b, &resource_a], &query);
        assert_eq!(res[0], resource_b);
}

#[test]
fn comparefilesbyscore_avoid_match_scattering_bug_12095() {
        let resource_a = file("src/vs/workbench/contrib/files/common/explorerViewModel.ts");
        let resource_b = file("src/vs/workbench/contrib/files/browser/views/explorerView.ts");
        let resource_c = file("src/vs/workbench/contrib/files/browser/views/explorerViewer.ts");

        let query = "filesexplorerview.ts".to_string();

        let mut res = sorted(&[&resource_a, &resource_b, &resource_c], &query);
        assert_eq!(res[0], resource_b);

        res = sorted(&[&resource_a, &resource_c, &resource_b], &query);
        assert_eq!(res[0], resource_b);
}

#[test]
fn comparefilesbyscore_prefer_case_match_bug_96122() {
        let resource_a = file("lists.php");
        let resource_b = file("lib/Lists.php");

        let query = "Lists.php".to_string();

        let mut res = sorted(&[&resource_a, &resource_b], &query);
        assert_eq!(res[0], resource_b);

        res = sorted(&[&resource_b, &resource_a], &query);
        assert_eq!(res[0], resource_b);
}

#[test]
fn comparefilesbyscore_prefer_shorter_match_bug_103052_foo_bar() {
        let resource_a = file("app/emails/foo.bar.js");
        let resource_b = file("app/emails/other-footer.other-bar.js");

        for query in ["foo bar", "foobar"] {
            let mut res = sorted(&[&resource_a, &resource_b], &query);
            assert_eq!(res[0], resource_a);
            assert_eq!(res[1], resource_b);

            res = sorted(&[&resource_b, &resource_a], &query);
            assert_eq!(res[0], resource_a);
            assert_eq!(res[1], resource_b);
        }
}

#[test]
fn comparefilesbyscore_prefer_shorter_match_bug_103052_payment_model() {
        let resource_a = file("app/components/payment/payment.model.js");
        let resource_b = file("app/components/online-payments-history/online-payments-history.model.js");

        for query in ["payment model", "paymentmodel"] {
            let mut res = sorted(&[&resource_a, &resource_b], &query);
            assert_eq!(res[0], resource_a);
            assert_eq!(res[1], resource_b);

            res = sorted(&[&resource_b, &resource_a], &query);
            assert_eq!(res[0], resource_a);
            assert_eq!(res[1], resource_b);
        }
}

#[test]
fn comparefilesbyscore_prefer_shorter_match_bug_103052_color() {
        let resource_a = file("app/constants/color.js");
        let resource_b = file("app/components/model/input/pick-avatar-color.js");

        for query in ["color js", "colorjs"] {
            let mut res = sorted(&[&resource_a, &resource_b], &query);
            assert_eq!(res[0], resource_a);
            assert_eq!(res[1], resource_b);

            res = sorted(&[&resource_b, &resource_a], &query);
            assert_eq!(res[0], resource_a);
            assert_eq!(res[1], resource_b);
        }
}

#[test]
fn comparefilesbyscore_prefer_strict_case_prefix() {
        let resource_a = file("app/constants/color.js");
        let resource_b = file("app/components/model/input/Color.js");

        let mut query = "Color".to_string();

        let mut res = sorted(&[&resource_a, &resource_b], &query);
        assert_eq!(res[0], resource_b);
        assert_eq!(res[1], resource_a);

        res = sorted(&[&resource_b, &resource_a], &query);
        assert_eq!(res[0], resource_b);
        assert_eq!(res[1], resource_a);

        query = "color".to_string();

        res = sorted(&[&resource_a, &resource_b], &query);
        assert_eq!(res[0], resource_a);
        assert_eq!(res[1], resource_b);

        res = sorted(&[&resource_b, &resource_a], &query);
        assert_eq!(res[0], resource_a);
        assert_eq!(res[1], resource_b);
}

#[test]
fn comparefilesbyscore_prefer_prefix_bug_103052() {
        let resource_a = file("test/smoke/src/main.ts");
        let resource_b = file("src/vs/editor/common/services/semantikTokensProviderStyling.ts");

        let query = "smoke main.ts".to_string();

        let mut res = sorted(&[&resource_a, &resource_b], &query);
        assert_eq!(res[0], resource_a);
        assert_eq!(res[1], resource_b);

        res = sorted(&[&resource_b, &resource_a], &query);
        assert_eq!(res[0], resource_a);
        assert_eq!(res[1], resource_b);
}

#[test]
fn comparefilesbyscore_boost_better_prefix_match_if_multiple_queries_are_used() {
        let resource_a = file("src/vs/workbench/services/host/browser/browserHostService.ts");
        let resource_b = file("src/vs/workbench/browser/workbench.ts");

        for query in ["workbench.ts browser", "browser workbench.ts", "browser workbench", "workbench browser"] {
            let mut res = sorted(&[&resource_a, &resource_b], &query);
            assert_eq!(res[0], resource_b);
            assert_eq!(res[1], resource_a);

            res = sorted(&[&resource_b, &resource_a], &query);
            assert_eq!(res[0], resource_b);
            assert_eq!(res[1], resource_a);
        }
}

#[test]
fn comparefilesbyscore_boost_shorter_prefix_match_if_multiple_queries_are_used() {
        let resource_a = file("src/vs/workbench/node/actions/windowActions.ts");
        let resource_b = file("src/vs/workbench/electron-node/window.ts");

        for query in ["window node", "window.ts node"] {
            let mut res = sorted(&[&resource_a, &resource_b], &query);
            assert_eq!(res[0], resource_b);
            assert_eq!(res[1], resource_a);

            res = sorted(&[&resource_b, &resource_a], &query);
            assert_eq!(res[0], resource_b);
            assert_eq!(res[1], resource_a);
        }
}

#[test]
fn comparefilesbyscore_skip_preference_on_label_match_when_using_path_sep() {
        let resource_a = file("djangosite/ufrela/def.py");
        let resource_b = file("djangosite/urls/default.py");

        let query = "url/def".to_string();

        let mut res = sorted(&[&resource_a, &resource_b], &query);
        assert_eq!(res[0], resource_b);
        assert_eq!(res[1], resource_a);

        res = sorted(&[&resource_b, &resource_a], &query);
        assert_eq!(res[0], resource_b);
        assert_eq!(res[1], resource_a);
}

#[test]
fn comparefilesbyscore_boost_shorter_prefix_match_if_multiple_queries_are_used_99171() {
        let resource_a = file("mesh_editor_lifetime_job.h");
        let resource_b = file("lifetime_job.h");

        let query = "m life, life m".to_string();

        let mut res = sorted(&[&resource_a, &resource_b], &query);
        assert_eq!(res[0], resource_b);
        assert_eq!(res[1], resource_a);

        res = sorted(&[&resource_b, &resource_a], &query);
        assert_eq!(res[0], resource_b);
        assert_eq!(res[1], resource_a);
}

#[test]
fn comparefilesbyscore_boost_consecutive_matches_in_the_beginning_over_end() {
        let resource_a = file("src/vs/server/node/extensionHostStatusService.ts");
        let resource_b = file("src/vs/workbench/browser/parts/notifications/notificationsStatus.ts");

        let query = "notStatus".to_string();

        let mut res = sorted(&[&resource_a, &resource_b], &query);
        assert_eq!(res[0], resource_b);
        assert_eq!(res[1], resource_a);

        res = sorted(&[&resource_b, &resource_a], &query);
        assert_eq!(res[0], resource_b);
        assert_eq!(res[1], resource_a);
}

#[test]
fn preparequery() {
        assert_eq!(prepare_query(" f*a ").piece.normalized, "fa");
        assert_eq!(prepare_query(" f…a ").piece.normalized, "fa");
        assert_eq!(prepare_query("main#").piece.normalized, "main");
        assert_eq!(prepare_query("main#").piece.original, "main#");
        assert_eq!(prepare_query("foo*").piece.normalized, "foo");
        assert_eq!(prepare_query("foo*").piece.original, "foo*");
        assert_eq!(prepare_query("model Tester.ts").piece.original, "model Tester.ts");
        assert_eq!(prepare_query("model Tester.ts").piece.original_lowercase, "model Tester.ts".to_lowercase());
        assert_eq!(prepare_query("model Tester.ts").piece.normalized, "modelTester.ts");
        assert_eq!(prepare_query("model Tester.ts").piece.expect_contiguous_match, false); // doesn't have quotes in it
        assert_eq!(prepare_query("Model Tester.ts").piece.normalized_lowercase, "modeltester.ts");
        assert_eq!(prepare_query("ModelTester.ts").contains_path_separator, false);
        assert_eq!(prepare_query("Model/Tester.ts").contains_path_separator, true);
        assert_eq!(prepare_query("\"hello\"").piece.expect_contiguous_match, true);
        assert_eq!(prepare_query("\"hello\"").piece.normalized, "hello");

        // with spaces
        let mut query = prepare_query("He*llo World");
        assert_eq!(query.piece.original, "He*llo World");
        assert_eq!(query.piece.normalized, "HelloWorld");
        assert_eq!(query.piece.normalized_lowercase, "HelloWorld".to_lowercase());
        assert_eq!(values(&query).len(), 2);
        assert_eq!(values(&query)[0].original, "He*llo");
        assert_eq!(values(&query)[0].normalized, "Hello");
        assert_eq!(values(&query)[0].normalized_lowercase, "Hello".to_lowercase());
        assert_eq!(values(&query)[1].original, "World");
        assert_eq!(values(&query)[1].normalized, "World");
        assert_eq!(values(&query)[1].normalized_lowercase, "World".to_lowercase());

        let restored_query = piece_to_query(values(&query));
        assert_eq!(restored_query.piece.original, query.piece.original);
        assert_eq!(values(&restored_query).len(), values(&query).len());
        assert_eq!(restored_query.contains_path_separator, query.contains_path_separator);

        // with spaces that are empty
        query = prepare_query(" Hello   World      ");
        assert_eq!(query.piece.original, " Hello   World      ");
        assert_eq!(query.piece.original_lowercase, " Hello   World      ".to_lowercase());
        assert_eq!(query.piece.normalized, "HelloWorld");
        assert_eq!(query.piece.normalized_lowercase, "HelloWorld".to_lowercase());
        assert_eq!(values(&query).len(), 2);
        assert_eq!(values(&query)[0].original, "Hello");
        assert_eq!(values(&query)[0].original_lowercase, "Hello".to_lowercase());
        assert_eq!(values(&query)[0].normalized, "Hello");
        assert_eq!(values(&query)[0].normalized_lowercase, "Hello".to_lowercase());
        assert_eq!(values(&query)[1].original, "World");
        assert_eq!(values(&query)[1].original_lowercase, "World".to_lowercase());
        assert_eq!(values(&query)[1].normalized, "World");
        assert_eq!(values(&query)[1].normalized_lowercase, "World".to_lowercase());

        // Path related
        assert_eq!(prepare_query("/some/path").piece.path_normalized, "/some/path");
        assert_eq!(prepare_query("/some/path").piece.normalized, "/some/path");
        assert_eq!(prepare_query("/some/path").contains_path_separator, true);
        assert_eq!(prepare_query("\\some\\path").piece.path_normalized, "/some/path");
        assert_eq!(prepare_query("\\some\\path").piece.normalized, "/some/path");
        assert_eq!(prepare_query("\\some\\path").contains_path_separator, true);
}

#[test]
fn fuzzyscore2_matching() {
        let target = "HelLo-World";

        for offset in [0, 3] {
            let target_with_offset = if offset == 0 { target.to_owned() } else { format!("123{target}") };
            let (score, matches) = do_score2(&target_with_offset, "HelLo-World", offset);

            assert!(score.is_some());
            assert_eq!(matches.len(), 1);
            assert_eq!(matches[0].start, 0 + offset);
            assert_eq!(matches[0].end, target.len() + offset);

            let (score, matches) = do_score2(&target_with_offset, "HW", offset);

            assert!(score.is_some());
            assert_eq!(matches.len(), 2);
            assert_eq!(matches[0].start, 0 + offset);
            assert_eq!(matches[0].end, 1 + offset);
            assert_eq!(matches[1].start, 6 + offset);
            assert_eq!(matches[1].end, 7 + offset);
        }
}

#[test]
fn fuzzyscore2_multiple_queries() {
        let target = "HelLo-World";

        let (first_single_score, first_single_matches) = do_score2(target, "HelLo", 0);
        let (second_single_score, second_single_matches) = do_score2(target, "World", 0);
        let first_and_second_single_matches: Vec<_> = first_single_matches.iter().chain(&second_single_matches).cloned().collect();

        let assert_score = |(multi_score, multi_matches): FuzzyScore2| {
            assert!(multi_score.unwrap_or(0) >= first_single_score.unwrap_or(0) + second_single_score.unwrap_or(0));
            for (i, multi_match) in multi_matches.iter().enumerate() {
                let first_and_second_single_match = first_and_second_single_matches.get(i).expect("a single match for each multi match");
                assert_eq!(multi_match.start, first_and_second_single_match.start);
                assert_eq!(multi_match.end, first_and_second_single_match.end);
            }
        };

        let assert_no_score = |(multi_score, multi_matches): FuzzyScore2| {
            assert_eq!(multi_score, None);
            assert_eq!(multi_matches.len(), 0);
        };

        assert_score(do_score2(target, "HelLo World", 0));
        assert_score(do_score2(target, "World HelLo", 0));
        assert_score(do_score2(target, "World HelLo World", 0));
        assert_no_score(do_score2(target, "World HelLo Nothing", 0));
        assert_no_score(do_score2(target, "More Nothing", 0));
}

#[test]
fn fuzzyscore2_95716() {
        let target = "# ❌ Wow";

        let score = do_score2(target, "❌", 0);
        assert!(score.0.is_some());
        assert!(score.1.len() > 0);
}

#[test]
fn using_quotes_should_expect_contiguous_matches_match() {
        // missing the "i" in the query
        assert_eq!(do_score("contiguous", "\"contguous\"", None).0, 0);

        let score = do_score("contiguous", "\"contiguous\"", None);
        assert!(score.0 > 0);
}

#[test]
fn using_quotes_should_highlight_contiguous_indexes() {
        let score = do_score("2021-7-26.md", "\"26\"", None);
        assert_eq!(score.0, 14);

        // The indexes of the 2 and 6 of "26"
        assert_eq!(score.1[0], 7);
        assert_eq!(score.1[1], 8);
}

#[test]
fn workspace_symbol_search_with_special_characters() {
        // Simulates the scenario from the issue where rust-analyzer uses # and * as query modifiers
        // The original query (with special chars) should reach the language server
        // but normalized query (without special chars) should be used for fuzzy matching
        let cases = [
            // Test #: User types "main#", language server returns "main" symbol
            ("main#", "main", "main"),
            // Test *: User types "foo*", language server returns "foo" symbol
            ("foo*", "foo", "foo"),
            // Test both: User types "MyClass#*", should match "MyClass"
            ("MyClass#*", "MyClass", "MyClass"),
            // Test fuzzy matching still works: User types "MC#", should match "MyClass"
            ("MC#", "MC", "MyClass"),
            // Make sure leading # or # in the middle are not removed.
            ("#SpecialFunction", "#SpecialFunction", "#SpecialFunction"),
            // Make sure standalone # is not removed
            ("#", "#", "#"),
        ];
        for (original, normalized, symbol) in cases {
            let query = prepare_query(original);
            assert_eq!(query.piece.original, original); // Sent to language server
            assert_eq!(query.piece.normalized, normalized); // Used for fuzzy matching
            let (score, matches) = do_score2(symbol, original, 0);
            assert!(score.is_some_and(|score| score > 0), "Should match {symbol:?} symbol when query is {original:?}");
            assert!(matches.len() > 0);
        }
}
