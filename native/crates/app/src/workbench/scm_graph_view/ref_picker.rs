//! scmHistoryViewPane.ts `HistoryItemRefPicker`: the Graph view's multi-select pick of All, Auto or the provider's
//! refs by category, which sets the repository's history items filter.

use gpui::{Context, Window};
use serde_json::{Value, json};

use super::RefsFilter;
use crate::commands::Outcome;
use crate::main_thread_quick_open::Owner;
use crate::quick_input::{Entry, Event, Item, Kind, QuickInput};
use crate::scm_history::HistoryItemRef;
use crate::workbench::Workbench;

const ALL: i64 = 0;
const AUTO: i64 = 1;

/// The open picker's refs (handle 2 is the first) and its selection before the last change.
pub struct RefPicker {
    refs: Vec<HistoryItemRef>,
    selected: Vec<i64>,
}

impl Workbench {
    /// `pickHistoryItemRef`: the picker shows busy while the provider lists its refs.
    pub(in crate::workbench) fn show_graph_ref_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let Some((handle, _)) = self.scm_graph.loaded_for.as_ref() else { return Outcome::done() };
        let call = self.host.call_cancellable("ExtHostSCM", "$provideHistoryItemRefs", vec![json!(handle), Value::Null]);
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some("Select one/more history item references to view, type to filter".into());
        input.can_select_many = true;
        input.hide_check_all = true;
        input.busy = true;
        let id = self.quick_inputs.add(input, Owner::GraphRefs(RefPicker { refs: Vec::new(), selected: Vec::new() }));
        self.show_quick_input(id, window, cx);
        self.flush_quick_input_events(window, cx);
        cx.spawn_in(window, async move |workbench, cx| {
            let answer = call.await.ok().and_then(Result::ok).unwrap_or_default();
            let refs: Vec<HistoryItemRef> = answer.as_array().into_iter().flatten().filter_map(HistoryItemRef::from_dto).collect();
            let _ = workbench.update_in(cx, |workbench, window, cx| workbench.fill_graph_ref_picker(id, refs, window, cx));
        })
        .detach();
        Outcome::done()
    }

    fn fill_graph_ref_picker(&mut self, id: i64, refs: Vec<HistoryItemRef>, window: &mut Window, cx: &mut Context<Self>) {
        let (entries, refs, selected) = ref_picker_items(refs, &self.scm_graph.filter);
        let Some(Owner::GraphRefs(picker)) = self.quick_inputs.owner_mut(id) else { return };
        *picker = RefPicker { refs, selected: selected.clone() };
        let Some(input) = self.quick_inputs.get_mut(id) else { return };
        input.set_items(entries);
        input.set_selected_items(&selected);
        input.busy = false;
        self.flush_quick_input_events(window, cx);
    }

    pub fn graph_ref_picker_event(&mut self, id: i64, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Owner::GraphRefs(picker)) = self.quick_inputs.owner_mut(id) else { return };
        match event {
            Event::ChangeSelection(handles) => {
                let ids = |handle: i64| match handle {
                    ALL => "all".to_owned(),
                    AUTO => "auto".to_owned(),
                    handle => picker.refs[handle as usize - 2].id.clone(),
                };
                let selection = exclusive_selection(&picker.selected, &handles, ids);
                picker.selected = selection.clone();
                if selection != handles
                    && let Some(input) = self.quick_inputs.get_mut(id)
                {
                    input.set_selected_items(&selection);
                }
            }
            Event::Accept(_) => {
                let filter = accepted_filter(&picker.selected, &picker.refs);
                self.hide_quick_input_by_id(id, window, cx);
                if let Some(filter) = filter {
                    self.set_scm_graph_filter(filter, cx);
                }
            }
            Event::Hide => self.quick_inputs.remove(id),
            _ => {}
        }
    }
}

/// `_createQuickPickItems` with the initial selection: All, Auto, the refs grouped by category (sorted by it, each
/// group under its category's separator); a list filter's refs move after Auto under a separator of their own.
/// Returns the entries, the refs in handle order (from 2) and the selected handles.
pub(super) fn ref_picker_items(mut refs: Vec<HistoryItemRef>, filter: &RefsFilter) -> (Vec<Entry>, Vec<HistoryItemRef>, Vec<i64>) {
    refs.sort_by(|a, b| a.category.as_deref().unwrap_or_default().cmp(b.category.as_deref().unwrap_or_default()));
    let item = |handle: i64, reference: &HistoryItemRef| {
        let icon = reference.icon.as_ref().and_then(|icon| icon["id"].as_str()).map(str::to_string);
        Entry::Item(Item { handle, label: reference.name.clone(), description: reference.description.clone(), icon, pickable: true, ..Item::default() })
    };
    let mut entries = vec![
        Entry::Item(Item { handle: ALL, label: "All".into(), description: Some("All history item references".into()), pickable: true, ..Item::default() }),
        Entry::Item(Item { handle: AUTO, label: "Auto".into(), description: Some("Current history item reference(s)".into()), pickable: true, ..Item::default() }),
    ];
    let picked = |reference: &HistoryItemRef| matches!(filter, RefsFilter::Refs(ids) if ids.contains(&reference.id));
    let mut selected = match filter {
        RefsFilter::All => vec![ALL],
        RefsFilter::Auto => vec![AUTO],
        RefsFilter::Refs(_) => Vec::new(),
    };
    if refs.iter().any(picked) {
        entries.push(Entry::Separator(None));
    }
    for (index, reference) in refs.iter().enumerate().filter(|(_, reference)| picked(reference)) {
        entries.push(item(index as i64 + 2, reference));
        selected.push(index as i64 + 2);
    }
    let mut category = None;
    for (index, reference) in refs.iter().enumerate().filter(|(_, reference)| !picked(reference)) {
        if category != Some(&reference.category) {
            category = Some(&reference.category);
            entries.push(Entry::Separator(reference.category.clone()));
        }
        entries.push(item(index as i64 + 2, reference));
    }
    (entries, refs, selected)
}

/// The picker's `onDidChangeSelection`: an added All or Auto (the first added by id) is the only selection, any
/// other added ref unchecks All and Auto.
pub(super) fn exclusive_selection(previous: &[i64], now: &[i64], id: impl Fn(i64) -> String) -> Vec<i64> {
    let first_added = now.iter().copied().filter(|handle| !previous.contains(handle)).min_by_key(|handle| id(*handle));
    match first_added {
        None => now.to_vec(),
        Some(handle @ (ALL | AUTO)) => vec![handle],
        Some(_) => now.iter().copied().filter(|handle| *handle != ALL && *handle != AUTO).collect(),
    }
}

/// `onDidAccept`: nothing picked keeps the filter.
pub(super) fn accepted_filter(selected: &[i64], refs: &[HistoryItemRef]) -> Option<RefsFilter> {
    match selected {
        [] => None,
        [ALL] => Some(RefsFilter::All),
        [AUTO] => Some(RefsFilter::Auto),
        handles => Some(RefsFilter::Refs(handles.iter().filter(|handle| **handle > AUTO).map(|handle| refs[*handle as usize - 2].id.clone()).collect())),
    }
}
