//! explorerFileNestingTrie.test.ts.

use super::*;

const ATTRIBUTES: FilenameAttributes = FilenameAttributes { dirname: "mydir", basename: "", extname: "" };

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| item.to_string()).collect()
}

fn trie(config: &[(&str, &[&str])]) -> FileNestingTrie {
    let config: Vec<(String, Vec<String>)> = config.iter().map(|(parent, children)| (parent.to_string(), strings(children))).collect();
    FileNestingTrie::new(&config)
}

#[track_caller]
fn assert_nesting(actual: HashMap<String, Vec<String>>, expected: &[(&str, &[&str])]) {
    let as_sets = |entries: Vec<(String, Vec<String>)>| -> HashMap<String, HashSet<String>> { entries.into_iter().map(|(parent, children)| (parent, children.into_iter().collect())).collect() };
    let expected = expected.iter().map(|(parent, children)| (parent.to_string(), strings(children))).collect();
    assert_eq!(as_sets(actual.into_iter().collect()), as_sets(expected));
}

#[test]
fn suf_trie_exact_matches() {
    let mut t = SufTrie::default();
    t.add(".npmrc", "MyKey");
    assert_eq!(t.get(".npmrc", &ATTRIBUTES), ["MyKey"]);
    assert!(t.get(".npmrcs", &ATTRIBUTES).is_empty());
    assert!(t.get("a.npmrc", &ATTRIBUTES).is_empty());
}

#[test]
fn suf_trie_star_matches() {
    let mut t = SufTrie::default();
    t.add("*.npmrc", "MyKey");
    assert_eq!(t.get(".npmrc", &ATTRIBUTES), ["MyKey"]);
    assert!(t.get("npmrc", &ATTRIBUTES).is_empty());
    assert!(t.get(".npmrcs", &ATTRIBUTES).is_empty());
    assert_eq!(t.get("a.npmrc", &ATTRIBUTES), ["MyKey"]);
    assert_eq!(t.get("a.b.c.d.npmrc", &ATTRIBUTES), ["MyKey"]);
}

#[test]
fn suf_trie_star_substitutes() {
    let mut t = SufTrie::default();
    t.add("*.npmrc", "${capture}.json");
    assert_eq!(t.get(".npmrc", &ATTRIBUTES), [".json"]);
    assert!(t.get("npmrc", &ATTRIBUTES).is_empty());
    assert!(t.get(".npmrcs", &ATTRIBUTES).is_empty());
    assert_eq!(t.get("a.npmrc", &ATTRIBUTES), ["a.json"]);
    assert_eq!(t.get("a.b.c.d.npmrc", &ATTRIBUTES), ["a.b.c.d.json"]);
}

#[test]
fn suf_trie_multi_matches() {
    let mut t = SufTrie::default();
    t.add("*.npmrc", "Key1");
    t.add("*.json", "Key2");
    t.add("*d.npmrc", "Key3");
    assert_eq!(t.get(".npmrc", &ATTRIBUTES), ["Key1"]);
    assert!(t.get("npmrc", &ATTRIBUTES).is_empty());
    assert!(t.get(".npmrcs", &ATTRIBUTES).is_empty());
    assert_eq!(t.get(".json", &ATTRIBUTES), ["Key2"]);
    assert_eq!(t.get("a.json", &ATTRIBUTES), ["Key2"]);
    assert_eq!(t.get("a.npmrc", &ATTRIBUTES), ["Key1"]);
    assert_eq!(t.get("a.b.c.d.npmrc", &ATTRIBUTES), ["Key1", "Key3"]);
}

#[test]
fn suf_trie_multi_substitutes() {
    let mut t = SufTrie::default();
    t.add("*.npmrc", "Key1.${capture}.js");
    t.add("*.json", "Key2.${capture}.js");
    t.add("*d.npmrc", "Key3.${capture}.js");
    assert_eq!(t.get(".npmrc", &ATTRIBUTES), ["Key1..js"]);
    assert!(t.get("npmrc", &ATTRIBUTES).is_empty());
    assert!(t.get(".npmrcs", &ATTRIBUTES).is_empty());
    assert_eq!(t.get(".json", &ATTRIBUTES), ["Key2..js"]);
    assert_eq!(t.get("a.json", &ATTRIBUTES), ["Key2.a.js"]);
    assert_eq!(t.get("a.npmrc", &ATTRIBUTES), ["Key1.a.js"]);
    assert_eq!(t.get("a.b.cd.npmrc", &ATTRIBUTES), ["Key1.a.b.cd.js", "Key3.a.b.c.js"]);
    assert_eq!(t.get("a.b.c.d.npmrc", &ATTRIBUTES), ["Key1.a.b.c.d.js", "Key3.a.b.c..js"]);
}

