//! The provider's reply as the editor applies it: `decodeSemanticTokensDto`
//! (editor/common/services/semanticTokensDto.ts), the delta branch of `ModelSemanticColoring._setDocumentSemanticTokens`
//! (editor/contrib/semanticTokens/browser/documentSemanticTokens.ts), and `SemanticTokensProviderStyling` with
//! `toMultilineTokens2` (editor/common/services/semanticTokensProviderStyling.ts).

use std::collections::HashMap;

use super::TokenStyle;

#[derive(Clone, Debug, PartialEq)]
pub enum SemanticTokensDto {
    Full { id: u32, data: Vec<u32> },
    Delta { id: u32, deltas: Vec<SemanticTokensEdit> },
}

/// `SemanticTokensEdit`: replaces `delete_count` u32s of the previous data at `start` with `data`.
#[derive(Clone, Debug, PartialEq)]
pub struct SemanticTokensEdit {
    pub start: u32,
    pub delete_count: u32,
    pub data: Option<Vec<u32>>,
}

/// `decodeSemanticTokensDto`: little endian u32s `id, type (1 full, 2 delta), ...`.
/// A reply shorter than its lengths state, or not a whole number of u32s, is `None` where upstream reads
/// `undefined` past the end.
pub fn decode_semantic_tokens_dto(bytes: &[u8]) -> Option<SemanticTokensDto> {
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let src: Vec<u32> = bytes.chunks_exact(4).map(|chunk| u32::from_le_bytes(chunk.try_into().unwrap())).collect();
    let mut reader = src.iter().copied();
    let mut take = |length: usize| -> Option<Vec<u32>> {
        let values: Vec<u32> = reader.by_ref().take(length).collect();
        (values.len() == length).then_some(values)
    };
    let header = take(3)?;
    let (id, kind, count) = (header[0], header[1], header[2] as usize);
    if kind == 1 {
        return Some(SemanticTokensDto::Full { id, data: take(count)? });
    }
    let mut deltas = Vec::with_capacity(count.min(src.len()));
    for _ in 0..count {
        let edit = take(3)?;
        let data = if edit[2] > 0 { Some(take(edit[2] as usize)?) } else { None };
        deltas.push(SemanticTokensEdit { start: edit[0], delete_count: edit[1], data });
    }
    Some(SemanticTokensDto::Delta { id, deltas })
}

/// Why `apply_semantic_tokens_edits` rejected the edits; upstream then drops the tokens and asks for a full result.
#[derive(Debug, PartialEq)]
pub enum InvalidSemanticTokensEdit {
    /// `warnInvalidEditDeleteCount`
    DeleteCount { previous_length: usize, delta_length: i64 },
    /// `warnInvalidEditStart`
    Start { edit_index: usize, start: u32, previous_length: usize },
}

/// The `isSemanticTokensEdits` branch of `_setDocumentSemanticTokens`: the edits applied back to front on the
/// previous result's data.
pub fn apply_semantic_tokens_edits(previous: &[u32], edits: &[SemanticTokensEdit]) -> Result<Vec<u32>, InvalidSemanticTokensEdit> {
    if edits.is_empty() {
        return Ok(previous.to_vec());
    }
    let delta_length: i64 = edits.iter().map(|edit| edit.data.as_ref().map_or(0, Vec::len) as i64 - edit.delete_count as i64).sum();
    let dest_length = previous.len() as i64 + delta_length;
    if dest_length < 0 {
        return Err(InvalidSemanticTokensEdit::DeleteCount { previous_length: previous.len(), delta_length });
    }
    let mut dest = vec![0; dest_length as usize];
    let mut src_last_start = previous.len() as i64;
    let mut dest_last_start = dest.len() as i64;
    for (edit_index, edit) in edits.iter().enumerate().rev() {
        if edit.start as usize > previous.len() {
            return Err(InvalidSemanticTokensEdit::Start { edit_index, start: edit.start, previous_length: previous.len() });
        }
        let copy_count = src_last_start - (edit.start as i64 + edit.delete_count as i64);
        if copy_count > 0 {
            copy(previous, src_last_start - copy_count, &mut dest, dest_last_start - copy_count, copy_count);
            dest_last_start -= copy_count;
        }
        if let Some(data) = &edit.data {
            copy(data, 0, &mut dest, dest_last_start - data.len() as i64, data.len() as i64);
            dest_last_start -= data.len() as i64;
        }
        src_last_start = edit.start as i64;
    }
    if src_last_start > 0 {
        copy(previous, 0, &mut dest, 0, src_last_start);
    }
    Ok(dest)
}

/// `ModelSemanticColoring._copy`: clamped to both arrays; a negative destination index is dropped, as a typed
/// array ignores it.
fn copy(src: &[u32], src_offset: i64, dest: &mut [u32], dest_offset: i64, length: i64) {
    let length = length.min(dest.len() as i64 - dest_offset).min(src.len() as i64 - src_offset);
    for i in 0..length.max(0) {
        let dest_index = dest_offset + i;
        if dest_index >= 0 {
            dest[dest_index as usize] = src[(src_offset + i) as usize];
        }
    }
}

/// `SemanticTokensLegend`
#[derive(Clone, Debug, Default)]
pub struct SemanticTokensLegend {
    pub token_types: Vec<String>,
    pub token_modifiers: Vec<String>,
}

/// `SemanticTokensProviderStyling`: a provider's legend and the styles already looked up for it.
pub struct SemanticTokensProviderStyling {
    legend: SemanticTokensLegend,
    /// `_hashTable`: `None` is `NO_STYLING`.
    cache: HashMap<(u32, u32, String), Option<TokenStyle>>,
}

