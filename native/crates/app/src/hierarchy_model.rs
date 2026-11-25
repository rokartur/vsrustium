//! callHierarchy.ts and typeHierarchy.ts with their trees (callHierarchyTree.ts, typeHierarchyTree.ts): the items
//! a provider prepared at a position, and below each one the calls to or from it, or its super- or subtypes, asked
//! when the row is first expanded.

use std::cmp::Ordering;

use serde_json::Value;

use crate::outline_model::Range;
use crate::references_model::uri_key;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HierarchyKind {
    Calls,
    Types,
}

/// `CallHierarchyDirection` (`CallsTo` incoming, `CallsFrom` outgoing) and `TypeHierarchyDirection` (`Supertypes`,
/// `Subtypes`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    CallsTo,
    CallsFrom,
    Supertypes,
    Subtypes,
}

impl Direction {
    /// The stored value of `callHierarchy/defaultDirection` and `typeHierarchy/defaultDirection`.
    pub fn storage_value(self) -> &'static str {
        match self {
            Direction::CallsTo => "incomingCalls",
            Direction::CallsFrom => "outgoingCalls",
            Direction::Supertypes => "supertypes",
            Direction::Subtypes => "subtypes",
        }
    }

    /// `sanitizedDirection`: anything else is incoming calls or subtypes.
    pub fn from_storage(kind: HierarchyKind, value: Option<&str>) -> Self {
        match (kind, value) {
            (HierarchyKind::Calls, Some("outgoingCalls")) => Direction::CallsFrom,
            (HierarchyKind::Calls, _) => Direction::CallsTo,
            (HierarchyKind::Types, Some("supertypes")) => Direction::Supertypes,
            (HierarchyKind::Types, _) => Direction::Subtypes,
        }
    }

    /// The `ExtHostLanguageFeatures` method that resolves an item's children in this direction.
    pub fn method(self) -> &'static str {
        match self {
            Direction::CallsTo => "$provideCallHierarchyIncomingCalls",
            Direction::CallsFrom => "$provideCallHierarchyOutgoingCalls",
            Direction::Supertypes => "$provideTypeHierarchySupertypes",
            Direction::Subtypes => "$provideTypeHierarchySubtypes",
        }
    }

    /// `_updatePreview`'s title.
    pub fn title(self, root: &str) -> String {
        match self {
            Direction::CallsTo => format!("Callers of '{root}'"),
            Direction::CallsFrom => format!("Calls from '{root}'"),
            Direction::Supertypes => format!("Supertypes of '{root}'"),
            Direction::Subtypes => format!("Subtypes of '{root}'"),
        }
    }

    /// `showModel`'s message when the root has no children.
    pub fn empty_message(self, root: &str) -> String {
        match self {
            Direction::CallsTo => format!("No callers of '{root}'"),
            Direction::CallsFrom => format!("No calls from '{root}'"),
            Direction::Supertypes => format!("No supertypes of '{root}'"),
            Direction::Subtypes => format!("No subtypes of '{root}'"),
        }
    }

    /// The preview shows the parent's file for outgoing calls and supertypes, the item's own file otherwise.
    fn previews_parent(self) -> bool {
        matches!(self, Direction::CallsFrom | Direction::Supertypes)
    }
}

/// `CallHierarchyItem` / `TypeHierarchyItem` as the extension host sends it; `dto` goes back to resolve children.
#[derive(Clone, Debug, PartialEq)]
pub struct HierarchyItem {
    pub dto: Value,
    pub name: String,
    pub detail: String,
    pub kind: u64,
    pub deprecated: bool,
    pub uri: String,
    pub resource: Value,
    pub range: Range,
    pub selection_range: Range,
}

impl HierarchyItem {
    pub fn from_dto(dto: &Value) -> Option<Self> {
        Some(HierarchyItem {
            name: dto["name"].as_str()?.to_string(),
            detail: dto["detail"].as_str().unwrap_or_default().to_string(),
            kind: dto["kind"].as_u64().unwrap_or(0),
            deprecated: dto["tags"].as_array().is_some_and(|tags| tags.iter().any(|tag| tag == 1)),
            uri: uri_key(&dto["uri"])?,
            resource: dto["uri"].clone(),
            range: Range::from_dto(&dto["range"])?,
            selection_range: Range::from_dto(&dto["selectionRange"])?,
            dto: dto.clone(),
        })
    }

