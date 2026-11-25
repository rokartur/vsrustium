//! The views each search shows (extensionsViewlet.ts context keys and `when` clauses) and the gallery dates.

use super::*;

fn titles(value: &str, has_installed: bool) -> Vec<&'static str> {
    visible_views(value, has_installed, true).iter().map(|view| view.title).collect()
}

#[test]
fn default_views_depend_on_installed_extensions() {
    assert_eq!(titles("", false), ["Installed", "Popular", "Recommended"]);
    assert_eq!(titles("", true), ["Installed", "Recommended"]);
    assert_eq!(titles("@sort:name", true), ["Installed", "Recommended"]);
    assert_eq!(titles("@sort:updateDate", true), ["Installed"]);
}

#[test]
fn searches_pick_their_views() {
    assert_eq!(titles("yaml", true), ["Marketplace"]);
    assert_eq!(titles("@popular", true), ["Marketplace"]);
    assert_eq!(titles("category:themes", true), ["Marketplace"]);
    assert_eq!(titles("@installed", true), ["Installed"]);
    assert_eq!(titles("@installed yaml", true), ["Installed"]);
    assert_eq!(titles("@enabled", true), ["Enabled"]);
    assert_eq!(titles("@disabled", true), ["Disabled"]);
    assert_eq!(titles("@outdated", true), ["Available Updates"]);
    assert_eq!(titles("@updates", true), ["Available Updates", "Recently Updated"]);
    assert_eq!(titles("@builtin", true), ["Features", "Themes", "Programming Languages"]);
    assert_eq!(titles("@builtin json", true), ["Builtin"]);
    assert_eq!(titles("@recommended", true), ["Workspace Recommendations", "Other Recommendations"]);
    assert_eq!(visible_views("@recommended", true, false).iter().map(|view| view.title).collect::<Vec<_>>(), ["Other Recommendations"]);
    assert_eq!(titles("@restartrequired", true), ["Restart Required"]);
    assert_eq!(titles("@contribute:themes", true), ["Installed"]);
}

#[test]
fn views_turn_the_search_into_their_query() {
    let view = |value: &str, title: &str| visible_views(value, false, true).into_iter().find(|view| view.title == title).unwrap().view_query(value);
    assert_eq!(view("", "Installed"), "@installed");
    assert_eq!(view("@sort:name", "Installed"), "@sort:name @installed");
    assert_eq!(view("", "Popular"), "");
    assert_eq!(view("", "Recommended"), "@recommended:all");
    assert_eq!(view("@updates", "Available Updates"), "@outdated");
    assert_eq!(view("@updates", "Recently Updated"), "@recentlyUpdated");
    assert_eq!(view("@builtin", "Themes"), "@builtin category:themes");
}

#[test]
fn reads_gallery_dates() {
    assert_eq!(iso_millis("1970-01-01T00:00:00.000Z"), Some(0.));
    assert_eq!(iso_millis("2024-03-01T12:30:15.5Z"), Some(1_709_296_215_500.));
    assert_eq!(iso_millis("not a date"), None);
}
