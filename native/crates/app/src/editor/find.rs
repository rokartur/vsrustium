//! The editor find: search over the model (textModelSearch.ts), the replace string (replacePattern.ts),
//! the find state (findState.ts), the controller (findController.ts) and the matches in an editor (findModel.ts, findDecorations.ts).

pub mod controller;
pub mod model;
pub mod multicursor;
pub mod replace_pattern;
pub mod search;
pub mod state;
