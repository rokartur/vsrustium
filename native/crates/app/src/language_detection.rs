//! languageDetectionWebWorker.ts `detectLanguagesImpl`: the guesslang model's results kept while their adjusted
//! confidence stays close to the best one. The model runs in Bun (`workbench/language_detection.rs`).
// The regexp model (`vscode-regexp-languagedetection`) is not a dependency of 1.147.0, so upstream's historical
// resolver never loads; the port has none either.

/// `expectedRelativeConfidence`.
const EXPECTED_RELATIVE_CONFIDENCE: f64 = 0.2;

/// `adjustLanguageConfidence`: languages common in VS Code up, languages with simple syntaxes (#131912) down.
fn adjusted(language: &str, confidence: f64) -> f64 {
    match language {
        "js" | "html" | "json" | "ts" | "css" | "py" | "xml" | "php" => confidence + 0.05,
        "cpp" | "sh" | "java" | "cs" | "c" => confidence + 0.025,
        "bat" | "ini" | "makefile" | "sql" | "csv" | "toml" => confidence - 0.5,
        _ => confidence,
    }
}

/// The model language ids `detectLanguagesImpl` yields, best first, for results sorted by confidence.
pub fn detected_languages(results: &[(String, f64)]) -> Vec<String> {
    let mut yielded = Vec::new();
    let Some((first, confidence)) = results.first() else { return yielded };
    if *confidence < EXPECTED_RELATIVE_CONFIDENCE {
        return yielded;
    }
    let first_confidence = adjusted(first, *confidence);
    if first_confidence < EXPECTED_RELATIVE_CONFIDENCE {
        return yielded;
    }
    let mut possible = vec![(first.clone(), first_confidence)];
    for (language, confidence) in &results[1..] {
        let confidence = adjusted(language, *confidence);
        let highest = possible.last().expect("possible keeps one").1;
        if highest - confidence >= EXPECTED_RELATIVE_CONFIDENCE {
            yielded.extend(possible.drain(..).map(|(language, _)| language));
        }
        if confidence <= EXPECTED_RELATIVE_CONFIDENCE {
            return yielded;
        }
        possible.push((language.clone(), confidence));
    }
    yielded
}

/// `getTextForDetection`: the text up to offset 10000 (`positionAt(10000)`, the EOL counted as one char each).
pub fn text_for_detection(lines: &[String]) -> String {
    let text = lines.join("\n");
    match text.char_indices().nth(10000) {
        Some((byte, _)) => text[..byte].to_owned(),
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn results(pairs: &[(&str, f64)]) -> Vec<(String, f64)> {
        pairs.iter().map(|(language, confidence)| (language.to_string(), *confidence)).collect()
    }

    #[test]
    fn a_clear_winner_is_yielded_once_the_next_drops_by_the_expected_confidence() {
        assert_eq!(detected_languages(&results(&[("py", 0.23), ("hs", 0.046), ("groovy", 0.04)])), ["py"]);
        assert_eq!(detected_languages(&results(&[("ts", 0.48), ("rs", 0.1)])), ["ts"]);
    }

    #[test]
    fn weak_or_penalized_results_yield_nothing() {
        assert!(detected_languages(&results(&[("rs", 0.19), ("c", 0.1)])).is_empty());
        assert!(detected_languages(&results(&[("sql", 0.6), ("c", 0.1)])).is_empty());
        // Without a drop of 0.2 the generator ends before it yields.
        assert!(detected_languages(&results(&[("go", 0.4), ("rs", 0.3), ("c", 0.1)])).is_empty());
    }

    #[test]
    fn close_results_are_all_yielded_when_a_later_one_drops() {
        assert_eq!(detected_languages(&results(&[("go", 0.5), ("rs", 0.45), ("c", 0.15), ("lua", 0.01)])), ["go", "rs"]);
        // `c` gets 0.025, so it stays above 0.2 and is a candidate after `go` and `rs` were yielded.
        assert_eq!(detected_languages(&results(&[("go", 0.5), ("rs", 0.45), ("c", 0.21), ("lua", 0.01)])), ["go", "rs", "c"]);
    }

    #[test]
    fn detection_reads_the_first_10000_chars() {
        let lines = vec!["a".repeat(9999), "b".repeat(10)];
        assert_eq!(text_for_detection(&lines).chars().count(), 10000);
        assert!(text_for_detection(&lines).ends_with('\n'));
    }
}