#[test]
fn pre_trie_exact_matches() {
    let mut t = PreTrie::default();
    t.add(".npmrc", "MyKey");
    assert_eq!(t.get(".npmrc", &ATTRIBUTES), ["MyKey"]);
    assert!(t.get(".npmrcs", &ATTRIBUTES).is_empty());
    assert!(t.get("a.npmrc", &ATTRIBUTES).is_empty());
}

#[test]
fn pre_trie_star_matches() {
    let mut t = PreTrie::default();
    t.add("*.npmrc", "MyKey");
    assert_eq!(t.get(".npmrc", &ATTRIBUTES), ["MyKey"]);
    assert!(t.get("npmrc", &ATTRIBUTES).is_empty());
    assert!(t.get(".npmrcs", &ATTRIBUTES).is_empty());
    assert_eq!(t.get("a.npmrc", &ATTRIBUTES), ["MyKey"]);
    assert_eq!(t.get("a.b.c.d.npmrc", &ATTRIBUTES), ["MyKey"]);
}

#[test]
fn pre_trie_star_substitutes() {
    let mut t = PreTrie::default();
    t.add("*.npmrc", "${capture}.json");
    assert_eq!(t.get(".npmrc", &ATTRIBUTES), [".json"]);
    assert!(t.get("npmrc", &ATTRIBUTES).is_empty());
    assert!(t.get(".npmrcs", &ATTRIBUTES).is_empty());
    assert_eq!(t.get("a.npmrc", &ATTRIBUTES), ["a.json"]);
    assert_eq!(t.get("a.b.c.d.npmrc", &ATTRIBUTES), ["a.b.c.d.json"]);
}

#[test]
fn pre_trie_multi_matches() {
    let mut t = PreTrie::default();
    t.add("*.npmrc", "Key1");
    t.add("*.json", "Key2");
    t.add("*d.npmrc", "Key3");
    assert_eq!(t.get(".npmrc", &ATTRIBUTES), ["Key1"]);
    assert!(t.get("npmrc", &ATTRIBUTES).is_empty());
    assert!(t.get(".npmrcs", &ATTRIBUTES).is_empty());
    assert_eq!(t.get(".json", &ATTRIBUTES), ["Key2"]);
    assert_eq!(t.get("a.json", &ATTRIBUTES), ["Key2"]);
    assert_eq!(t.get("a.npmrc", &ATTRIBUTES), ["Key1"]);
    assert_eq!(t.get("a.b.c.d.npmrc", &ATTRIBUTES), ["Key1", "Key3"]);
}

#[test]
fn pre_trie_multi_substitutes() {
    let mut t = PreTrie::default();
    t.add("*.npmrc", "Key1.${capture}.js");
    t.add("*.json", "Key2.${capture}.js");
    t.add("*d.npmrc", "Key3.${capture}.js");
    assert_eq!(t.get(".npmrc", &ATTRIBUTES), ["Key1..js"]);
    assert!(t.get("npmrc", &ATTRIBUTES).is_empty());
    assert!(t.get(".npmrcs", &ATTRIBUTES).is_empty());
    assert_eq!(t.get(".json", &ATTRIBUTES), ["Key2..js"]);
    assert_eq!(t.get("a.json", &ATTRIBUTES), ["Key2.a.js"]);
    assert_eq!(t.get("a.npmrc", &ATTRIBUTES), ["Key1.a.js"]);
    assert_eq!(t.get("a.b.cd.npmrc", &ATTRIBUTES), ["Key1.a.b.cd.js", "Key3.a.b.c.js"]);
    assert_eq!(t.get("a.b.c.d.npmrc", &ATTRIBUTES), ["Key1.a.b.c.d.js", "Key3.a.b.c..js"]);
}

