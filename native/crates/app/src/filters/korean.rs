//! `getKoreanAltChars` of base/common/naturalLanguage/korean.ts: the Latin keys a 2-set Korean keyboard
//! types for a Hangul jamo or syllable, so `matchesWords` finds "files" from "랴ㅣㄷㄴ".

use super::korean_tables::{COMPATIBILITY_JAMO, MODERN_CONSONANTS, MODERN_FINAL_CONSONANTS, MODERN_VOWELS};

pub fn korean_alt_chars(code: char) -> Option<String> {
    let code = code as u32;
    for (table, start) in [(&MODERN_CONSONANTS[..], 0x1100), (&MODERN_VOWELS[..], 0x1161), (&MODERN_FINAL_CONSONANTS[..], 0x11A8), (&COMPATIBILITY_JAMO[..], 0x3131)] {
        if let Some(keys) = code.checked_sub(start).and_then(|index| table.get(index as usize)) {
            return (!keys.is_empty()).then(|| keys.to_string());
        }
    }
    if !(0xAC00..=0xD7A3).contains(&code) {
        return None;
    }
    // A precomposed syllable: initial consonant, vowel, then the final consonant, where 0 stands for none.
    let index = code - 0xAC00;
    let vowel_and_final = index % 588;
    let mut keys = String::new();
    keys.push_str(MODERN_CONSONANTS[(index / 588) as usize]);
    keys.push_str(MODERN_VOWELS[(vowel_and_final / 28) as usize]);
    if let Some(final_index) = (vowel_and_final % 28).checked_sub(1) {
        keys.push_str(MODERN_FINAL_CONSONANTS[final_index as usize]);
    }
    Some(keys)
}
