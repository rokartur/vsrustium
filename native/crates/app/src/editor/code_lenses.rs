//! The code lenses a model shows (codelensController.ts `_currentCodeLensModel` and its widgets), drawn by
//! workbench/editor_view/code_lens.rs.

use serde_json::Value;

/// One lens: its provider's handle and the `CodeLensDto` (`cacheId`, `range`, `command`).
#[derive(Clone, Debug, PartialEq)]
pub struct Lens {
    pub handle: u64,
    pub symbol: Value,
}

/// The lenses of a model: the version they were asked for, the lists to release and the groups shown.
#[derive(Clone, Debug, Default)]
pub struct CodeLenses {
    pub version: Option<u32>,
    pub event: u64,
    /// `(handle, cacheId)` of each provider's list, released with `$releaseCodeLenses` when replaced.
    pub lists: Vec<(u64, Value)>,
    /// The view zone and the lenses of each line, 0-based.
    pub groups: Vec<(usize, u32, Vec<Lens>)>,
}