#[test]
fn pre_trie_empty_matches() {
    let mut t = PreTrie::default();
    t.add("package*json", "package");
    assert_eq!(t.get("package.json", &ATTRIBUTES), ["package"]);
    assert_eq!(t.get("packagejson", &ATTRIBUTES), ["package"]);
    assert_eq!(t.get("package-lock.json", &ATTRIBUTES), ["package"]);
}

#[test]
fn does_added_extension_nesting() {
    let t = trie(&[("*", &["${capture}.*"])]);
    let files = strings(&["file", "file.json", "boop.test", "boop.test1", "boop.test.1", "beep", "beep.test1", "beep.boop.test1", "beep.boop.test2", "beep.boop.a"]);
    assert_nesting(
        t.nest(&files, "mydir"),
        &[("file", &["file.json"]), ("boop.test", &["boop.test.1"]), ("boop.test1", &[]), ("beep", &["beep.test1", "beep.boop.test1", "beep.boop.test2", "beep.boop.a"])],
    );
}

#[test]
fn does_ext_specific_nesting() {
    let t = trie(&[("*.ts", &["${capture}.js"]), ("*.js", &["${capture}.map"])]);
    let files = strings(&["a.ts", "a.js", "a.jss", "ab.js", "b.js", "b.map", "c.ts", "c.js", "c.map", "d.ts", "d.map"]);
    assert_nesting(
        t.nest(&files, "mydir"),
        &[("a.ts", &["a.js"]), ("ab.js", &[]), ("a.jss", &[]), ("b.js", &["b.map"]), ("c.ts", &["c.js", "c.map"]), ("d.ts", &[]), ("d.map", &[])],
    );
}

#[test]
fn handles_loops() {
    let t = trie(&[
        ("*.a", &["${capture}.b", "${capture}.c"]),
        ("*.b", &["${capture}.a"]),
        ("*.c", &["${capture}.d"]),
        ("*.aa", &["${capture}.bb"]),
        ("*.bb", &["${capture}.cc", "${capture}.dd"]),
        ("*.cc", &["${capture}.aa"]),
        ("*.dd", &["${capture}.ee"]),
    ]);
    let files = strings(&[
        ".a", ".b", ".c", ".d", "a.a", "a.b", "a.d", "a.aa", "a.bb", "a.cc", "b.aa", "b.bb", "c.bb", "c.cc", "d.aa", "d.cc", "e.aa", "e.bb", "e.dd", "e.ee", "f.aa", "f.bb", "f.cc", "f.dd", "f.ee",
    ]);
    assert_nesting(
        t.nest(&files, "mydir"),
        &[
            (".a", &[]),
            (".b", &[]),
            (".c", &[]),
            (".d", &[]),
            ("a.a", &[]),
            ("a.b", &[]),
            ("a.d", &[]),
            ("a.aa", &[]),
            ("a.bb", &[]),
            ("a.cc", &[]),
            ("b.aa", &["b.bb"]),
            ("c.bb", &["c.cc"]),
            ("d.cc", &["d.aa"]),
            ("e.aa", &["e.bb", "e.dd", "e.ee"]),
            ("f.aa", &[]),
            ("f.bb", &[]),
            ("f.cc", &[]),
            ("f.dd", &[]),
            ("f.ee", &[]),
        ],
    );
}

#[test]
fn does_general_bidirectional_suffix_matching() {
    let t = trie(&[("*-vsdoc.js", &["${capture}.js"]), ("*.js", &["${capture}-vscdoc.js"])]);
    let files = strings(&["a-vsdoc.js", "a.js", "b.js", "b-vscdoc.js"]);
    assert_nesting(t.nest(&files, "mydir"), &[("a-vsdoc.js", &["a.js"]), ("b.js", &["b-vscdoc.js"])]);
}

#[test]
fn does_general_bidirectional_prefix_matching() {
    let t = trie(&[("vsdoc-*.js", &["${capture}.js"]), ("*.js", &["vscdoc-${capture}.js"])]);
    let files = strings(&["vsdoc-a.js", "a.js", "b.js", "vscdoc-b.js"]);
    assert_nesting(t.nest(&files, "mydir"), &[("vsdoc-a.js", &["a.js"]), ("b.js", &["vscdoc-b.js"])]);
}

