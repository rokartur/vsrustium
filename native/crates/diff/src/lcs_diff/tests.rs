use super::{DiffChange, lcs_diff};

fn codes(text: &str) -> Vec<i32> {
    text.chars().map(|c| c as i32).collect()
}

fn mask_based_substring(text: &[char], mask: &[bool]) -> String {
    text.iter().zip(mask).filter(|(_, keep)| **keep).map(|(c, _)| *c).collect()
}

/// `assertAnswer`.
fn assert_answer(original: &str, modified: &str, changes: &[DiffChange], answer: &str, only_length: bool) {
    let original: Vec<char> = original.chars().collect();
    let modified: Vec<char> = modified.chars().collect();
    let mut original_mask = vec![true; original.len()];
    let mut modified_mask = vec![true; modified.len()];
    for change in changes {
        original_mask[change.original_start..change.original_start + change.original_length].fill(false);
        modified_mask[change.modified_start..change.modified_start + change.modified_length].fill(false);
    }
    let original_answer = mask_based_substring(&original, &original_mask);
    let modified_answer = mask_based_substring(&modified, &modified_mask);
    if only_length {
        assert_eq!(original_answer.chars().count(), answer.chars().count());
        assert_eq!(modified_answer.chars().count(), answer.chars().count());
    } else {
        assert_eq!(original_answer, answer);
        assert_eq!(modified_answer, answer);
    }
}

fn lcs_inner_test(original: &str, modified: &str, answer: &str, only_length: bool) {
    let changes = lcs_diff(&codes(original), &codes(modified), false);
    assert_answer(original, modified, &changes, answer, only_length);
}

/// `stringPower`: doubles the string `power` times.
fn string_power(text: &str, power: usize) -> String {
    let mut result = text.to_string();
    for _ in 0..power {
        result = result.repeat(2);
    }
    result
}

fn lcs_test(original: &str, modified: &str, answer: &str) {
    lcs_inner_test(original, modified, answer, false);
    for power in 2..=5 {
        lcs_inner_test(&string_power(original, power), &string_power(modified, power), &string_power(answer, power), true);
    }
}

fn change(original_start: usize, original_length: usize, modified_start: usize, modified_length: usize) -> DiffChange {
    DiffChange { original_start, original_length, modified_start, modified_length }
}

#[test]
fn lcs_diff_different_strings() {
    lcs_test("heLLo world", "hello orlando", "heo orld");
    lcs_test("abcde", "acd", "acd");
    lcs_test("abcdbce", "bcede", "bcde");
    lcs_test("abcdefgabcdefg", "bcehafg", "bceafg");
    lcs_test("abcde", "fgh", "");
    lcs_test("abcfabc", "fabc", "fabc");
    lcs_test("0azby0", "9axbzby9", "azby");
    lcs_test("0abc00000", "9a1b2c399999", "abc");

    lcs_test("fooBar", "myfooBar", "fooBar");
    lcs_test("fooBar", "fooMyBar", "fooBar");
    lcs_test("fooBar", "fooBar", "fooBar");
}

/// The `stringDiff` surrogate pair cases, over code points instead of UTF-16 units.
#[test]
fn code_points_are_single_elements() {
    for pretty in [false, true] {
        assert_eq!(lcs_diff(&codes("𫟹"), &codes("𫔍"), pretty), vec![change(0, 1, 0, 1)]);
        assert_eq!(lcs_diff(&codes("a😀b"), &codes("a😃b"), pretty), vec![change(1, 1, 1, 1)]);
        assert_eq!(lcs_diff(&codes("😀a"), &codes("😀b"), pretty), vec![change(1, 1, 1, 1)]);
    }
    assert_eq!(lcs_diff(&codes("a"), &codes("a😀"), false), vec![change(1, 0, 1, 1)]);
    assert_eq!(lcs_diff(&codes("😀a"), &codes("a"), false), vec![change(0, 1, 0, 0)]);
}

#[test]
fn empty_sequences() {
    assert_eq!(lcs_diff(&[], &[], false), vec![]);
    assert_eq!(lcs_diff(&[], &[1, 2], true), vec![change(0, 0, 0, 2)]);
    assert_eq!(lcs_diff(&[1, 2], &[], true), vec![change(0, 2, 0, 0)]);
}

