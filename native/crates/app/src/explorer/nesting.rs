//! explorerFileNestingTrie.ts: which files of a folder nest under which, for `explorer.fileNesting.patterns`.

use std::collections::{HashMap, HashSet};

/// The parts of a file name the child patterns' `${basename}`, `${extname}` and `${dirname}` stand for.
pub struct FilenameAttributes<'a> {
    /// `index.test` in `index.test.json`.
    basename: &'a str,
    /// `json` in `index.test.json`.
    extname: &'a str,
    /// `my-folder` in `my-folder/index.test.json`.
    dirname: &'a str,
}

/// `ExplorerFileNestingTrie`: parent patterns with one `*` that child patterns refer to as `${capture}`.
pub struct FileNestingTrie {
    root: PreTrie,
}

impl FileNestingTrie {
    pub fn new(config: &[(String, Vec<String>)]) -> Self {
        let mut root = PreTrie::default();
        for (parent_pattern, child_patterns) in config {
            for child_pattern in child_patterns {
                root.add(parent_pattern, child_pattern);
            }
        }
        Self { root }
    }

    /// The root ancestors among `files`, each with the files nested under it (none for a file nothing nests
    /// under). A file missing from the keys is shown only under its parents.
    pub fn nest(&self, files: &[String], dirname: &str) -> HashMap<String, Vec<String>> {
        let mut parent_finder = PreTrie::default();
        for potential_parent in files {
            for child in self.root.get(potential_parent, &attributes(potential_parent, dirname)) {
                parent_finder.add(&child, potential_parent);
            }
        }

        let mut result: HashMap<String, Vec<String>> = HashMap::new();
        for file in files {
            let mut ancestors = Vec::new();
            find_all_root_ancestors(&parent_finder, file, dirname, &mut HashSet::new(), &mut ancestors);
            if ancestors.is_empty() {
                ancestors.push(file.clone());
            }
            for ancestor in ancestors {
                let nested = result.entry(ancestor.clone()).or_default();
                if *file != ancestor && !nested.contains(file) {
                    nested.push(file.clone());
                }
            }
        }
        result
    }
}

fn find_all_root_ancestors(parent_finder: &PreTrie, file: &str, dirname: &str, seen: &mut HashSet<String>, roots: &mut Vec<String>) {
    if !seen.insert(file.to_string()) {
        return;
    }
    // A file may match its own child patterns (`${basename}.*` and `${basename}*.${extname}`), but is never its own parent.
    let mut ancestors = parent_finder.get(file, &attributes(file, dirname));
    ancestors.retain(|ancestor| ancestor != file);
    if ancestors.is_empty() {
        roots.push(file.to_string());
        return;
    }
    let mut unique = HashSet::new();
    for ancestor in ancestors {
        if unique.insert(ancestor.clone()) {
            find_all_root_ancestors(parent_finder, &ancestor, dirname, seen, roots);
        }
    }
}

fn attributes<'a>(filename: &'a str, dirname: &'a str) -> FilenameAttributes<'a> {
    match filename.rfind('.') {
        Some(dot) if dot >= 1 => FilenameAttributes { basename: &filename[..dot], extname: &filename[dot + 1..], dirname },
        _ => FilenameAttributes { basename: filename, extname: "", dirname },
    }
}

/// Matches a key from its start up to a `*`, then hands the rest to a `SufTrie`.
#[derive(Default)]
pub struct PreTrie {
    value: SufTrie,
    map: HashMap<char, PreTrie>,
}

impl PreTrie {
    pub fn add(&mut self, key: &str, value: &str) {
        let mut chars = key.chars();
        match chars.next() {
            None | Some('*') => self.value.add(key, value),
            Some(head) => self.map.entry(head).or_default().add(chars.as_str(), value),
        }
    }

    pub fn get(&self, key: &str, attributes: &FilenameAttributes) -> Vec<String> {
        let mut results = self.value.get(key, attributes);
        let mut chars = key.chars();
        if let Some(existing) = chars.next().and_then(|head| self.map.get(&head)) {
            results.extend(existing.get(chars.as_str(), attributes));
        }
        results
    }
}

/// Matches a key from its end back to a `*`.
#[derive(Default)]
pub struct SufTrie {
    star: Vec<SubstitutionString>,
    epsilon: Vec<SubstitutionString>,
    map: HashMap<char, SufTrie>,
}

impl SufTrie {
    pub fn add(&mut self, key: &str, value: &str) {
        let mut chars = key.chars();
        match (key, chars.next_back()) {
            ("*", _) => self.star.push(SubstitutionString::new(value)),
            (_, None) => self.epsilon.push(SubstitutionString::new(value)),
            // Upstream throws "Unexpected star in SufTrie key", which fails the whole folder's children.
            // shortcut: a key with a second `*` is dropped instead, the settings schema allows one.
            (_, Some('*')) => eprintln!("explorer.fileNesting: unexpected star in {key}"),
            (_, Some(tail)) => self.map.entry(tail).or_default().add(chars.as_str(), value),
        }
    }

    pub fn get(&self, key: &str, attributes: &FilenameAttributes) -> Vec<String> {
        let mut results = Vec::new();
        if key.is_empty() {
            results.extend(self.epsilon.iter().map(|substitution| substitution.substitute(attributes, "")));
        }
        results.extend(self.star.iter().map(|substitution| substitution.substitute(attributes, key)));
        let mut chars = key.chars();
        if let Some(existing) = chars.next_back().and_then(|tail| self.map.get(&tail)) {
            results.extend(existing.get(chars.as_str(), attributes));
        }
        results
    }
}

enum Token {
    Text(String),
    Capture,
    Basename,
    Dirname,
    Extname,
}

/// A child pattern with its `${capture}`, `${basename}`, `${dirname}` and `${extname}` tokens; `$(name)` is the old form,
/// and the brackets may mix, as the tokenizer `/\$[({](capture|basename|dirname|extname)[)}]/g`.
struct SubstitutionString {
    tokens: Vec<Token>,
}

impl SubstitutionString {
    fn new(pattern: &str) -> Self {
        let mut tokens = Vec::new();
        let mut text_start = 0;
        let mut index = 0;
        while let Some(found) = pattern[index..].find('$') {
            let start = index + found;
            match parse_token(&pattern[start..]) {
                Some((token, length)) => {
                    tokens.push(Token::Text(pattern[text_start..start].to_string()));
                    tokens.push(token);
                    index = start + length;
                    text_start = index;
                }
                None => index = start + 1,
            }
        }
        if text_start != pattern.len() {
            tokens.push(Token::Text(pattern[text_start..].to_string()));
        }
        Self { tokens }
    }

    fn substitute(&self, attributes: &FilenameAttributes, capture: &str) -> String {
        let mut result = String::new();
        for token in &self.tokens {
            result.push_str(match token {
                Token::Text(text) => text,
                Token::Capture => capture,
                Token::Basename => attributes.basename,
                Token::Dirname => attributes.dirname,
                Token::Extname => attributes.extname,
            });
        }
        result
    }
}

/// A token at the start of `text`, which starts with `$`, and its length.
fn parse_token(text: &str) -> Option<(Token, usize)> {
    let rest = text[1..].strip_prefix(['(', '{'])?;
    for (name, token) in [("capture", Token::Capture), ("basename", Token::Basename), ("dirname", Token::Dirname), ("extname", Token::Extname)] {
        if rest.strip_prefix(name).is_some_and(|after| after.starts_with([')', '}'])) {
            return Some((token, name.len() + 3));
        }
    }
    None
}

#[cfg(test)]
mod tests;
