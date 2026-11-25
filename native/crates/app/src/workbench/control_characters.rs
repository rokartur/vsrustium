//! viewLineRenderer.ts `renderControlCharacters` with viewLines.css `.mtkcontrol`: a C0 control character (not
//! a tab) shows as its Control Pictures symbol, DEL as U+2421, white on dark red. The text itself draws a space
//! there (`expand_tabs`), so the columns stay one per character.

use gpui::{Div, ParentElement, Styled, div, px, rgb};

use super::Workbench;
use crate::editor::visible_column;

impl Workbench {
    pub(super) fn control_character_boxes(&self, text: &str, tab_size: usize, char_width: f32, rendered_len: usize) -> Vec<Div> {
        let mut boxes = Vec::new();
        for (column, c) in text.chars().take(rendered_len).enumerate() {
            let Some(picture) = control_picture(c) else { continue };
            let left = visible_column(text, column, tab_size) as f32 * char_width;
            boxes.push(div().absolute().top_0().h_full().left(px(left)).w(px(char_width)).bg(rgb(0x960000)).text_color(rgb(0xffffff)).whitespace_nowrap().child(picture.to_string()));
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
