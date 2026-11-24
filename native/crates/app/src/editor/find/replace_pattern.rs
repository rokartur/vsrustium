//! replacePattern.ts: the replace string of the find widget with `$n` groups, `\n` `\t` escapes and the
//! `\u` `\U` `\l` `\L` case operations, and `buildReplaceStringWithCasePreserved` (base/common/search.ts).

#[derive(Clone, Debug, PartialEq)]
pub struct ReplacePattern {
    pieces: Vec<ReplacePiece>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ReplacePiece {
    Static(String),
    /// A capture group with the case operations before it.
    Group(usize, Vec<char>),
}

impl ReplacePattern {
    pub fn from_static(value: &str) -> Self {
        Self::new(vec![ReplacePiece::Static(value.to_string())])
    }

    fn new(pieces: Vec<ReplacePiece>) -> Self {
        match pieces.as_slice() {
            [] => ReplacePattern { pieces: vec![ReplacePiece::Static(String::new())] },
            _ => ReplacePattern { pieces },
        }
    }

    /// `hasReplacementPatterns`: the value depends on the match.
    pub fn has_replacement_patterns(&self) -> bool {
        !matches!(self.pieces.as_slice(), [ReplacePiece::Static(_)])
    }

    /// The replacement for a match, `matches` its groups with group 0 first and `""` for an unmatched one.
    pub fn build(&self, matches: Option<&[String]>, preserve_case: bool) -> String {
        if let [ReplacePiece::Static(value)] = self.pieces.as_slice() {
            return match preserve_case {
                true => replace_with_case_preserved(matches, value),
                false => value.clone(),
            };
        }
        let mut result = String::new();
        for piece in &self.pieces {
            let (index, case_ops) = match piece {
                ReplacePiece::Static(value) => {
                    result.push_str(value);
                    continue;
                }
                ReplacePiece::Group(index, case_ops) => (*index, case_ops),
            };
            let group = substitute(index, matches);
            let mut op = 0;
            for (position, c) in group.chars().enumerate() {
                let Some(&case_op) = case_ops.get(op) else {
                    result.extend(group.chars().skip(position));
                    break;
                };
                match case_op {
                    'U' => result.extend(c.to_uppercase()),
                    'u' => {
                        result.extend(c.to_uppercase());
                        op += 1;
                    }
                    'L' => result.extend(c.to_lowercase()),
                    _ => {
                        result.extend(c.to_lowercase());
                        op += 1;
                    }
                }
            }
        }
        result
    }
}

/// `_substitute`: `$nn` is a group only if the search has it, else `$n` followed by the digit.
fn substitute(mut index: usize, matches: Option<&[String]>) -> String {
    let Some(matches) = matches else { return String::new() };
    if index == 0 {
        return matches[0].clone();
    }
    let mut remainder = String::new();
    while index > 0 {
        if index < matches.len() {
            return format!("{}{remainder}", matches[index]);
        }
        remainder = format!("{}{remainder}", index % 10);
        index /= 10;
    }
    format!("${remainder}")
}

/// `parseReplaceString`.
pub fn parse_replace_string(replace: &str) -> ReplacePattern {
    let chars: Vec<char> = replace.chars().collect();
    let mut pieces = Vec::new();
    let mut current = String::new();
    let mut case_ops = Vec::new();
    let flush = |current: &mut String, pieces: &mut Vec<ReplacePiece>| {
        if !current.is_empty() {
            pieces.push(ReplacePiece::Static(std::mem::take(current)));
        }
    };
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        match (c, next) {
            ('\\', Some('\\')) => current.push('\\'),
            ('\\', Some('n')) => current.push('\n'),
            ('\\', Some('t')) => current.push('\t'),
            ('\\', Some(op @ ('u' | 'U' | 'l' | 'L'))) => case_ops.push(op),
            ('$', Some('$')) => current.push('$'),
            ('$', Some('0' | '&')) => {
                flush(&mut current, &mut pieces);
                pieces.push(ReplacePiece::Group(0, std::mem::take(&mut case_ops)));
            }
            ('$', Some(digit @ '1'..='9')) => {
                let mut index = digit as usize - '0' as usize;
                if let Some(second) = chars.get(i + 2).filter(|c| c.is_ascii_digit()) {
                    index = index * 10 + (*second as usize - '0' as usize);
                    i += 1;
                }
                flush(&mut current, &mut pieces);
                pieces.push(ReplacePiece::Group(index, std::mem::take(&mut case_ops)));
            }
            ('\\' | '$', Some(other)) => {
                current.push(c);
                current.push(other);
            }
            _ => {
                current.push(c);
                i += 1;
                continue;
            }
        }
        i += 2;
    }
    flush(&mut current, &mut pieces);
    ReplacePattern::new(pieces)
}

/// `buildReplaceStringWithCasePreserved`.
pub fn replace_with_case_preserved(matches: Option<&[String]>, pattern: &str) -> String {
    let Some(matched) = matches.and_then(|matches| matches.first()).filter(|matched| !matched.is_empty()) else { return pattern.to_string() };
    let hyphens = same_pieces(matched, pattern, '-');
    let underscores = same_pieces(matched, pattern, '_');
    if hyphens != underscores {
        let separator = if hyphens { '-' } else { '_' };
        let parts: Vec<String> = pattern.split(separator).zip(matched.split(separator)).map(|(part, matched)| replace_with_case_preserved(Some(&[matched.to_string()]), part)).collect();
        return parts.join(&separator.to_string());
    }
    let first = matched.chars().next().expect("not empty");
    let mut rest = pattern.chars();
    if matched.to_uppercase() == *matched {
        pattern.to_uppercase()
    } else if matched.to_lowercase() == *matched {
        pattern.to_lowercase()
    } else if first.to_lowercase().ne(std::iter::once(first)) && !pattern.is_empty() {
        rest.next().into_iter().flat_map(char::to_uppercase).chain(rest).collect()
    } else if first.to_uppercase().ne(std::iter::once(first)) && !pattern.is_empty() {
        rest.next().into_iter().flat_map(char::to_lowercase).chain(rest).collect()
    } else {
        pattern.to_string()
    }
}

/// `validateSpecificSpecialCharacter`: both hold `separator` and split into as many pieces.
fn same_pieces(matched: &str, pattern: &str, separator: char) -> bool {
    matched.contains(separator) && pattern.contains(separator) && matched.split(separator).count() == pattern.split(separator).count()
}

#[cfg(test)]
mod tests;