#[test]
fn pretty_moves_an_insertion_to_the_sequence_start() {
    assert_eq!(lcs_diff(&codes("a"), &codes("aa"), false), vec![change(1, 0, 1, 1)]);
    assert_eq!(lcs_diff(&codes("a"), &codes("aa"), true), vec![change(0, 0, 0, 1)]);
}

/// `(original, modified, changes with pretty false, changes with pretty true)` as upstream `LcsDiff` computes them.
type Changes = &'static [(usize, usize, usize, usize)];

const UPSTREAM_CASES: &[(&str, &str, Changes, Changes)] = &[
    ("aaaba", "", &[(0, 5, 0, 0)], &[(0, 5, 0, 0)]),
    ("", "acababc", &[(0, 0, 0, 7)], &[(0, 0, 0, 7)]),
    ("bbbaba", "bb\na\na\n\nabba", &[(2, 0, 2, 7), (3, 1, 10, 0)], &[(2, 0, 2, 7), (3, 1, 10, 0)]),
    ("ad", "c", &[(0, 2, 0, 1)], &[(0, 2, 0, 1)]),
    ("\n\nb\n\n\n  ba", "b", &[(0, 2, 0, 0), (3, 7, 1, 0)], &[(0, 2, 0, 0), (3, 7, 1, 0)]),
    ("bb", "bbba", &[(2, 0, 2, 2)], &[(2, 0, 2, 2)]),
    ("ab", "a", &[(1, 1, 1, 0)], &[(1, 1, 1, 0)]),
    ("a\n\na\nbaaba", "", &[(0, 10, 0, 0)], &[(0, 10, 0, 0)]),
    ("adbbcacaca", "bdcaadbdbbac", &[(0, 0, 0, 4), (1, 0, 5, 2), (4, 1, 10, 0), (7, 3, 12, 0)], &[(0, 0, 0, 4), (1, 0, 5, 2), (4, 1, 10, 0), (7, 3, 12, 0)]),
    ("b a\n", "ba\n", &[(1, 1, 1, 0)], &[(1, 1, 1, 0)]),
    ("bbaabb", "b", &[(1, 5, 1, 0)], &[(0, 5, 0, 0)]),
    ("acbabac", "", &[(0, 7, 0, 0)], &[(0, 7, 0, 0)]),
    ("\nb\nab\n\naaa\nb", "a\na", &[(0, 3, 0, 0), (4, 2, 1, 0), (8, 4, 3, 0)], &[(0, 3, 0, 0), (4, 2, 1, 0), (8, 4, 3, 0)]),
    ("ddcdaccdaaa", "bdaabac", &[(0, 7, 0, 1), (10, 0, 4, 1), (11, 0, 6, 1)], &[(0, 7, 0, 1), (10, 0, 4, 1), (11, 0, 6, 1)]),
    ("\n\nab\n\n\n\n", "b\n \n\na a  \nb", &[(0, 3, 0, 0), (5, 0, 2, 1), (7, 0, 5, 5), (8, 0, 11, 1)], &[(0, 3, 0, 0), (5, 0, 2, 1), (7, 0, 5, 5), (8, 0, 11, 1)]),
    ("baabaabaa", "b", &[(1, 8, 1, 0)], &[(1, 8, 1, 0)]),
    ("c", "bcc", &[(0, 0, 0, 2)], &[(0, 0, 0, 2)]),
    ("b\n\n\nbaa\n\n", "bb\n\na\n", &[(1, 0, 1, 1), (3, 3, 4, 0), (7, 1, 5, 0)], &[(0, 0, 0, 1), (3, 3, 4, 0), (8, 1, 6, 0)]),
    ("", "ddaabdabdac", &[(0, 0, 0, 11)], &[(0, 0, 0, 11)]),
    ("b\na", "b", &[(1, 2, 1, 0)], &[(1, 2, 1, 0)]),
    ("bababaaabb", "", &[(0, 10, 0, 0)], &[(0, 10, 0, 0)]),
    ("bbaaa", "bccbaba", &[(1, 0, 1, 2), (3, 1, 5, 1)], &[(1, 0, 1, 2), (3, 1, 5, 1)]),
    ("aab\n\naaabaa", "\naabb\n\na\nbb", &[(0, 0, 0, 1), (3, 0, 4, 1), (6, 2, 8, 2), (9, 2, 11, 0)], &[(0, 0, 0, 1), (3, 0, 4, 1), (6, 2, 8, 2), (9, 2, 11, 0)]),
    ("", "a", &[(0, 0, 0, 1)], &[(0, 0, 0, 1)]),
    ("bb\nb", " \n", &[(0, 2, 0, 1), (3, 1, 2, 0)], &[(0, 2, 0, 1), (3, 1, 2, 0)]),
    ("ababbbbbbbab", "bbbaba", &[(0, 3, 0, 0), (6, 0, 3, 1), (7, 3, 5, 0), (11, 1, 6, 0)], &[(0, 3, 0, 0), (6, 3, 3, 1), (11, 1, 6, 0)]),
    ("bacca", "cbbab", &[(0, 0, 0, 1), (1, 0, 2, 1), (2, 3, 4, 1)], &[(0, 0, 0, 2), (2, 3, 4, 1)]),
    ("\nb", "\n\naba", &[(1, 0, 1, 2), (2, 0, 4, 1)], &[(1, 0, 1, 2), (2, 0, 4, 1)]),
    ("bad", "aaacb", &[(0, 0, 0, 4), (1, 2, 5, 0)], &[(0, 0, 0, 4), (1, 2, 5, 0)]),
    (" \n\nab\nbba", " ", &[(1, 8, 1, 0)], &[(1, 8, 1, 0)]),
];