impl SemanticTokensProviderStyling {
    pub fn new(legend: SemanticTokensLegend) -> Self {
        SemanticTokensProviderStyling { legend, cache: HashMap::new() }
    }

    /// `SemanticTokensProviderStyling.getMetadata`: the style of a token type index and modifier bitset, looked up
    /// by `style_of(type, modifiers, language)` (`getTokenStyleMetadata`); `None` is `NO_STYLING`: a type outside
    /// the legend, or a style that sets nothing. Modifier bits outside the legend are ignored.
    pub fn get_metadata(&mut self, token_type_index: u32, token_modifier_set: u32, language: &str, style_of: &impl Fn(&str, &[&str], &str) -> TokenStyle) -> Option<TokenStyle> {
        let key = (token_type_index, token_modifier_set, language.to_owned());
        if let Some(style) = self.cache.get(&key) {
            return style.clone();
        }
        let style = self.legend.token_types.get(token_type_index as usize).and_then(|token_type| {
            let mut modifiers = Vec::new();
            let mut modifier_set = token_modifier_set;
            for modifier in &self.legend.token_modifiers {
                if modifier_set == 0 {
                    break;
                }
                if modifier_set & 1 == 1 {
                    modifiers.push(modifier.as_str());
                }
                modifier_set >>= 1;
            }
            Some(style_of(token_type, &modifiers, language)).filter(|style| *style != TokenStyle::default())
        });
        self.cache.insert(key, style.clone());
        style
    }
}

/// A styled UTF-16 column range of one line.
#[derive(Clone, Debug, PartialEq)]
pub struct StyledRange {
    pub start: u32,
    pub end: u32,
    pub style: TokenStyle,
}

/// `SemanticColoringConstants.DesiredTokensPerArea`
const DESIRED_TOKENS_PER_AREA: usize = 400;
/// `SemanticColoringConstants.DesiredMaxAreas`
const DESIRED_MAX_AREAS: usize = 1024;

/// `toMultilineTokens2`: the styled ranges of each of the `line_count` lines (0-based) from the full `data`
/// (5 u32s per token: delta line, delta start, length, type index, modifier bitset).
/// Tokens with an invalid length, overlapping the previous styled token of their line, or without styling are
/// dropped as upstream drops them, in the same int32 arithmetic. Upstream stores the rest in areas that the text
/// model reads by binary search; a token its line order or column order would break there (a line before an
/// earlier token's, a start wrapped past its end) or a line outside the document is dropped here.
pub fn to_multiline_tokens(data: &[u32], styling: &mut SemanticTokensProviderStyling, language: &str, line_count: usize, style_of: &impl Fn(&str, &[&str], &str) -> TokenStyle) -> Vec<Vec<StyledRange>> {
    let token_count = data.len() / 5;
    let tokens_per_area = token_count.div_ceil(DESIRED_MAX_AREAS).max(DESIRED_TOKENS_PER_AREA);
    let mut lines = vec![Vec::new(); line_count];
    let mut last_stored_line = 0;
    let mut token_index = 0;
    let mut last_line_number: i32 = 1;
    // A JS number: a start on a new line is the raw u32, only a start on the same line wraps to int32.
    let mut last_start_character: i64 = 0;
    while token_index < token_count {
        let token_end_index = area_end(data, token_index, token_count, tokens_per_area);
        let mut area_line: i32 = 0;
        let mut area = Vec::new();
        let (mut prev_line_number, mut prev_end_character) = (0, 0);
        while token_index < token_end_index {
            let token = &data[5 * token_index..5 * token_index + 5];
            let delta_line = token[0];
            let line_number = last_line_number.wrapping_add(delta_line as i32);
            let start_character = if delta_line == 0 { to_int32(last_start_character + token[1] as i64) as i64 } else { token[1] as i64 };
            let end_character = to_int32(start_character + token[2] as i64);
            let is_invalid_length = end_character as i64 <= start_character;
            let overlaps_previous = prev_line_number == line_number && prev_end_character as i64 > start_character;
            if !is_invalid_length && !overlaps_previous {
                if let Some(style) = styling.get_metadata(token[3], token[4], language, style_of) {
                    if area_line == 0 {
                        area_line = line_number;
                    }
                    // Stored in a Uint32Array upstream.
                    let delta = (line_number as i64 - area_line as i64) as u32;
                    area.push((delta, start_character as u32, end_character as u32, style));
                    prev_line_number = line_number;
                    prev_end_character = end_character;
                }
            }
            last_line_number = line_number;
            last_start_character = start_character;
            token_index += 1;
        }
        for (delta, start, end, style) in area {
            let Ok(line) = usize::try_from(area_line as i64 + delta as i64 - 1) else { continue };
            if line >= line_count || line < last_stored_line || start >= end || lines[line].last().is_some_and(|previous: &StyledRange| previous.end > start) {
                continue;
            }
            last_stored_line = line;
            lines[line].push(StyledRange { start, end, style });
        }
    }
    lines
}

/// The end of the area that starts at `start`: about `tokens_per_area` tokens, ending at a line break unless one
/// line has more tokens than that.
fn area_end(data: &[u32], start: usize, token_count: usize, tokens_per_area: usize) -> usize {
    let end = (start + tokens_per_area).min(token_count);
    if end == token_count {
        return end;
    }
    let mut small_end = end;
    while small_end - 1 > start && data[5 * small_end] == 0 {
        small_end -= 1;
    }
    if small_end - 1 != start {
        return small_end;
    }
    let mut big_end = end;
    while big_end + 1 < token_count && data[5 * big_end] == 0 {
        big_end += 1;
    }
    big_end
}

/// JS `| 0`.
fn to_int32(value: i64) -> i32 {
    value as u32 as i32
}
