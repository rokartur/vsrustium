use std::cmp::Ordering;
use std::collections::HashMap;

use serde_json::Value;

/// A theme color key, `ColorIdentifier` upstream.
pub type ColorId = &'static str;

pub const SCM_INCOMING_HISTORY_ITEM_ID: &str = "scm-graph-incoming-changes";
pub const SCM_OUTGOING_HISTORY_ITEM_ID: &str = "scm-graph-outgoing-changes";

pub const HISTORY_ITEM_REF_COLOR: ColorId = "scmGraph.historyItemRefColor";
pub const HISTORY_ITEM_REMOTE_REF_COLOR: ColorId = "scmGraph.historyItemRemoteRefColor";
pub const HISTORY_ITEM_BASE_REF_COLOR: ColorId = "scmGraph.historyItemBaseRefColor";

/// scmHistory.ts `colorRegistry`: the lane colors handed out in turn to unlabeled branches.
pub const COLOR_REGISTRY: [ColorId; 5] =
    ["scmGraph.foreground1", "scmGraph.foreground2", "scmGraph.foreground3", "scmGraph.foreground4", "scmGraph.foreground5"];

/// history.ts `ISCMHistoryItemRef`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HistoryItemRef {
    pub id: String,
    pub name: String,
    pub revision: Option<String>,
    pub category: Option<String>,
    pub description: Option<String>,
    pub color: Option<ColorId>,
    /// `IconPathDto`: a `UriComponents`, `{light, dark}` or a `ThemeIcon`.
    pub icon: Option<Value>,
}

/// history.ts `ISCMHistoryItemStatistics`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HistoryItemStatistics {
    pub files: u64,
    pub insertions: u64,
    pub deletions: u64,
}

/// history.ts `ISCMHistoryItem`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HistoryItem {
    pub id: String,
    pub parent_ids: Vec<String>,
    pub subject: String,
    pub message: String,
    pub display_id: Option<String>,
    pub author: Option<String>,
    pub author_email: Option<String>,
    pub author_icon: Option<Value>,
    pub timestamp: Option<f64>,
    pub statistics: Option<HistoryItemStatistics>,
    pub references: Vec<HistoryItemRef>,
    /// `IMarkdownString | IMarkdownString[]`.
    pub tooltip: Option<Value>,
}

/// history.ts `ISCMHistoryItemGraphNode`.
#[derive(Clone, Debug, PartialEq)]
pub struct GraphNode {
    pub id: String,
    pub color: ColorId,
}

/// history.ts `ISCMHistoryItemViewModel['kind']`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistoryItemKind {
    Head,
    Node,
    IncomingChanges,
    OutgoingChanges,
}

/// history.ts `ISCMHistoryItemViewModel`.
#[derive(Clone, Debug, PartialEq)]
pub struct HistoryItemViewModel {
    pub history_item: HistoryItem,
    pub input_swimlanes: Vec<GraphNode>,
    pub output_swimlanes: Vec<GraphNode>,
    pub kind: HistoryItemKind,
}

fn string(value: &Value) -> Option<String> {
    value.as_str().map(str::to_string)
}

fn non_null(value: &Value) -> Option<Value> {
    Some(value.clone()).filter(|value| !value.is_null())
}

impl HistoryItemRef {
    /// extHost.protocol.ts `SCMHistoryItemRefDto`; `None` without a string `id` and `name`.
    pub fn from_dto(dto: &Value) -> Option<Self> {
        Some(Self {
            id: string(&dto["id"])?,
            name: string(&dto["name"])?,
            revision: string(&dto["revision"]),
            category: string(&dto["category"]),
            description: string(&dto["description"]),
            color: None,
            icon: non_null(&dto["icon"]),
        })
    }
}

impl HistoryItem {
    /// extHost.protocol.ts `SCMHistoryItemDto`; `None` without a string `id`. Malformed refs are dropped.
    pub fn from_dto(dto: &Value) -> Option<Self> {
        let statistics = &dto["statistics"];
        Some(Self {
            id: string(&dto["id"])?,
            parent_ids: dto["parentIds"].as_array().into_iter().flatten().filter_map(string).collect(),
            subject: string(&dto["subject"]).unwrap_or_default(),
            message: string(&dto["message"]).unwrap_or_default(),
            display_id: string(&dto["displayId"]),
            author: string(&dto["author"]),
            author_email: string(&dto["authorEmail"]),
            author_icon: non_null(&dto["authorIcon"]),
            timestamp: dto["timestamp"].as_f64(),
            statistics: statistics.is_object().then(|| HistoryItemStatistics {
                files: statistics["files"].as_u64().unwrap_or_default(),
                insertions: statistics["insertions"].as_u64().unwrap_or_default(),
                deletions: statistics["deletions"].as_u64().unwrap_or_default(),
            }),
            references: dto["references"].as_array().into_iter().flatten().filter_map(HistoryItemRef::from_dto).collect(),
            tooltip: non_null(&dto["tooltip"]),
        })
    }
}

