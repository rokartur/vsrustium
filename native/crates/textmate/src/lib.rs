//! TextMate tokenization, ported from vscode-textmate 9.3.2 on Oniguruma, the regex engine
//! vscode-oniguruma runs.

mod grammar;
mod matcher;
mod plist;
mod raw;
mod registry;
mod rule;
mod theme;

pub use grammar::{Grammar, ScopeStack, StandardTokenType, State, Token};
pub use registry::Registry;
pub use theme::{BOLD, FONT_STYLE_NOT_SET, ITALIC, STRIKETHROUGH, Style, Theme, UNDERLINE};