    pub fn session_id(&self) -> &str {
        self.dto["_sessionId"].as_str().unwrap_or_default()
    }

    pub fn item_id(&self) -> &str {
        self.dto["_itemId"].as_str().unwrap_or_default()
    }
}

/// A tree row: the item, the call ranges for calls (`fromRanges` in `locations_uri`), its parent, and its children
/// once asked for.
#[derive(Clone, Debug)]
pub struct Node {
    pub item: HierarchyItem,
    pub locations: Option<(String, Vec<Range>)>,
    pub parent: Option<usize>,
    pub depth: usize,
    pub children: Option<Vec<usize>>,
    pub expanded: bool,
}

/// The tree of one session in one direction, its nodes in an arena, the roots first.
#[derive(Clone, Debug)]
pub struct HierarchyTree {
    pub direction: Direction,
    pub nodes: Vec<Node>,
    pub roots: Vec<usize>,
}

impl HierarchyTree {
    pub fn new(direction: Direction, roots: Vec<HierarchyItem>) -> Self {
        let nodes: Vec<Node> = roots.into_iter().map(|item| Node { item, locations: None, parent: None, depth: 0, children: None, expanded: false }).collect();
        let roots = (0..nodes.len()).collect();
        HierarchyTree { direction, nodes, roots }
    }

    pub fn root(&self) -> &HierarchyItem {
        &self.nodes[self.roots[0]].item
    }

    /// `DataSource.getChildren` answered: calls become their caller (`from`) or callee (`to`) with the call ranges,
    /// types the type itself; sorted by resource then range (`Call.compare`).
    pub fn set_children(&mut self, parent: usize, answer: &Value) {
        let parent_uri = self.nodes[parent].item.uri.clone();
        let depth = self.nodes[parent].depth + 1;
        let mut children = Vec::new();
        for entry in answer.as_array().into_iter().flatten() {
            let (item, locations) = match self.direction {
                Direction::CallsTo => (&entry["from"], Some(None)),
                Direction::CallsFrom => (&entry["to"], Some(Some(parent_uri.clone()))),
                Direction::Supertypes | Direction::Subtypes => (entry, None),
            };
            let Some(item) = HierarchyItem::from_dto(item) else { continue };
            let locations = locations.map(|uri| {
                let ranges = entry["fromRanges"].as_array().into_iter().flatten().filter_map(Range::from_dto).collect();
                (uri.unwrap_or_else(|| item.uri.clone()), ranges)
            });
            children.push(Node { item, locations, parent: Some(parent), depth, children: None, expanded: false });
        }
        children.sort_by(|a, b| compare_items(&a.item, &b.item));
        let first = self.nodes.len();
        self.nodes.extend(children);
        self.nodes[parent].children = Some((first..self.nodes.len()).collect());
    }

    /// The visible rows: the roots and the children of expanded rows, depth first.
    pub fn rows(&self) -> Vec<usize> {
        let mut rows = Vec::new();
        let mut stack: Vec<usize> = self.roots.iter().rev().copied().collect();
        while let Some(node) = stack.pop() {
            rows.push(node);
            if self.nodes[node].expanded
                && let Some(children) = &self.nodes[node].children
            {
                stack.extend(children.iter().rev());
            }
        }
        rows
    }

    /// `_updatePreview`: the file the preview shows for a row and the ranges it marks there.
    pub fn preview(&self, node: usize) -> (String, Value, Vec<Range>) {
        let node = &self.nodes[node];
        let shown = match (self.direction.previews_parent(), node.parent) {
            (true, Some(parent)) => &self.nodes[parent].item,
            (true, None) => self.root(),
            (false, _) => &node.item,
        };
        let ranges = match &node.locations {
            Some((uri, ranges)) if *uri == shown.uri => ranges.clone(),
            Some(_) => Vec::new(),
            None if node.item.uri == shown.uri => vec![node.item.selection_range],
            None => Vec::new(),
        };
        (shown.uri.clone(), shown.resource.clone(), ranges)
    }
}

fn compare_items(a: &HierarchyItem, b: &HierarchyItem) -> Ordering {
    a.uri.cmp(&b.uri).then(a.range.start.cmp(&b.range.start)).then(a.range.end.cmp(&b.range.end))
}

#[cfg(test)]
mod tests;