/// scmHistory.ts `getLabelColorIdentifier`.
fn label_color(history_item: &HistoryItem, color_map: &HashMap<String, Option<ColorId>>) -> Option<ColorId> {
    if history_item.id == SCM_INCOMING_HISTORY_ITEM_ID {
        return Some(HISTORY_ITEM_REMOTE_REF_COLOR);
    }
    if history_item.id == SCM_OUTGOING_HISTORY_ITEM_ID {
        return Some(HISTORY_ITEM_REF_COLOR);
    }
    history_item.references.iter().find_map(|reference| color_map.get(&reference.id).copied().flatten())
}

/// scmHistory.ts `toISCMHistoryItemViewModelArray`. A `color_map` entry of `None` gives the ref the color of
/// its item's graph node.
#[allow(clippy::too_many_arguments)]
pub fn to_history_item_view_models(
    history_items: &[HistoryItem],
    color_map: &HashMap<String, Option<ColorId>>,
    current_ref: Option<&HistoryItemRef>,
    current_remote_ref: Option<&HistoryItemRef>,
    current_base_ref: Option<&HistoryItemRef>,
    add_incoming_changes: bool,
    add_outgoing_changes: bool,
    merge_base: Option<&str>,
) -> Vec<HistoryItemViewModel> {
    let mut color_index = 0;
    let mut view_models: Vec<HistoryItemViewModel> = Vec::new();

    for history_item in history_items {
        let is_head = current_ref.and_then(|r| r.revision.as_deref()) == Some(history_item.id.as_str());
        let kind = if is_head { HistoryItemKind::Head } else { HistoryItemKind::Node };
        let input_swimlanes = view_models.last().map(|vm| vm.output_swimlanes.clone()).unwrap_or_default();
        let mut output_swimlanes = Vec::new();

        let mut first_parent_added = false;
        if let Some(first_parent) = history_item.parent_ids.first() {
            for node in &input_swimlanes {
                if node.id != history_item.id {
                    output_swimlanes.push(node.clone());
                } else if !first_parent_added {
                    let color = label_color(history_item, color_map).unwrap_or(node.color);
                    output_swimlanes.push(GraphNode { id: first_parent.clone(), color });
                    first_parent_added = true;
                }
            }
        }

        for (i, parent_id) in history_item.parent_ids.iter().enumerate().skip(usize::from(first_parent_added)) {
            let label = if i == 0 {
                label_color(history_item, color_map)
            } else {
                history_items.iter().find(|h| &h.id == parent_id).and_then(|parent| label_color(parent, color_map))
            };
            let color = label.unwrap_or_else(|| {
                let color = COLOR_REGISTRY[color_index];
                color_index = (color_index + 1) % COLOR_REGISTRY.len();
                color
            });
            output_swimlanes.push(GraphNode { id: parent_id.clone(), color });
        }

        let mut references: Vec<HistoryItemRef> = history_item
            .references
            .iter()
            .map(|reference| {
                let color = match color_map.get(&reference.id) {
                    Some(None) => Some(node_color(&history_item.id, &input_swimlanes, &output_swimlanes)),
                    Some(color) => *color,
                    None => None,
                };
                HistoryItemRef { color, ..reference.clone() }
            })
            .collect();
        references.sort_by(|a, b| compare_history_item_refs(a, b, current_ref, current_remote_ref, current_base_ref));

        view_models.push(HistoryItemViewModel {
            history_item: HistoryItem { references, ..history_item.clone() },
            input_swimlanes,
            output_swimlanes,
            kind,
        });
    }

    add_incoming_outgoing_changes_history_items(
        &mut view_models,
        current_ref,
        current_remote_ref,
        add_incoming_changes,
        add_outgoing_changes,
        merge_base,
    );
    view_models
}

/// The circle color of `renderSCMHistoryItemGraph`, inlined in `toISCMHistoryItemViewModelArray`.
pub fn node_color(id: &str, input_swimlanes: &[GraphNode], output_swimlanes: &[GraphNode]) -> ColorId {
    let circle_index = input_swimlanes.iter().position(|node| node.id == id).unwrap_or(input_swimlanes.len());
    output_swimlanes
        .get(circle_index)
        .or_else(|| input_swimlanes.get(circle_index))
        .map_or(HISTORY_ITEM_REF_COLOR, |node| node.color)
}

/// scmHistory.ts `getHistoryItemIndex`: the swimlane column of the item's circle.
pub fn history_item_index(view_model: &HistoryItemViewModel) -> usize {
    let id = &view_model.history_item.id;
    let input_swimlanes = &view_model.input_swimlanes;
    input_swimlanes.iter().position(|node| &node.id == id).unwrap_or(input_swimlanes.len())
}

