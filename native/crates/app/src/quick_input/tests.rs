//! Ported from platform/quickinput/test/browser/quickinput.test.ts (the cases about the model, not the DOM),
//! plus the event sequences of extensions/vscode-api-tests quickInput.test.ts.

use super::*;

fn items(labels: &[&str]) -> Vec<Entry> {
    labels
        .iter()
        .enumerate()
        .map(|(handle, label)| match label.strip_prefix("---") {
            Some(separator) => Entry::Separator((!separator.is_empty()).then(|| separator.to_owned())),
            None => Entry::Item(Item { handle: handle as i64, label: label.to_string(), pickable: true, ..Item::default() }),
        })
        .collect()
}

fn pick(labels: &[&str]) -> QuickInput {
    let mut input = QuickInput::new(1, Kind::QuickPick);
    input.set_items(items(labels));
    input
}

fn labels(input: &QuickInput, handles: &[i64]) -> Vec<String> {
    handles
        .iter()
        .map(|handle| {
            input.items().iter().find_map(|entry| match entry {
                Entry::Item(item) if item.handle == *handle => Some(item.label.clone()),
                _ => None,
            })
            .unwrap()
        })
        .collect()
}

/// The events as the api-tests record them: `active`, `selection`, `accept`, `hide`, with their labels.
fn drain(input: &mut QuickInput) -> Vec<String> {
    let events = std::mem::take(&mut input.events);
    events
        .into_iter()
        .map(|event| match event {
            Event::ChangeActive(handles) => format!("active {:?}", labels(input, &handles)),
            Event::ChangeSelection(handles) => format!("selection {:?}", labels(input, &handles)),
            Event::Accept(_) => "accept".into(),
            Event::ChangeValue(value) => format!("value {value}"),
            Event::Hide => "hide".into(),
            Event::TriggerButton(handle, checked) => format!("button {handle} {checked:?}"),
            Event::TriggerItemButton(item, button, checked) => format!("item button {item} {button} {checked:?}"),
        })
        .collect()
}

fn active(input: &QuickInput) -> Vec<String> {
    labels(input, &input.active_handles())
}

#[test]
fn pick_basecase() {
    let mut input = pick(&["foo", "bar"]);
    input.show();
    input.accept_from_ui();
    assert_eq!(labels(&input, &input.selected_handles()), ["foo"]);
}

#[test]
fn pick_active_item_is_honored() {
    let mut input = pick(&["bar", "foo"]);
    input.show();
    input.set_active_items(&[1]);
    input.accept_from_ui();
    assert_eq!(labels(&input, &input.selected_handles()), ["foo"]);
}

#[test]
fn on_did_change_value_gets_triggered_when_value_is_set() {
    let mut input = QuickInput::new(1, Kind::QuickPick);
    input.set_value("changed".into());
    assert_eq!(drain(&mut input), ["value changed"]);
}

#[test]
fn selected_items_do_not_hang_over_to_the_next_set_of_items() {
    let mut input = pick(&["step 1"]);
    input.show();
    input.accept_from_ui();
    input.can_select_many = true;
    input.set_items(items(&["a", "b", "c"]));
    input.accept_from_ui();
    assert!(input.selected_handles().is_empty());
}

