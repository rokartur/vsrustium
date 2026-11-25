//! viewLineRenderer.ts `renderControlCharacters` with viewLines.css `.mtkcontrol`: a C0 control character (not
//! a tab) shows as its Control Pictures symbol, DEL as U+2421, a bidi format character as `[U+XXXX]`, white on dark
//! red. The text itself draws spaces there (`expand_tabs`), one column, or 8 for a bidi format character.

use gpui::{Div, ParentElement, Styled, div, px, rgb};

use super::Workbench;
use crate::editor::{char_columns, is_bidi_format_character, visible_column};

impl Workbench {
    pub(super) fn control_character_boxes(&self, text: &str, tab_size: usize, char_width: f32, rendered_len: usize) -> Vec<Div> {
        let mut boxes = Vec::new();
        for (column, c) in text.chars().take(rendered_len).enumerate() {
            let label = match control_picture(c) {
                Some(picture) => picture.to_string(),
                // viewLineRenderer.ts `_renderLine`: a bidi format character is its code point, 8 columns wide.
                None if is_bidi_format_character(c) => format!("[U+{:04X}]", c as u32),
                None => continue,
            };
            let left = visible_column(text, column, tab_size) as f32 * char_width;
            let width = char_columns(c, 0, tab_size) as f32 * char_width;
            boxes.push(div().absolute().top_0().h_full().left(px(left)).w(px(width)).bg(rgb(0x960000)).text_color(rgb(0xffffff)).whitespace_nowrap().child(label));
        }
        boxes
    }
}

/// The Control Pictures symbol of a C0 control character other than a tab, or of DEL.
pub fn control_picture(c: char) -> Option<char> {
    match c as u32 {
        9 => None,
        code @ 0..32 => char::from_u32(0x2400 + code),
        127 => Some('\u{2421}'),
        _ => None,
    }
}

#[test]
fn control_characters_draw_their_picture_over_a_space() {
    assert_eq!(control_picture('\u{0}'), Some('\u{2400}'));
    assert_eq!(control_picture('\u{1b}'), Some('\u{241b}'));
    assert_eq!(control_picture('\u{7f}'), Some('\u{2421}'));
    assert_eq!(control_picture('\t'), None);
    assert_eq!(control_picture('a'), None);
    assert_eq!(crate::editor::expand_tabs("a\u{1}\tb\u{7f}", 4), "a   b ");
}

#[test]
fn bidi_format_characters_take_eight_columns() {
    assert_eq!(crate::editor::expand_tabs("a\u{202E}b", 4), "a        b");
    assert_eq!(visible_column("a\u{202E}b", 2, 4), 9);
    assert_eq!(crate::editor::logical_visible_column("a\u{202E}b", 2, 4), 2);
    assert_eq!(crate::editor::column_at_visible("a\u{202E}b", 9.2, 4), 2);
    assert_eq!(format!("[U+{:04X}]", '\u{61C}' as u32), "[U+061C]");
}