fn tuples(changes: &[DiffChange]) -> Vec<(usize, usize, usize, usize)> {
    changes.iter().map(|c| (c.original_start, c.original_length, c.modified_start, c.modified_length)).collect()
}

#[test]
fn matches_upstream_tie_breaking() {
    for (original, modified, plain, pretty) in UPSTREAM_CASES {
        assert_eq!(tuples(&lcs_diff(&codes(original), &codes(modified), false)), *plain, "{original:?} {modified:?}");
        assert_eq!(tuples(&lcs_diff(&codes(original), &codes(modified), true)), *pretty, "{original:?} {modified:?} pretty");
    }
}

/// xorshift32, the same generator the upstream reference script ran.
struct Random(u32);

impl Random {
    fn below(&mut self, n: u32) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0 % n
    }

    fn text(&mut self, alphabet: &str, max_len: u32) -> String {
        let alphabet: Vec<char> = alphabet.chars().collect();
        let len = self.below(max_len + 1);
        (0..len).map(|_| alphabet[self.below(alphabet.len() as u32) as usize]).collect()
    }
}

/// FNV-1a over 500 random pairs up to 120 elements; 1391210316 is what upstream `LcsDiff` gives for the same pairs.
#[test]
fn matches_upstream_on_random_sequences() {
    let alphabets = ["ab", "abc", "ab\n", "abcd", "a b\n"];
    let mut random = Random(2463534242);
    for (k, (original, modified, _, _)) in UPSTREAM_CASES.iter().enumerate() {
        let alphabet = alphabets[k % alphabets.len()];
        assert_eq!((random.text(alphabet, 12), random.text(alphabet, 12)), (original.to_string(), modified.to_string()));
    }
    let mut hash: u32 = 2166136261;
    let mut mix = |value: usize| hash = (hash ^ value as u32).wrapping_mul(16777619);
    for k in 0..500 {
        let alphabet = alphabets[k % alphabets.len()];
        let (original, modified) = (random.text(alphabet, 120), random.text(alphabet, 120));
        for pretty in [false, true] {
            let changes = lcs_diff(&codes(&original), &codes(&modified), pretty);
            mix(changes.len());
            for c in changes {
                mix(c.original_start);
                mix(c.original_length);
                mix(c.modified_start);
                mix(c.modified_length);
            }
        }
    }
    assert_eq!(hash, 1391210316);
}