/// scmHistory.ts `addIncomingOutgoingChangesHistoryItems`.
fn add_incoming_outgoing_changes_history_items(
    view_models: &mut Vec<HistoryItemViewModel>,
    current_ref: Option<&HistoryItemRef>,
    current_remote_ref: Option<&HistoryItemRef>,
    add_incoming_changes: bool,
    add_outgoing_changes: bool,
    merge_base: Option<&str>,
) {
    let current_revision = current_ref.and_then(|r| r.revision.as_deref());
    let remote_revision = current_remote_ref.and_then(|r| r.revision.as_deref());
    let Some(merge_base) = merge_base.filter(|merge_base| !merge_base.is_empty()) else { return };
    if current_revision == remote_revision {
        return;
    }

    if let Some(remote_ref) = current_remote_ref.filter(|_| add_incoming_changes && remote_revision != Some(merge_base)) {
        add_incoming_changes_history_item(view_models, remote_ref, merge_base);
    }

    let Some((current_ref, revision)) = current_ref.zip(current_revision) else { return };
    if !add_outgoing_changes || revision.is_empty() || revision == merge_base {
        return;
    }
    let Some(head_index) =
        view_models.iter().position(|vm| vm.kind == HistoryItemKind::Head && vm.history_item.id == revision)
    else {
        return;
    };
    let display_id = view_models[0].history_item.display_id.as_ref().filter(|id| !id.is_empty()).map(|id| "0".repeat(id.len()));
    let outgoing_changes_history_item = HistoryItem {
        id: SCM_OUTGOING_HISTORY_ITEM_ID.to_string(),
        display_id,
        parent_ids: vec![revision.to_string()],
        author: Some(current_ref.name.clone()),
        subject: "Outgoing Changes".to_string(),
        ..HistoryItem::default()
    };
    let head_node = GraphNode { id: revision.to_string(), color: HISTORY_ITEM_REF_COLOR };
    let input_swimlanes = view_models[head_index].input_swimlanes.clone();
    let mut output_swimlanes = input_swimlanes.clone();
    output_swimlanes.push(head_node.clone());
    view_models.insert(
        head_index,
        HistoryItemViewModel {
            history_item: outgoing_changes_history_item,
            input_swimlanes,
            output_swimlanes,
            kind: HistoryItemKind::OutgoingChanges,
        },
    );
    view_models[head_index + 1].input_swimlanes.push(head_node);
}

/// The incoming changes half of `addIncomingOutgoingChangesHistoryItems`.
fn add_incoming_changes_history_item(view_models: &mut Vec<HistoryItemViewModel>, remote_ref: &HistoryItemRef, merge_base: &str) {
    // Either is missing while the merge base item is not loaded yet.
    let Some(before_index) = view_models.iter().rposition(|vm| vm.output_swimlanes.iter().any(|node| node.id == merge_base))
    else {
        return;
    };
    let Some(after_index) = view_models.iter().position(|vm| vm.history_item.id == merge_base) else { return };

    // The incoming changes are already merged, https://github.com/microsoft/vscode/issues/276064.
    let before_parents = &view_models[before_index].history_item.parent_ids;
    if before_parents.len() == 2 && before_parents.iter().any(|id| id == merge_base) {
        return;
    }

    let before = &mut view_models[before_index];
    for node in before.input_swimlanes.iter_mut().chain(before.output_swimlanes.iter_mut()) {
        if node.id == merge_base && node.color == HISTORY_ITEM_REMOTE_REF_COLOR {
            node.id = SCM_INCOMING_HISTORY_ITEM_ID.to_string();
        }
    }

    let display_id_length = view_models[0].history_item.display_id.as_ref().map_or(0, String::len);
    let incoming_changes_history_item = HistoryItem {
        id: SCM_INCOMING_HISTORY_ITEM_ID.to_string(),
        display_id: Some("0".repeat(display_id_length)),
        parent_ids: vec![merge_base.to_string()],
        author: Some(remote_ref.name.clone()),
        subject: "Incoming Changes".to_string(),
        ..HistoryItem::default()
    };
    let input_swimlanes = view_models[before_index].output_swimlanes.clone();
    let output_swimlanes = view_models[after_index].input_swimlanes.clone();
    view_models.insert(
        after_index,
        HistoryItemViewModel {
            history_item: incoming_changes_history_item,
            input_swimlanes,
            output_swimlanes,
            kind: HistoryItemKind::IncomingChanges,
        },
    );
}

/// scmHistory.ts `compareHistoryItemRefs`: current, then remote, then base, then colored refs, then the rest.
pub fn compare_history_item_refs(
    ref1: &HistoryItemRef,
    ref2: &HistoryItemRef,
    current_ref: Option<&HistoryItemRef>,
    current_remote_ref: Option<&HistoryItemRef>,
    current_base_ref: Option<&HistoryItemRef>,
) -> Ordering {
    let order = |reference: &HistoryItemRef| {
        let is = |other: Option<&HistoryItemRef>| other.is_some_and(|other| other.id == reference.id);
        if is(current_ref) {
            1
        } else if is(current_remote_ref) {
            2
        } else if is(current_base_ref) {
            3
        } else if reference.color.is_some() {
            4
        } else {
            99
        }
    };
    order(ref1).cmp(&order(ref2))
}

#[cfg(test)]
mod tests;
