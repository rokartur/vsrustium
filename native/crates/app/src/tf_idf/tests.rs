//! Ported from src/vs/base/test/common/tfIdf.test.ts.

use super::*;

fn make_document(key: &str, chunks: &[&str]) -> Document {
    Document { key: key.to_owned(), text_chunks: chunks.iter().map(|chunk| chunk.to_string()).collect() }
}

fn permutations(documents: &[(&'static str, &'static [&'static str])]) -> Vec<Vec<Document>> {
    if documents.is_empty() {
        return vec![vec![]];
    }
    let mut result = Vec::new();
    for index in 0..documents.len() {
        let mut rest = documents.to_vec();
        let (key, chunks) = rest.remove(index);
        for mut tail in permutations(&rest) {
            tail.insert(0, make_document(key, chunks));
            result.push(tail);
        }
    }
    result
}

fn assert_score_orders_equal(mut scores: Vec<Score>, expected: &[&str]) {
    scores.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.key.cmp(&b.key)));
    let keys: Vec<&str> = scores.iter().map(|score| score.key.as_str()).collect();
    assert_eq!(keys, expected);
}

fn check(documents: &[(&'static str, &'static [&'static str])], query: &str, expected: &[&str]) {
    for documents in permutations(documents) {
        let mut tfidf = TfIdfCalculator::default();
        tfidf.update_documents(documents);
        assert_score_orders_equal(tfidf.calculate_scores(query), expected);
    }
}

#[test]
fn no_scores_without_documents() {
    assert_score_orders_equal(TfIdfCalculator::default().calculate_scores("something"), &[]);
}

#[test]
fn no_scores_for_term_not_in_document() {
    check(&[("A", &["cat dog fish"])], "elepant", &[]);
}

#[test]
fn scores_document_with_exact_match() {
    check(&[("A", &["cat dog cat"]), ("B", &["cat fish"])], "dog", &["A"]);
}

#[test]
fn document_with_more_matches_first() {
    check(&[("/A", &["cat dog cat"]), ("/B", &["cat fish"]), ("/C", &["frog"])], "cat", &["/A", "/B"]);
}

#[test]
fn more_matches_first_when_term_appears_in_all_documents() {
    check(&[("/A", &["cat dog cat cat"]), ("/B", &["cat fish"]), ("/C", &["frog cat cat"])], "cat", &["/A", "/C", "/B"]);
}

#[test]
fn weighs_less_common_term_higher() {
    check(&[("/A", &["cat dog cat"]), ("/B", &["fish"]), ("/C", &["cat cat cat cat"]), ("/D", &["cat fish"])], "cat the dog", &["/A", "/C", "/D"]);
}

#[test]
fn weighs_chunks_with_less_common_terms_higher() {
    let documents: &[(&str, &[&str])] = &[("/A", &["cat dog cat", "fish"]), ("/B", &["cat cat cat cat dog", "dog"])];
    check(documents, "cat", &["/B", "/A"]);
    check(documents, "dog", &["/A", "/B", "/B"]);
    check(documents, "cat the dog", &["/B", "/A", "/B"]);
    check(documents, "lake fish", &["/A"]);
}

#[test]
fn ignores_case_and_punctuation() {
    check(&[("/A", &["Cat doG.cat"]), ("/B", &["cAt fiSH"]), ("/C", &["frOg"])], ". ,CaT!  ", &["/A", "/B"]);
}

#[test]
fn matches_camel_case_words() {
    check(&[("/A", &["catDog cat"]), ("/B", &["fishCatFish"]), ("/C", &["frogcat"])], "catDOG", &["/A", "/B"]);
}

#[test]
fn does_not_match_document_after_delete() {
    let mut tfidf = TfIdfCalculator::default();
    tfidf.update_documents(vec![make_document("/A", &["cat dog cat"]), make_document("/B", &["cat fish"]), make_document("/C", &["frog"])]);
    assert_score_orders_equal(tfidf.calculate_scores("cat"), &["/A", "/B"]);
    tfidf.delete_document("/A");
    assert_score_orders_equal(tfidf.calculate_scores("cat"), &["/B"]);
    tfidf.delete_document("/C");
    assert_score_orders_equal(tfidf.calculate_scores("cat"), &["/B"]);
    tfidf.delete_document("/B");
    assert_score_orders_equal(tfidf.calculate_scores("cat"), &[]);
}

#[test]
fn words_follow_the_javascript_regex() {
    assert_eq!(words("café is a naïve word2 _bad x1y"), ["caf", "naïve", "word2", "x1y"]);
    assert_eq!(split_terms("fishCatFish"), ["fishcatfish", "fish", "cat", "fish"]);
}
