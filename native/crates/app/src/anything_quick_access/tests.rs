use super::*;
use crate::fuzzy_scorer::prepare_query;

fn range(start_line_number: usize, start_column: usize, end_line_number: usize, end_column: usize) -> TextRange {
    TextRange { start_line_number, start_column, end_line_number, end_column }
}

// extractRange.test.ts
#[test]
fn extract_range_basics() {
    assert_eq!(extract_range_from_filter("", &[]), None);
    assert_eq!(extract_range_from_filter("/some/path", &[]), None);
    assert_eq!(extract_range_from_filter("/some/path/file.txt", &[]), None);
    let base = "/some/path/file.txt";
    for line_sep in [":", "#", "(", ":line "] {
        for col_sep in [":", "#", ","] {
            let (filter, range) = extract_range_from_filter(&format!("{base}{line_sep}20"), &[]).unwrap();
            assert_eq!((filter.as_str(), range.start_line_number, range.start_column), (base, 20, 1));
            let (filter, range) = extract_range_from_filter(&format!("{base}{line_sep}20{col_sep}"), &[]).unwrap();
            assert_eq!((filter.as_str(), range.start_line_number, range.start_column), (base, 20, 1));
            let (filter, range) = extract_range_from_filter(&format!("{base}{line_sep}20{col_sep}3"), &[]).unwrap();
            assert_eq!((filter.as_str(), range.start_line_number, range.start_column), (base, 20, 3));
        }
    }
}

#[test]
fn extract_range_allows_space_after_path() {
    let (filter, range) = extract_range_from_filter("/some/path/file.txt (19,20)", &[]).unwrap();
    assert_eq!((filter.as_str(), range.start_line_number, range.start_column), ("/some/path/file.txt", 19, 20));
}

#[test]
fn extract_range_ranges() {
    let base = "/some/path/file.txt";
    let specs = [
        (format!("{base}:20-40"), range(20, 1, 40, 1)),
        (format!("{base}:20:3-40:5"), range(20, 3, 40, 5)),
        (format!("{base}:20:3-40"), range(20, 3, 40, 1)),
        (format!("{base}#20#3-40,5"), range(20, 3, 40, 5)),
        (format!("{base}(20,3-40,5)"), range(20, 3, 40, 5)),
        (format!("{base}:20-"), range(20, 1, 20, 1)),
    ];
    for (filter, expected) in specs {
        assert_eq!(extract_range_from_filter(&filter, &[]), Some((base.to_owned(), expected)), "{filter}");
    }
    assert_eq!(extract_range_from_filter("/some/path/my-file.txt", &[]), None);
    assert_eq!(extract_range_from_filter("/some/path/file-2.txt", &[]), None);
}

#[test]
fn extract_range_unless() {
    assert_eq!(extract_range_from_filter("/some/path/file.txt@alphasymbol", &["@"]), None);
    assert_eq!(extract_range_from_filter("@/some/path/file.txt (19,20)", &["@"]), None);
    assert_eq!(extract_range_from_filter("/some/path/file.txt (19,20)@", &["@"]), None);
    assert_eq!(extract_range_from_filter("/some/@path/file.txt (19,20)", &["@"]), Some(("/some/@path/file.txt".into(), range(19, 20, 19, 20))));
    assert_eq!(extract_range_from_filter("/some/@path/file.txt:19:20", &["@"]), Some(("/some/@path/file.txt".into(), range(19, 20, 19, 20))));
    assert_eq!(extract_range_from_filter("/some/@path/file.txt#19", &["@"]), Some(("/some/@path/file.txt".into(), range(19, 1, 19, 1))));
}

#[test]
fn a_trailing_separator_opens_the_file_at_its_start() {
    assert_eq!(extract_range_from_filter("main.ts:", &[]), Some(("main.ts".into(), range(1, 1, 1, 1))));
}

fn entries(root: &str, relative: &[&str]) -> Vec<FileEntry> {
    relative.iter().map(|path| FileEntry { relative_path: path.to_string(), path: Path::new(root).join(path) }).collect()
}

#[test]
fn file_results_put_label_matches_first() {
    let files = entries("/w", &["README.md", "docs/guide.md", "notes.txt", "package.json", "src/main.ts", "src/util.ts"]);
    let query = prepare_query("main");
    let results = file_search_results(&files, &query, &mut FuzzyScorerCache::new());
    assert_eq!(results, [PathBuf::from("/w/src/main.ts")]);
    let query = prepare_query("ts");
    let results = file_search_results(&files, &query, &mut FuzzyScorerCache::new());
    // Equal label scores fall back to `compareAnything`, which sorts the labels.
    assert_eq!(results, [PathBuf::from("/w/src/main.ts"), PathBuf::from("/w/src/util.ts"), PathBuf::from("/w/notes.txt")]);
}

#[test]
fn picks_show_the_folder_relative_to_the_workspace_and_highlight_the_match() {
    let workspace = Some(Path::new("/w"));
    let picks = vec![FilePick::new("/w/src/main.ts".into(), workspace), FilePick::new("/w/README.md".into(), workspace)];
    assert_eq!((picks[0].label.as_str(), picks[0].description.as_str()), ("main.ts", "src"));
    assert_eq!(picks[1].description, "");
    let picks = additional_picks(picks, &prepare_query("srcmain"), &mut FuzzyScorerCache::new());
    assert_eq!(picks.len(), 1);
    assert_eq!(picks[0].label_highlights, vec![0..4]);
    assert_eq!(picks[0].description_highlights, vec![0..3]);
}

#[test]
fn history_matches_labels_unless_the_query_has_a_path_separator() {
    let history = [PathBuf::from("/w/src/util.ts"), PathBuf::from("/w/docs/guide.md")];
    let workspace = Some(Path::new("/w"));
    let mut cache = FuzzyScorerCache::new();
    assert_eq!(editor_history_picks(&history, &prepare_query(""), workspace, true, false, &mut cache).len(), 2);
    assert!(editor_history_picks(&history, &prepare_query("src"), workspace, true, false, &mut cache).is_empty());
    let picks = editor_history_picks(&history, &prepare_query("src/ut"), workspace, true, false, &mut cache);
    assert_eq!(picks.iter().map(|pick| pick.label.as_str()).collect::<Vec<_>>(), ["util.ts"]);
    assert!(editor_history_picks(&history, &prepare_query("u"), workspace, false, false, &mut cache).is_empty());
    let picks = editor_history_picks(&history, &prepare_query("u"), workspace, true, false, &mut cache);
    assert_eq!(picks.iter().map(|pick| pick.label.as_str()).collect::<Vec<_>>(), ["util.ts", "guide.md"]);
}

#[test]
fn untildify_replaces_only_a_leading_home() {
    assert_eq!(untildify("~/a.txt", "/Users/me"), "/Users/me/a.txt");
    assert_eq!(untildify("~", "/Users/me"), "/Users/me");
    assert_eq!(untildify("~x/a", "/Users/me"), "~x/a");
    assert_eq!(untildify("/a/~", "/Users/me"), "/a/~");
}