#[test]
fn on_did_change_active_is_triggered_after_setting_items() {
    let mut input = QuickInput::new(1, Kind::QuickPick);
    input.show();
    input.set_items(items(&["step 1"]));
    assert_eq!(drain(&mut input), [r#"active ["step 1"]"#]);
    assert_eq!(active(&input), ["step 1"]);
}

/// #207832
#[test]
fn item_activation_none_still_triggers_on_did_change_active_after_selection() {
    let mut input = pick(&["step 1"]);
    input.show();
    assert_eq!(active(&input), ["step 1"]);
    input.events.clear();
    input.item_activation = ItemActivation::None;
    input.set_items(items(&["step 1"]));
    assert_eq!(drain(&mut input), ["active []"]);
    assert!(input.active_handles().is_empty());
}

#[test]
fn previous_section_targets_visible_matches_instead_of_hidden_first_children() {
    let mut input = pick(&["apple", "---", "berry", "apricot"]);
    input.sort_by_label = false;
    input.set_value("a".into());
    input.show();
    input.focus(Focus::PreviousSeparator, 0);
    input.accept_from_ui();
    assert_eq!(active(&input), ["apricot"]);
    assert_eq!(labels(&input, &input.selected_handles()), ["apricot"]);
}

#[test]
fn previous_section_moves_to_the_current_section_start_before_the_preceding_section() {
    let mut input = pick(&["A1", "A2", "---", "B1", "B2"]);
    input.show();
    input.focus(Focus::Last, 0);
    let mut seen = Vec::new();
    for _ in 0..3 {
        input.focus(Focus::PreviousSeparator, 0);
        seen.push(active(&input));
    }
    assert_eq!(seen, [["B1"], ["A1"], ["B1"]]);
}

#[test]
fn next_section_wraps_to_the_first_visible_item_when_filtering_hides_the_original_first_item() {
    let mut input = pick(&["hidden", "hit one", "---", "hit two"]);
    input.sort_by_label = false;
    input.set_value("hit".into());
    input.show();
    input.set_active_items(&[3]);
    let mut seen = Vec::new();
    for _ in 0..3 {
        input.focus(Focus::NextSeparator, 0);
        seen.push(active(&input));
    }
    assert_eq!(seen, [["hit one"], ["hit two"], ["hit one"]]);
}

#[test]
fn previous_section_distinguishes_a_section_interior_and_its_first_item() {
    let mut input = pick(&["---A", "A1", "A2", "---B", "B1", "B2"]);
    input.show();
    input.set_active_items(&[5]);
    input.focus(Focus::PreviousSeparator, 0);
    let from_interior = active(&input);
    input.focus(Focus::PreviousSeparator, 0);
    assert_eq!((from_interior, active(&input)), (vec!["B1".to_owned()], vec!["A1".to_owned()]));
}

#[test]
fn section_navigation_starts_at_the_correct_boundary_with_no_active_item() {
    for separator in ["---A", "---"] {
        let mut input = QuickInput::new(1, Kind::QuickPick);
        input.item_activation = ItemActivation::None;
        input.set_items(items(&[separator, "A1", separator, "B1", separator, "C1"]));
        input.show();
        assert!(input.active_handles().is_empty());
        input.focus(Focus::PreviousSeparator, 0);
        let backward = active(&input);
        input.set_active_items(&[]);
        input.focus(Focus::NextSeparator, 0);
        assert_eq!((backward, active(&input)), (vec!["C1".to_owned()], vec!["A1".to_owned()]));
    }
}

#[test]
fn filtering_does_not_carry_a_section_label_across_an_unlabelled_boundary() {
    let mut input = pick(&["---A", "apple", "---", "berry"]);
    input.sort_by_label = false;
    input.show();
    let mut states = Vec::new();
    for query in ["berry", "apple", ""] {
        input.set_value(query.into());
        let sections: Vec<String> = input.order.iter().filter_map(|row| input.rows[*row].separator.clone().filter(|label| !label.is_empty())).collect();
        states.push((query, active(&input), sections));
    }
    let owned = |labels: &[&str]| labels.iter().map(|label| label.to_string()).collect::<Vec<_>>();
    assert_eq!(states, [("berry", owned(&["berry"]), owned(&[])), ("apple", owned(&["apple"]), owned(&["A"])), ("", owned(&["apple"]), owned(&["A"]))]);
}

#[test]
fn api_create_quick_pick_select_second() {
    let mut input = pick(&["eins", "zwei", "drei"]);
    input.show();
    input.focus(Focus::Next, 0);
    input.accept_from_ui();
    input.did_hide();
    assert_eq!(drain(&mut input), [r#"active ["eins"]"#, r#"active ["zwei"]"#, r#"selection ["zwei"]"#, "accept", "hide"]);
}

#[test]
fn api_create_quick_pick_focus_second() {
    let mut input = pick(&["eins", "zwei", "drei"]);
    input.set_active_items(&[1]);
    input.show();
    input.accept_from_ui();
    assert_eq!(drain(&mut input), [r#"active ["zwei"]"#, r#"selection ["zwei"]"#, "accept"]);
}

#[test]
fn api_create_quick_pick_select_first_and_second() {
    let mut input = QuickInput::new(1, Kind::QuickPick);
    input.can_select_many = true;
    input.set_items(items(&["eins", "zwei", "drei"]));
    input.show();
    input.focus(Focus::Next, 0);
    input.toggle();
    input.focus(Focus::Next, 0);
    input.toggle();
    input.accept_from_ui();
    assert_eq!(
        drain(&mut input),
        [r#"active ["eins"]"#, r#"selection ["eins"]"#, r#"active ["zwei"]"#, r#"selection ["eins", "zwei"]"#, "accept"]
    );
    assert_eq!(active(&input), ["zwei"]);
}

#[test]
fn api_create_quick_pick_selection_events() {
    let mut input = pick(&["eins", "zwei", "drei"]);
    input.show();
    input.set_selected_items(&[1]);
    input.set_selected_items(&[2]);
    assert_eq!(drain(&mut input), [r#"active ["eins"]"#, r#"selection ["zwei"]"#, "accept", r#"selection ["drei"]"#, "accept"]);
    assert_eq!(active(&input), ["eins"]);
}

#[test]
fn api_create_quick_pick_continue_after_first_accept() {
    let mut input = pick(&["eins", "zwei"]);
    input.show();
    input.accept_from_ui();
    let first = drain(&mut input);
    input.set_items(items(&["drei", "vier"]));
    input.accept_from_ui();
    assert_eq!(first, [r#"active ["eins"]"#, r#"selection ["eins"]"#, "accept"]);
    assert_eq!(drain(&mut input), [r#"active ["drei"]"#, r#"selection ["drei"]"#, "accept"]);
}

#[test]
fn api_create_quick_pick_hide_show_hide_fires_two_hides() {
    let mut input = QuickInput::new(1, Kind::QuickPick);
    input.show();
    input.did_hide();
    input.did_hide();
    input.show();
    input.did_hide();
    assert_eq!(drain(&mut input), ["hide", "hide"]);
}

#[test]
fn api_match_item_by_label_derived_from_resource_uri() {
    let workspace = Workspace::folder(PathBuf::from("/work"));
    let transfer = [
        json_item(0, Some("a1"), "/work/test1.txt"),
        json_item(1, None, "/work/test2.txt"),
        json_item(2, Some("a3"), "/work/test3.txt"),
    ];
    let mut input = QuickInput::new(1, Kind::QuickPick);
    input.set_items(transfer.iter().map(|item| entry_from_transfer(item, Some(&workspace))).collect());
    input.set_value("test2.txt".into());
    input.show();
    input.accept_from_ui();
    assert_eq!(input.selected_handles(), [1]);
    assert_eq!(labels(&input, &[1]), ["test2.txt"]);
}

#[test]
fn api_match_item_by_description_derived_from_resource_uri() {
    let workspace = Workspace::folder(PathBuf::from("/work"));
    let transfer = [
        json_item(0, Some("a1"), "/work/test1.txt"),
        json_item(1, Some("a2"), "/work/test2.txt"),
        json_item(2, Some("a3"), "/work/test3.txt"),
    ];
    let mut input = QuickInput::new(1, Kind::QuickPick);
    input.set_items(transfer.iter().map(|item| entry_from_transfer(item, Some(&workspace))).collect());
    input.match_on_description = true;
    input.set_value("test2.txt".into());
    input.show();
    input.accept_from_ui();
    assert_eq!(labels(&input, &input.selected_handles()), ["a2"]);
}

#[test]
fn sort_by_label_puts_label_matches_first() {
    let mut input = pick(&["xbar", "foo", "bar"]);
    input.set_value("bar".into());
    input.show();
    let shown: Vec<&str> = input.order.iter().map(|row| input.rows[*row].item.label.as_str()).collect();
    assert_eq!(shown, ["bar", "xbar"]);
}

#[test]
fn input_box_accepts_its_value() {
    let mut input = QuickInput::new(1, Kind::InputBox);
    input.set_value("foo".into());
    input.show();
    input.accept_from_ui();
    assert_eq!(drain(&mut input), ["value foo", "accept"]);
    assert_eq!(input.input_box.value, "foo");
}

fn json_item(handle: i64, label: Option<&str>, path: &str) -> Value {
    serde_json::json!({ "handle": handle, "label": label, "resourceUri": { "scheme": "file", "path": path } })
}

#[test]
fn drag_layout_snaps_clamps_and_clears_window_controls() {
    let window = (1000., 800.);
    let widget = (600., 300.);
    // Near the title bar top and the horizontal center: both snap.
    assert_eq!(drag_layout(15., 190., window, widget, 35., 6.), (6. / 800., 0.5));
    // Near the 25% line.
    assert_eq!(drag_layout(210., 190., window, widget, 35., 6.), (200. / 800., 0.5));
    // Off every snap line: kept as dragged.
    assert_eq!(drag_layout(400., 300., window, widget, 35., 6.), (400. / 800., 600. / 1000.));
    // Past the bottom right corner: clamped inside the window.
    assert_eq!(drag_layout(900., 900., window, widget, 35., 6.), (500. / 800., 700. / 1000.));
    // Over the title bar at the left edge: moved right of the traffic lights.
    assert_eq!(drag_layout(30., 0., window, widget, 35., 6.), (30. / 800., 380. / 1000.));
}

/// label.test.ts 'labels of files in multiroot workspaces are the foldername followed by offset from the folder'.
#[test]
fn path_labels_in_a_multi_root_workspace_start_with_the_folder_name() {
    use config::workspace::WorkspaceFolder;
    let folder = |path: &str, name: &str| WorkspaceFolder { path: PathBuf::from(path), name: name.into() };
    let folders = vec![folder("/folder1/src", "Sources"), folder("/folder1/test", "Tests"), folder("/folder2", "folder2")];
    let workspace = Workspace { file: Some(PathBuf::from("/test.code-workspace")), folders, transient: false };
    let label = |path: &str| path_label(Path::new(path), Some(&workspace));
    assert_eq!(label("/folder1/src/file"), "Sources \u{2022} file");
    assert_eq!(label("/folder1/src/folder/file"), "Sources \u{2022} folder/file");
    assert_eq!(label("/folder1/src"), "Sources");
    assert_eq!(label("/folder1/other"), "/folder1/other");
    assert_eq!(label("/folder2/other"), "folder2 \u{2022} other");
}
