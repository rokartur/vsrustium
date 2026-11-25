//! outlineModel.ts: the document symbols of a model as a tree, one group per provider, compacted to the elements
//! when only one provider gave symbols; markers are counted per element for the Outline view.

use serde_json::Value;

/// `DocumentSymbol` as the extension host sends it: 1-based ranges (`IRange`).
#[derive(Clone, Debug, PartialEq)]
pub struct DocumentSymbol {
    pub name: String,
    pub detail: String,
    pub kind: u64,
    pub tags: Vec<u64>,
    pub container_name: Option<String>,
    pub range: Range,
    pub selection_range: Range,
    pub children: Vec<DocumentSymbol>,
}

/// `IRange` as 1-based (line, column) pairs, ordered as `Range.compareRangesUsingStarts`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Range {
    pub start: (usize, usize),
    pub end: (usize, usize),
}

impl Range {
    pub fn new(start_line: usize, start_column: usize, end_line: usize, end_column: usize) -> Self {
        Range { start: (start_line, start_column), end: (end_line, end_column) }
    }

    pub fn from_dto(range: &Value) -> Option<Self> {
        let number = |key: &str| range[key].as_u64().map(|value| value as usize);
        Some(Range::new(number("startLineNumber")?, number("startColumn")?, number("endLineNumber")?, number("endColumn")?))
    }

    pub fn contains(&self, position: (usize, usize)) -> bool {
        self.start <= position && position <= self.end
    }

    /// `Range.areIntersecting`: touching ranges do not intersect.
    fn intersects(&self, other: &Range) -> bool {
        !(self.end <= other.start || other.end <= self.start)
    }
}

impl DocumentSymbol {
    pub fn from_dto(dto: &Value) -> Option<Self> {
        Some(DocumentSymbol {
            name: dto["name"].as_str()?.to_string(),
            detail: dto["detail"].as_str().unwrap_or_default().to_string(),
            kind: dto["kind"].as_u64().unwrap_or(0),
            tags: dto["tags"].as_array().map(|tags| tags.iter().filter_map(Value::as_u64).collect()).unwrap_or_default(),
            container_name: dto["containerName"].as_str().map(String::from),
            range: Range::from_dto(&dto["range"])?,
            selection_range: Range::from_dto(&dto["selectionRange"])?,
            children: dto["children"].as_array().map(|children| children.iter().filter_map(DocumentSymbol::from_dto).collect()).unwrap_or_default(),
        })
    }
}

/// `IOutlineMarker`: a marker's range and severity (MarkerSeverity: Hint 1, Info 2, Warning 4, Error 8).
#[derive(Clone, Copy, Debug)]
pub struct OutlineMarker {
    pub range: Range,
    pub severity: u32,
}

#[derive(Clone, Debug)]
pub struct OutlineElement {
    pub id: String,
    pub symbol: DocumentSymbol,
    pub children: Vec<OutlineElement>,
    /// The markers in the element and its children, and the most severe.
    pub marker: Option<(usize, u32)>,
}

#[derive(Clone, Debug)]
pub struct OutlineGroup {
    pub id: String,
    pub label: String,
    pub children: Vec<OutlineElement>,
}

/// The root's children: the groups of several providers, or the elements of the only one.
#[derive(Clone, Debug)]
pub enum OutlineChildren {
    Groups,
    Elements,
}

#[derive(Clone, Debug)]
pub struct OutlineModel {
    pub groups: Vec<OutlineGroup>,
    pub children: OutlineChildren,
}

/// `TreeElement.findId`: the container's id and the name, then the start position, then a counter for duplicates.
fn find_id(candidate: &str, start: Option<(usize, usize)>, container_id: &str, siblings: &[String]) -> String {
    let taken = |id: &str| siblings.iter().any(|sibling| sibling == id);
    let mut candidate_id = format!("{container_id}/{candidate}");
    if let Some((line, column)) = start
        && taken(&candidate_id)
    {
        candidate_id = format!("{container_id}/{candidate}_{line}_{column}");
    }
    let mut id = candidate_id.clone();
    let mut index = 0;
    while taken(&id) {
        id = format!("{candidate_id}_{index}");
        index += 1;
    }
    id
}

fn make_element(symbol: DocumentSymbol, container_id: &str, siblings: &[String]) -> OutlineElement {
    let id = find_id(&symbol.name, Some(symbol.range.start), container_id, siblings);
    let mut children = Vec::new();
    for child in &symbol.children {
        let ids: Vec<String> = children.iter().map(|child: &OutlineElement| child.id.clone()).collect();
        children.push(make_element(child.clone(), &id, &ids));
    }
    OutlineElement { id, symbol, children, marker: None }
}