#[test]
fn does_general_bidirectional_general_matching() {
    let t = trie(&[("foo-*-bar.js", &["${capture}.js"]), ("*.js", &["bib-${capture}-bap.js"])]);
    let files = strings(&["foo-a-bar.js", "a.js", "b.js", "bib-b-bap.js"]);
    assert_nesting(t.nest(&files, "mydir"), &[("foo-a-bar.js", &["a.js"]), ("b.js", &["bib-b-bap.js"])]);
}

#[test]
fn does_extension_specific_path_segment_matching() {
    let t = trie(&[("*.js", &["${capture}.*.js"])]);
    let files = strings(&["foo.js", "foo.test.js", "fooTest.js", "bar.js.js"]);
    assert_nesting(t.nest(&files, "mydir"), &[("foo.js", &["foo.test.js"]), ("fooTest.js", &[]), ("bar.js.js", &[])]);
}

#[test]
fn does_exact_match_nesting() {
    let t = trie(&[
        ("package.json", &[".npmrc", "npm-shrinkwrap.json", "yarn.lock", ".yarnclean", ".yarnignore", ".yarn-integrity", ".yarnrc"]),
        ("bower.json", &[".bowerrc"]),
    ]);
    let files = strings(&["package.json", ".npmrc", "npm-shrinkwrap.json", "yarn.lock", ".bowerrc"]);
    assert_nesting(t.nest(&files, "mydir"), &[("package.json", &[".npmrc", "npm-shrinkwrap.json", "yarn.lock"]), (".bowerrc", &[])]);
}

#[test]
fn eslint_test() {
    let t = trie(&[(".eslintrc*", &[".eslint*"])]);
    assert_nesting(t.nest(&strings(&[".eslintrc.json", ".eslintignore"]), "mydir"), &[(".eslintrc.json", &[".eslintignore"])]);
    assert_nesting(t.nest(&strings(&[".eslintrc", ".eslintignore"]), "mydir"), &[(".eslintrc", &[".eslintignore"])]);
}

#[test]
fn basename_expansion() {
    let t = trie(&[("*-vsdoc.js", &["${basename}.doc"])]);
    let files = strings(&["boop-vsdoc.js", "boop-vsdoc.doc", "boop.doc"]);
    assert_nesting(t.nest(&files, "mydir"), &[("boop-vsdoc.js", &["boop-vsdoc.doc"]), ("boop.doc", &[])]);
}

#[test]
fn extname_expansion() {
    let t = trie(&[("*-vsdoc.js", &["${extname}.doc"])]);
    let files = strings(&["boop-vsdoc.js", "js.doc", "boop.doc"]);
    assert_nesting(t.nest(&files, "mydir"), &[("boop-vsdoc.js", &["js.doc"]), ("boop.doc", &[])]);
}

#[test]
fn added_segment_matcher() {
    let files = strings(&["some.file", "some.html.file", "some.html.nested.file", "other.file", "some.thing", "some.thing.else"]);
    let expected: &[(&str, &[&str])] = &[("some.file", &["some.html.file", "some.html.nested.file"]), ("other.file", &[]), ("some.thing", &[]), ("some.thing.else", &[])];
    assert_nesting(trie(&[("*", &["${basename}.*.${extname}"])]).nest(&files, "mydir"), expected);
    // The old format.
    assert_nesting(trie(&[("*", &["$(basename).*.$(extname)"])]).nest(&files, "mydir"), expected);
}

#[test]
fn multiple_child_patterns_matching_the_parent_itself() {
    let t = trie(&[("*", &["${basename}.*", "${basename}*.${extname}"])]);
    let files = strings(&["Button.tsx", "Button.test.tsx", "Button.module.css", "ButtonGroup.tsx", "other.ts"]);
    assert_nesting(t.nest(&files, "mydir"), &[("Button.tsx", &["Button.test.tsx", "Button.module.css", "ButtonGroup.tsx"]), ("other.ts", &[])]);
}

#[test]
fn dirname_matching() {
    let t = trie(&[("index.ts", &["${dirname}.ts"])]);
    let files = strings(&["otherFile.ts", "MyComponent.ts", "index.ts"]);
    assert_nesting(t.nest(&files, "MyComponent"), &[("index.ts", &["MyComponent.ts"]), ("otherFile.ts", &[])]);
}
