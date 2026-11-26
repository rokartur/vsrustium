//! The workbench undo stack of platform/undoRedo/common/undoRedoService.ts for workspace elements: one past and
//! future per resource, an element on the stack of every resource it touches, undone only while it is the last
//! past element of all of them. The file operations of the Explorer and of `workspace.applyEdit` are such elements
//! (`FileUndoRedoElement`). shortcut: no resource elements; the text editors keep their own undo (editor/, M8),
//! ordered against these by their stamps (workbench/bulk_undo.rs).

use std::collections::{BTreeMap, HashSet};

pub struct Element<T> {
    pub id: u64,
    pub label: String,
    pub confirm_before_undo: bool,
    pub source: u64,
    /// `UndoRedoGroup` id, 0 for none.
    pub group: u64,
    /// When it was pushed, in the order of every undo element (`next_undo_stamp`, `groupOrder`).
    pub stamp: u64,
    source_order: u64,
    /// The comparison keys of its resources, with their labels.
    resources: Vec<(String, String)>,
    /// Labels of resources whose stack dropped the element from its future (`RemovedResourceReason.NoParallelUniverses`).
    removed: Vec<String>,
    /// `None` while the element runs.
    actual: Option<T>,
}

#[derive(Default)]
struct Stack {
    past: Vec<u64>,
    future: Vec<u64>,
}

pub struct UndoRedo<T> {
    elements: BTreeMap<u64, Element<T>>,
    stacks: BTreeMap<String, Stack>,
    /// Resources with an undo or redo running.
    locked: HashSet<String>,
    next_id: u64,
    next_source_order: u64,
}

impl<T> Default for UndoRedo<T> {
    fn default() -> Self {
        UndoRedo { elements: BTreeMap::new(), stacks: BTreeMap::new(), locked: HashSet::new(), next_id: 1, next_source_order: 1 }
    }
}

/// Whether `begin` runs an element's undo or its redo.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Direction {
    Undo,
    Redo,
}

