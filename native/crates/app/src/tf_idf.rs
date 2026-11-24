//! Port of src/vs/base/common/tfIdf.ts: term frequency, inverse document frequency scores of documents made of
//! text chunks against a query, which the Command Palette uses to suggest "similar commands".

use std::collections::HashMap;

pub struct Document {
    pub key: String,
    pub text_chunks: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Score {
    pub key: String,
    pub score: f64,
}

#[derive(Default)]
pub struct TfIdfCalculator {
    chunk_count: usize,
    /// For each term, the number of chunks it occurs in.
    chunk_occurrences: HashMap<String, usize>,
    /// In insertion order, as upstream's `Map`, which decides the order of equal scores.
    documents: Vec<(String, Vec<HashMap<String, usize>>)>,
}

impl TfIdfCalculator {
    /// One score per chunk that shares a term with the query.
    pub fn calculate_scores(&self, query: &str) -> Vec<Score> {
        let embedding = self.compute_tfidf(&term_frequencies(query));
        let mut idf_cache = HashMap::new();
        let mut scores = Vec::new();
        for (key, chunks) in &self.documents {
            for chunk in chunks {
                let score = self.similarity_score(chunk, &embedding, &mut idf_cache);
                if score > 0.0 {
                    scores.push(Score { key: key.clone(), score });
                }
            }
        }
        scores
    }

    pub fn update_documents(&mut self, documents: Vec<Document>) -> &mut Self {
        for document in &documents {
            self.delete_document(&document.key);
        }
        for document in documents {
            let mut chunks = Vec::new();
            for text in &document.text_chunks {
                let tf: HashMap<String, usize> = term_frequencies(text).into_iter().collect();
                for term in tf.keys() {
                    *self.chunk_occurrences.entry(term.clone()).or_default() += 1;
                }
                chunks.push(tf);
            }
            self.chunk_count += chunks.len();
            self.documents.push((document.key, chunks));
        }
        self
    }

    pub fn delete_document(&mut self, key: &str) {
        let Some(index) = self.documents.iter().position(|(document_key, _)| document_key == key) else { return };
        let (_, chunks) = self.documents.remove(index);
        self.chunk_count -= chunks.len();
        for chunk in &chunks {
            for term in chunk.keys() {
                if let Some(occurrences) = self.chunk_occurrences.get_mut(term) {
                    *occurrences -= 1;
                    if *occurrences == 0 {
                        self.chunk_occurrences.remove(term);
                    }
                }
            }
        }
    }

    fn similarity_score(&self, chunk: &HashMap<String, usize>, query_embedding: &[(String, f64)], idf_cache: &mut HashMap<String, f64>) -> f64 {
        let mut sum = 0.0;
        for (term, term_tfidf) in query_embedding {
            let Some(chunk_tf) = chunk.get(term) else { continue };
            let chunk_idf = *idf_cache.entry(term.clone()).or_insert_with(|| self.compute_idf(term));
            sum += *chunk_tf as f64 * chunk_idf * term_tfidf;
        }
        sum
    }

    fn compute_idf(&self, term: &str) -> f64 {
        match self.chunk_occurrences.get(term) {
            Some(&occurrences) if occurrences > 0 => ((self.chunk_count + 1) as f64 / occurrences as f64).ln(),
            _ => 0.0,
        }
    }

    fn compute_tfidf(&self, term_frequencies: &[(String, usize)]) -> Vec<(String, f64)> {
        let mut embedding = Vec::new();
        for (word, occurrences) in term_frequencies {
            let idf = self.compute_idf(word);
            if idf > 0.0 {
                embedding.push((word.clone(), *occurrences as f64 * idf));
            }
        }
        embedding
    }
}

/// Sorts the scores from best to worst and divides them by the best.
pub fn normalize_scores(mut scores: Vec<Score>) -> Vec<Score> {
    scores.sort_by(|a, b| b.score.total_cmp(&a.score));
    let max = scores.first().map_or(0.0, |score| score.score);
    if max > 0.0 {
        for score in &mut scores {
            score.score /= max;
        }
    }
    scores
}

/// The terms in order of first occurrence with their counts.
fn term_frequencies(input: &str) -> Vec<(String, usize)> {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for term in split_terms(input) {
        match counts.iter_mut().find(|(known, _)| *known == term) {
            Some((_, count)) => *count += 1,
            None => counts.push((term, 1)),
        }
    }
    counts
}

/// The lowercase matches of `/\b\p{Letter}[\p{Letter}\d]{2,}\b/gu`, each followed by its camel case parts of
/// three or more letters when it has more than one.
fn split_terms(input: &str) -> Vec<String> {
    let mut terms = Vec::new();
    for word in words(input) {
        terms.push(word.to_lowercase());
        let parts = split_camel_case(&word);
        if parts.len() > 1 {
            for part in parts {
                if part.encode_utf16().count() > 2 && has_three_letters_in_a_row(&part) {
                    terms.push(part.to_lowercase());
                }
            }
        }
    }
    terms
}

/// The regex walk: `\b` and `\d` are ASCII-only in JavaScript even with the `u` flag.
/// shortcut: `\p{Letter}` is `char::is_alphabetic`, which also takes letter numbers and some marks; std has no Letter class.
fn words(input: &str) -> Vec<String> {
    let chars: Vec<char> = input.chars().collect();
    let is_word_char = |index: usize| chars.get(index).is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_');
    let is_boundary = |index: usize| (index > 0 && is_word_char(index - 1)) != is_word_char(index);
    let mut words = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        if !(chars[start].is_alphabetic() && is_boundary(start)) {
            start += 1;
            continue;
        }
        let run = chars[start + 1..].iter().take_while(|c| c.is_alphabetic() || c.is_ascii_digit()).count();
        // Greedy, then backtracking to the longest run of at least two that ends on a boundary.
        let Some(end) = (2..=run).rev().map(|length| start + 1 + length).find(|end| is_boundary(*end)) else {
            start += 1;
            continue;
        };
        words.push(chars[start..end].iter().collect());
        start = end;
    }
    words
}

/// `word.replace(/([a-z])([A-Z])/g, '$1 $2').split(/\s+/g)`.
fn split_camel_case(word: &str) -> Vec<String> {
    let mut parts = vec![String::new()];
    let mut previous = None;
    for c in word.chars() {
        if previous.is_some_and(|p: char| p.is_ascii_lowercase()) && c.is_ascii_uppercase() {
            parts.push(String::new());
        }
        parts.last_mut().expect("starts with one part").push(c);
        previous = Some(c);
    }
    parts
}

fn has_three_letters_in_a_row(part: &str) -> bool {
    let mut run = 0;
    for c in part.chars() {
        run = if c.is_alphabetic() { run + 1 } else { 0 };
        if run >= 3 {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests;
