use super::*;

/// outlineModel.test.ts `fakeSymbolInformation`.
fn symbol(range: Range, name: &str) -> DocumentSymbol {
    DocumentSymbol { name: name.into(), detail: "fake".into(), kind: 16, tags: Vec::new(), container_name: None, range, selection_range: range, children: Vec::new() }
}

fn element(id: &str, range: Range) -> OutlineElement {
    OutlineElement { id: id.into(), symbol: symbol(range, "foo"), children: Vec::new(), marker: None }
}

fn marker(range: Range) -> OutlineMarker {
    OutlineMarker { range, severity: 8 }
}

fn group(children: Vec<OutlineElement>) -> OutlineModel {
    OutlineModel { groups: vec![OutlineGroup { id: "group".into(), label: String::new(), children }], children: OutlineChildren::Elements }
}

fn counts(elements: &[OutlineElement]) -> Vec<Option<usize>> {
    elements.iter().map(|element| element.marker.map(|(count, _)| count)).collect()
}

#[test]
fn outline_element_update_marker() {
    let mut model = group(vec![element("foo1", Range::new(1, 1, 1, 10)), element("foo2", Range::new(2, 1, 5, 1)), element("foo3", Range::new(6, 1, 10, 10))]);
    let mut data = vec![marker(Range::new(6, 1, 6, 7)), marker(Range::new(1, 1, 1, 4)), marker(Range::new(10, 2, 14, 1))];
    data.sort_by_key(|marker| marker.range);
    update_markers(&mut model.groups[0].children, &mut data);
    assert!(data.is_empty(), "all 'stolen'");
    assert_eq!(counts(&model.groups[0].children), [Some(1), None, Some(2)]);
    model.update_markers(&[]);
    assert_eq!(counts(&model.groups[0].children), [None, None, None]);
}

#[test]
fn outline_element_update_marker_2() {
    let mut parent = element("A", Range::new(1, 1, 11, 1));
    parent.children = vec![element("A/B", Range::new(2, 4, 5, 4)), element("A/C", Range::new(6, 4, 9, 4))];
    let mut model = group(vec![parent]);
    let check = |model: &mut OutlineModel, data: Vec<OutlineMarker>, parent: Option<usize>, children: [Option<usize>; 2]| {
        model.update_markers(&data);
        let p = &model.groups[0].children[0];
        assert_eq!(p.marker.map(|(count, _)| count), parent);
        assert_eq!(counts(&p.children), children);
    };
    check(&mut model, vec![marker(Range::new(2, 4, 5, 4))], Some(0), [Some(1), None]);
    check(&mut model, vec![marker(Range::new(2, 4, 5, 4)), marker(Range::new(2, 6, 2, 8)), marker(Range::new(7, 6, 7, 8))], Some(0), [Some(2), Some(1)]);
    check(&mut model, vec![marker(Range::new(1, 4, 1, 11)), marker(Range::new(7, 6, 7, 8))], Some(1), [None, Some(1)]);
}

#[test]
fn outline_element_update_marker_multiple_groups() {
    let mut c2 = element("c2", Range::new(1, 1, 7, 1));
    c2.children = vec![element("c2.1", Range::new(1, 3, 2, 19)), element("c2.2", Range::new(4, 1, 6, 10))];
    let mut model = OutlineModel {
        groups: vec![
            OutlineGroup { id: "g1".into(), label: String::new(), children: vec![element("c1", Range::new(1, 1, 11, 1))] },
            OutlineGroup { id: "g2".into(), label: String::new(), children: vec![c2] },
        ],
        children: OutlineChildren::Groups,
    };
    model.update_markers(&[marker(Range::new(1, 1, 2, 8)), marker(Range::new(6, 1, 6, 98))]);
    assert_eq!(model.groups[0].children[0].marker.map(|(count, _)| count), Some(2));
    assert_eq!(counts(&model.groups[1].children[0].children), [Some(1), Some(1)]);
}

/// `create` and `_compact`: empty groups go, one group gives its elements to the root; ids dedupe by start and counter.
#[test]
fn groups_compact_and_ids_dedupe() {
    let one = OutlineModel::new(vec![("A".into(), vec![]), ("B".into(), vec![symbol(Range::new(1, 1, 1, 4), "x"), symbol(Range::new(2, 1, 2, 4), "x"), symbol(Range::new(2, 1, 2, 4), "x")])]);
    assert!(matches!(one.children, OutlineChildren::Elements));
    let ids: Vec<&str> = one.elements().map(|element| element.id.as_str()).collect();
    assert_eq!(ids, ["root/provider_1/x", "root/provider_1/x_2_1", "root/provider_1/x_2_1_0"]);
    let two = OutlineModel::new(vec![("A".into(), vec![symbol(Range::new(1, 1, 1, 4), "x")]), ("B".into(), vec![symbol(Range::new(1, 1, 1, 4), "y")])]);
    assert!(matches!(two.children, OutlineChildren::Groups));
    assert_eq!(two.groups.iter().map(|group| group.label.as_str()).collect::<Vec<_>>(), ["A", "B"]);
}

/// `getItemEnclosingPosition` and `asListOfDocumentSymbols`.
#[test]
fn enclosing_item_and_flat_list() {
    let mut outer = symbol(Range::new(1, 1, 5, 2), "outer");
    outer.children = vec![symbol(Range::new(2, 3, 3, 4), "inner")];
    let model = OutlineModel::new(vec![("A".into(), vec![outer, symbol(Range::new(7, 1, 7, 9), "last")])]);
    assert_eq!(model.item_enclosing_position((2, 5), None).map(|element| element.symbol.name.as_str()), Some("inner"));
    assert_eq!(model.item_enclosing_position((4, 1), None).map(|element| element.symbol.name.as_str()), Some("outer"));
    assert!(model.item_enclosing_position((6, 1), None).is_none());
    let list: Vec<(String, String)> = model.as_list_of_document_symbols().into_iter().map(|symbol| (symbol.name, symbol.container_name.unwrap())).collect();
    assert_eq!(list, [("outer".into(), String::new()), ("inner".into(), "outer".into()), ("last".into(), String::new())]);
    let path: Vec<&str> = model.path_to("root/provider_0/outer/inner").unwrap().iter().map(|element| element.symbol.name.as_str()).collect();
    assert_eq!(path, ["outer", "inner"]);
}