impl<T> UndoRedo<T> {
    /// `pushElement` of a workspace element: each resource's future is dropped, the element joins its past.
    /// `resources` are `(comparison key, label)` pairs, as `getUriComparisonKey` and `getResourceLabel` give them.
    pub fn push(&mut self, label: String, resources: Vec<(String, String)>, source: u64, group: u64, confirm_before_undo: bool, actual: T) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let source_order = self.next_source_order;
        self.next_source_order += 1;
        let mut seen = HashSet::new();
        let resources: Vec<(String, String)> = resources.into_iter().filter(|(key, _)| seen.insert(key.clone())).collect();
        for (key, label) in &resources {
            let stack = self.stacks.entry(key.clone()).or_default();
            for future in std::mem::take(&mut stack.future) {
                if let Some(element) = self.elements.get_mut(&future) {
                    element.removed.push(label.clone());
                }
            }
            stack.past.push(id);
        }
        let stamp = crate::editor::next_undo_stamp();
        self.elements.insert(id, Element { id, label, confirm_before_undo, source, group, stamp, source_order, resources, removed: Vec::new(), actual: Some(actual) });
        self.collect_garbage();
        id
    }

    /// `_findClosestUndoElementWithSource` / `_findClosestRedoElementWithSource`: of the elements of `source` that
    /// are next on some stack, the one pushed last.
    pub fn closest(&self, source: u64, direction: Direction) -> Option<&Element<T>> {
        let mut closest: Option<&Element<T>> = None;
        for stack in self.stacks.values() {
            let next = match direction {
                Direction::Undo => stack.past.last(),
                Direction::Redo => stack.future.last(),
            };
            let Some(element) = next.and_then(|id| self.elements.get(id)) else { continue };
            if element.source == source && closest.is_none_or(|closest| element.source_order > closest.source_order) {
                closest = Some(element);
            }
        }
        closest
    }

    /// `getClosestPastElement` / `getClosestFutureElement` of one resource's stack.
    pub fn next_on_resource(&self, key: &str, direction: Direction) -> Option<&Element<T>> {
        self.next_on(key, direction).and_then(|id| self.elements.get(&id))
    }

    /// `_findClosestUndoElementInGroup` / `_findClosestRedoElementInGroup`: of the group's elements next on some
    /// stack, the one pushed last for undo, first for redo.
    pub fn next_in_group(&self, group: u64, direction: Direction) -> Option<&Element<T>> {
        let mut next: Option<&Element<T>> = None;
        for key in self.stacks.keys() {
            let Some(element) = self.next_on_resource(key, direction).filter(|element| element.group == group) else { continue };
            let better = match direction {
                Direction::Undo => next.is_none_or(|next| element.stamp > next.stamp),
                Direction::Redo => next.is_none_or(|next| element.stamp < next.stamp),
            };
            if better {
                next = Some(element);
            }
        }
        next
    }

    /// `_checkWorkspaceUndo` and `moveBackward`, or their redo twins: the element's work to run, locked until
    /// `finish` or `fail`. A check that fails flushes the stacks of the element's resources (a file element cannot
    /// split) and returns the warning upstream shows.
    pub fn begin(&mut self, id: u64, direction: Direction) -> Result<T, String> {
        let element = &self.elements[&id];
        let verb = match direction {
            Direction::Undo => "undo",
            Direction::Redo => "redo",
        };
        let label = element.label.clone();
        let keys: Vec<String> = element.resources.iter().map(|(key, _)| key.clone()).collect();
        let warning = if !element.removed.is_empty() {
            Some(format!("Could not {verb} '{label}' across all files. The following files have been modified in an incompatible way: {}.", element.removed.join(", ")))
        } else {
            let changed: Vec<String> = element.resources.iter().filter(|(key, _)| self.next_on(key, direction) != Some(id)).map(|(_, label)| label.clone()).collect();
            let locked: Vec<String> = element.resources.iter().filter(|(key, _)| self.locked.contains(key)).map(|(_, label)| label.clone()).collect();
            if !changed.is_empty() {
                Some(format!("Could not {verb} '{label}' across all files because changes were made to {}", changed.join(", ")))
            } else if !locked.is_empty() {
                Some(format!("Could not {verb} '{label}' across all files because there is already an undo or redo operation running on {}", locked.join(", ")))
            } else {
                None
            }
        };
        if let Some(warning) = warning {
            for key in &keys {
                self.remove_elements(key);
            }
            return Err(warning);
        }
        for key in &keys {
            let stack = self.stacks.get_mut(key).expect("an element's resources have stacks");
            let (from, to) = match direction {
                Direction::Undo => (&mut stack.past, &mut stack.future),
                Direction::Redo => (&mut stack.future, &mut stack.past),
            };
            from.pop();
            to.push(id);
            self.locked.insert(key.clone());
        }
        Ok(self.elements.get_mut(&id).expect("checked above").actual.take().expect("an unlocked element holds its work"))
    }

    /// The element ran: its work, ready to run the other way, goes back on it.
    pub fn finish(&mut self, id: u64, actual: T) {
        let element = self.elements.get_mut(&id).expect("a running element stays on the stacks");
        element.actual = Some(actual);
        for (key, _) in &element.resources {
            self.locked.remove(key);
        }
        self.collect_garbage();
    }

    /// `_onError`: an element that failed drops the stacks of all its resources.
    pub fn fail(&mut self, id: u64) {
        let keys: Vec<String> = self.elements[&id].resources.iter().map(|(key, _)| key.clone()).collect();
        self.elements.remove(&id);
        for key in &keys {
            self.locked.remove(key);
            self.remove_elements(key);
        }
    }

    /// `removeElements`: forgets the resource's stack.
    pub fn remove_elements(&mut self, key: &str) {
        self.stacks.remove(key);
        self.collect_garbage();
    }

    fn next_on(&self, key: &str, direction: Direction) -> Option<u64> {
        let stack = self.stacks.get(key)?;
        match direction {
            Direction::Undo => stack.past.last().copied(),
            Direction::Redo => stack.future.last().copied(),
        }
    }

    /// Elements no stack holds any more are gone; a running one stays until it finishes or fails.
    fn collect_garbage(&mut self) {
        let held: HashSet<u64> = self.stacks.values().flat_map(|stack| stack.past.iter().chain(&stack.future)).copied().collect();
        self.elements.retain(|id, element| held.contains(id) || element.actual.is_none());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resources(names: &[&str]) -> Vec<(String, String)> {
        names.iter().map(|name| (name.to_string(), name.to_string())).collect()
    }

    /// `multi resource elements` of undoRedoService.test.ts, with the undo and redo the caller runs.
    #[test]
    fn a_workspace_element_moves_on_every_stack_it_touches() {
        let mut stack = UndoRedo::default();
        let id = stack.push("typing 1".into(), resources(&["test1.txt", "test2.txt"]), 1, 0, false, "work");
        assert_eq!(stack.closest(1, Direction::Undo).map(|element| element.id), Some(id));
        assert!(stack.closest(1, Direction::Redo).is_none());
        assert!(stack.closest(2, Direction::Undo).is_none(), "another source does not see it");

        assert_eq!(stack.begin(id, Direction::Undo), Ok("work"));
        assert!(stack.closest(1, Direction::Undo).is_none());
        stack.finish(id, "redo work");
        assert_eq!(stack.closest(1, Direction::Redo).map(|element| element.id), Some(id));

        assert_eq!(stack.begin(id, Direction::Redo), Ok("redo work"));
        stack.finish(id, "work");
        assert_eq!(stack.closest(1, Direction::Undo).map(|element| element.id), Some(id));
    }

    #[test]
    fn the_last_element_of_the_source_undoes_first() {
        let mut stack = UndoRedo::default();
        let first = stack.push("Create a".into(), resources(&["a"]), 1, 0, true, ());
        let second = stack.push("Create b".into(), resources(&["b"]), 1, 0, false, ());
        assert_eq!(stack.closest(1, Direction::Undo).map(|element| element.id), Some(second));
        stack.begin(second, Direction::Undo).unwrap();
        stack.finish(second, ());
        let next = stack.closest(1, Direction::Undo).unwrap();
        assert_eq!((next.id, next.confirm_before_undo), (first, true));
    }

    #[test]
    fn a_later_change_to_one_resource_flushes_the_element() {
        let mut stack = UndoRedo::default();
        let rename = stack.push("Rename a to b".into(), resources(&["b", "a"]), 1, 0, false, ());
        stack.push("Create b/c".into(), resources(&["b"]), 2, 0, false, ());
        assert_eq!(stack.begin(rename, Direction::Undo), Err("Could not undo 'Rename a to b' across all files because changes were made to b".into()));
        assert!(stack.closest(1, Direction::Undo).is_none());
        assert!(stack.closest(2, Direction::Undo).is_none(), "the flushed stacks drop every element on them");
    }

    #[test]
    fn a_new_element_drops_the_future_it_replaces() {
        let mut stack = UndoRedo::default();
        let both = stack.push("Delete 2 files".into(), resources(&["a", "b"]), 1, 0, false, ());
        stack.begin(both, Direction::Undo).unwrap();
        stack.finish(both, ());
        stack.push("Create a".into(), resources(&["a"]), 1, 0, false, ());
        assert_eq!(stack.closest(1, Direction::Redo).map(|element| element.id), Some(both), "b's future still holds it");
        assert_eq!(
            stack.begin(both, Direction::Redo),
            Err("Could not redo 'Delete 2 files' across all files. The following files have been modified in an incompatible way: a.".into())
        );
    }

    #[test]
    fn a_failed_element_drops_its_stacks() {
        let mut stack = UndoRedo::default();
        let id = stack.push("Paste a".into(), resources(&["a"]), 1, 0, false, ());
        stack.begin(id, Direction::Undo).unwrap();
        stack.fail(id);
        assert!(stack.closest(1, Direction::Redo).is_none());
        assert!(stack.elements.is_empty());
    }

    /// `_findClosestUndoElementInGroup` takes the group's element pushed last, `_findClosestRedoElementInGroup` the
    /// one pushed first.
    #[test]
    fn a_group_undoes_last_first_and_redoes_first_first() {
        let mut stack = UndoRedo::default();
        let create = stack.push("Create a".into(), resources(&["a"]), 0, 7, false, ());
        let rename = stack.push("Rename b to c".into(), resources(&["c", "b"]), 0, 7, false, ());
        stack.push("Create d".into(), resources(&["d"]), 0, 8, false, ());
        assert_eq!(stack.next_in_group(7, Direction::Undo).map(|element| element.id), Some(rename));
        for id in [rename, create] {
            stack.begin(id, Direction::Undo).unwrap();
            stack.finish(id, ());
        }
        assert!(stack.next_in_group(7, Direction::Undo).is_none());
        assert_eq!(stack.next_in_group(7, Direction::Redo).map(|element| element.id), Some(create));
    }
}
