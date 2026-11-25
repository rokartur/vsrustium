use super::*;

fn chars(text: &str) -> Vec<char> {
    text.chars().collect()
}

#[test]
fn grapheme_break_type_reads_the_upstream_table() {
    assert_eq!(grapheme_break_type('\u{BC1}'), SpacingMark);
    assert_eq!(grapheme_break_type('\r'), CR);
    assert_eq!(grapheme_break_type('a'), Other);
}

#[test]
fn is_emoji_imprecise_includes_star() {
    // issue #115221
    assert!(is_emoji_imprecise('⭐'));
}

#[test]
fn char_lengths_keep_emoji_sequences_and_regional_indicator_runs_whole() {
    let family = chars("👨‍👩🏽‍👧‍👦x");
    assert_eq!(next_char_length(&family, 0), 8);
    assert_eq!(prev_char_length(&family, 8), 8);
    // Upstream drops GB12/13 pairing, so three flags' worth of indicators form one cluster.
    let flags = chars("🇵🇱🇩🇪🇫🇷");
    assert_eq!(next_char_length(&flags, 0), 6);
    assert_eq!(prev_char_length(&flags, 6), 6);
    assert_eq!(next_char_length(&chars("\r\n"), 0), 2);
}

#[test]
fn left_delete_offset_removes_the_last_emoji_component() {
    let family = chars("👨‍👩🏽‍👧‍👦");
    assert_eq!(left_delete_offset(8, &family), 6);
    assert_eq!(left_delete_offset(6, &family), 4);
    assert_eq!(left_delete_offset(4, &family), 1);
    assert_eq!(left_delete_offset(1, &family), 0);
    assert_eq!(left_delete_offset(2, &chars("👶🏾")), 0);
    assert_eq!(left_delete_offset(2, &chars("e\u{301}")), 1);
    assert_eq!(left_delete_offset(1, &chars("\u{FE0F}")), 0);
}