impl OutlineModel {
    /// `OutlineModel.create`: one group per provider in score order (`provider_<index>`, its display name), the
    /// empty ones left out, then `_compact`.
    pub fn new(answers: Vec<(String, Vec<DocumentSymbol>)>) -> Self {
        let mut groups = Vec::new();
        for (order, (label, symbols)) in answers.into_iter().enumerate() {
            let group_ids: Vec<String> = groups.iter().map(|group: &OutlineGroup| group.id.clone()).collect();
            let id = find_id(&format!("provider_{order}"), None, "root", &group_ids);
            let mut children = Vec::new();
            for symbol in symbols {
                let ids: Vec<String> = children.iter().map(|child: &OutlineElement| child.id.clone()).collect();
                children.push(make_element(symbol, &id, &ids));
            }
            if !children.is_empty() {
                groups.push(OutlineGroup { id, label, children });
            }
        }
        let children = if groups.len() == 1 { OutlineChildren::Elements } else { OutlineChildren::Groups };
        OutlineModel { groups, children }
    }

    /// The root's elements when one provider gave symbols, the groups' elements in order otherwise.
    pub fn elements(&self) -> impl Iterator<Item = &OutlineElement> {
        self.groups.iter().flat_map(|group| &group.children)
    }

    /// `getItemEnclosingPosition`: the deepest element whose range holds the position, in the context's group when
    /// one is given.
    pub fn item_enclosing_position(&self, position: (usize, usize), preferred_group: Option<&str>) -> Option<&OutlineElement> {
        let mut result = None;
        for group in &self.groups {
            result = enclosing(&group.children, position);
            if result.is_some() && preferred_group.is_none_or(|preferred| preferred == group.id) {
                break;
            }
        }
        result
    }

    /// `getItemById`, with the path of elements from the top down to it.
    pub fn path_to(&self, id: &str) -> Option<Vec<&OutlineElement>> {
        fn walk<'a>(elements: &'a [OutlineElement], id: &str, path: &mut Vec<&'a OutlineElement>) -> bool {
            for element in elements {
                path.push(element);
                if element.id == id || (id.starts_with(&element.id) && walk(&element.children, id, path)) {
                    return true;
                }
                path.pop();
            }
            false
        }
        let mut path = Vec::new();
        self.groups.iter().any(|group| walk(&group.children, id, &mut path)).then_some(path)
    }

    /// `updateMarker`: each group counts the markers sorted by start.
    pub fn update_markers(&mut self, markers: &[OutlineMarker]) {
        let mut sorted = markers.to_vec();
        sorted.sort_by_key(|marker| marker.range);
        for group in &mut self.groups {
            update_markers(&mut group.children, &mut sorted.clone());
        }
    }

    /// `getTopLevelSymbols`: the top elements of every group by range start.
    pub fn top_level_symbols(&self) -> Vec<&DocumentSymbol> {
        let mut roots: Vec<&DocumentSymbol> = self.elements().map(|element| &element.symbol).collect();
        roots.sort_by_key(|symbol| symbol.range.start);
        roots
    }

    /// `asListOfDocumentSymbols`: every symbol flattened with its parent's name as the container name, by start
    /// then by end descending.
    pub fn as_list_of_document_symbols(&self) -> Vec<DocumentSymbol> {
        fn flatten(bucket: &mut Vec<DocumentSymbol>, entries: &[&DocumentSymbol], container: &str) {
            for entry in entries {
                let container_name = entry.container_name.clone().filter(|name| !name.is_empty()).unwrap_or_else(|| container.to_string());
                bucket.push(DocumentSymbol { container_name: Some(container_name), children: Vec::new(), ..(*entry).clone() });
                let children: Vec<&DocumentSymbol> = entry.children.iter().collect();
                flatten(bucket, &children, &entry.name);
            }
        }
        let mut bucket = Vec::new();
        flatten(&mut bucket, &self.top_level_symbols(), "");
        bucket.sort_by(|a, b| a.range.start.cmp(&b.range.start).then(b.range.end.cmp(&a.range.end)));
        bucket
    }
}

fn enclosing(children: &[OutlineElement], position: (usize, usize)) -> Option<&OutlineElement> {
    let item = children.iter().find(|item| item.symbol.range.contains(position))?;
    enclosing(&item.children, position).or(Some(item))
}

/// `OutlineGroup._updateMarker` for each child: an element takes the markers that intersect it out of `markers`,
/// its children take theirs out of those, and the element keeps the count of the rest with the most severe of all
/// it took (a dot for markers only in its children).
fn update_markers(children: &mut [OutlineElement], markers: &mut Vec<OutlineMarker>) {
    for item in children {
        item.marker = None;
        let range = item.symbol.range;
        let mut start = markers.partition_point(|marker| marker.range < range);
        if start > 0 && markers[start - 1].range.intersects(&range) {
            start -= 1;
        }
        let mut mine = Vec::new();
        while start < markers.len() && range.intersects(&markers[start].range) {
            mine.push(markers.remove(start));
        }
        let top = mine.iter().map(|marker| marker.severity).max();
        update_markers(&mut item.children, &mut mine);
        if let Some(top) = top {
            item.marker = Some((mine.len(), top));
        }
    }
}

#[cfg(test)]
mod tests;
