//! referencesModel.ts: locations from definition, reference and similar providers, sorted by URI and range,
//! deduplicated and grouped by file.

use serde_json::Value;

/// `IRange` as 1-based (line, column) starts and ends, ordered as `Range.compareRangesUsingStarts`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Range {
    pub start: (usize, usize),
    pub end: (usize, usize),
}

impl Range {
    pub fn from_dto(range: &Value) -> Option<Self> {
        let number = |key: &str| range[key].as_u64().map(|value| value as usize);
        Some(Range { start: (number("startLineNumber")?, number("startColumn")?), end: (number("endLineNumber")?, number("endColumn")?) })
    }

    pub fn contains(&self, position: (usize, usize)) -> bool {
        self.start <= position && position <= self.end
    }
}

/// A `LocationLink` as revived by the main thread: its URI string keys the file, `link` keeps the DTO for API answers.
#[derive(Clone, Debug)]
pub struct Location {
    pub uri: String,
    pub resource: Value,
    pub range: Range,
    pub target_selection: Option<Range>,
    pub link: Value,
}

impl Location {
    /// `Location` or `LocationLink` DTOs (`uri`/`range` or `uri`/`range`/`targetSelectionRange`).
    pub fn from_dto(dto: &Value) -> Option<Self> {
        let resource = dto["uri"].clone();
        let uri = uri_key(&resource)?;
        Some(Location { uri, range: Range::from_dto(&dto["range"])?, target_selection: Range::from_dto(&dto["targetSelectionRange"]), resource, link: dto.clone() })
    }

    /// `OneReference.range`: the target selection range when there is one.
    pub fn selection(&self) -> Range {
        self.target_selection.unwrap_or(self.range)
    }
}

/// `URI.toString()` of `UriComponents`, enough to compare and group.
pub fn uri_key(uri: &Value) -> Option<String> {
    let part = |key: &str| uri[key].as_str().unwrap_or_default();
    let scheme = part("scheme");
    if scheme.is_empty() {
        return None;
    }
    let mut key = format!("{scheme}:");
    if !part("authority").is_empty() || scheme == "file" {
        key.push_str("//");
        key.push_str(part("authority"));
    }
    key.push_str(part("path"));
    Some(key)
}

pub struct Reference {
    pub location: Location,
    pub provider_first: bool,
    pub group: usize,
}

pub struct FileGroup {
    pub uri: String,
    pub resource: Value,
    pub children: Vec<usize>,
}

pub struct ReferencesModel {
    pub title: String,
    pub groups: Vec<FileGroup>,
    pub references: Vec<Reference>,
}

fn compare(a: &Location, uri: &str, range: Range) -> std::cmp::Ordering {
    a.uri.as_str().cmp(uri).then(a.range.cmp(&range))
}

impl ReferencesModel {
    pub fn new(mut links: Vec<Location>, title: &str) -> Self {
        let providers_first = links.first().map(|link| link.link.clone());
        links.sort_by(|a, b| compare(a, &b.uri, b.range));
        let mut model = ReferencesModel { title: title.to_string(), groups: Vec::new(), references: Vec::new() };
        for link in links {
            if model.groups.last().is_none_or(|group| group.uri != link.uri) {
                model.groups.push(FileGroup { uri: link.uri.clone(), resource: link.resource.clone(), children: Vec::new() });
            }
            let group = model.groups.len() - 1;
            let duplicate = model.groups[group].children.last().is_some_and(|&last| compare(&link, &model.references[last].location.uri, model.references[last].location.selection()).is_eq());
            if duplicate {
                continue;
            }
            let provider_first = providers_first.as_ref() == Some(&link.link);
            model.groups[group].children.push(model.references.len());
            model.references.push(Reference { location: link, provider_first, group });
        }
        model
    }

    pub fn first_reference(&self) -> Option<usize> {
        self.references.iter().position(|reference| reference.provider_first).or(if self.references.is_empty() { None } else { Some(0) })
    }

    pub fn reference_at(&self, uri: &str, position: (usize, usize)) -> Option<usize> {
        self.references.iter().position(|reference| reference.location.uri == uri && reference.location.selection().contains(position))
    }

    /// `nearestReference`: the longest common URI prefix, then the smallest line*100+column distance.
    pub fn nearest_reference(&self, uri: &str, position: (usize, usize)) -> Option<usize> {
        let prefix = |other: &str| uri.chars().zip(other.chars()).take_while(|(a, b)| a == b).count();
        let distance = |range: Range| range.start.0.abs_diff(position.0) * 100 + range.start.1.abs_diff(position.1);
        (0..self.references.len()).min_by_key(|&index| {
            let location = &self.references[index].location;
            (std::cmp::Reverse(prefix(&location.uri)), distance(location.selection()))
        })
    }

    /// `nextOrPreviousReference`: cycles within the file, then moves to the next or previous file's first or last.
    pub fn next_or_previous(&self, reference: usize, next: bool) -> usize {
        let group = &self.groups[self.references[reference].group];
        let index = group.children.iter().position(|&child| child == reference).unwrap();
        let count = group.children.len();
        if self.groups.len() == 1 || next && index + 1 < count || !next && index > 0 {
            return group.children[if next { (index + 1) % count } else { (index + count - 1) % count }];
        }
        let group_index = self.references[reference].group;
        let groups = self.groups.len();
        if next {
            self.groups[(group_index + 1) % groups].children[0]
        } else {
            let previous = &self.groups[(group_index + groups - 1) % groups];
            previous.children[previous.children.len() - 1]
        }
    }
}

#[cfg(test)]
mod tests;
