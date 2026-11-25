//! The workbench's view of one extension (extensionsWorkbenchService.ts `Extension`): its installed copy, its
//! gallery entry, its enablement and runtime state; and the queries of the Extensions view over them
//! (extensionQuery.ts `Query`, extensionsViews.ts `filterLocal` and its `is*Query` tests).

use std::cmp::Ordering;
use std::sync::LazyLock;

use exthost::gallery::GalleryExtension;
use exthost::scan::ScannedExtension;
use regex::Regex;
use serde_json::Value;

use crate::command_palette::locale_compare;
use crate::extension_enablement::{EnablementState, Identifier};

/// `ExtensionState`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ExtensionState {
    Installing,
    Installed,
    Uninstalling,
    Uninstalled,
}

#[derive(Clone, Debug)]
pub struct Extension {
    pub local: Option<ScannedExtension>,
    pub gallery: Option<GalleryExtension>,
    pub state: ExtensionState,
    pub enablement: EnablementState,
    /// The host runs it (`extensionService.extensions` has it).
    pub running: bool,
    /// `runtimeState`: installed, removed or changed while the host runs an older copy, so a restart applies it.
    pub restart_required: bool,
}

impl Extension {
    fn manifest(&self) -> Option<&Value> {
        self.local.as_ref().map(|local| &local.description)
    }

    fn manifest_str(&self, key: &str) -> Option<&str> {
        self.manifest().and_then(|manifest| manifest[key].as_str())
    }

    pub fn identifier(&self) -> Identifier {
        if let Some(gallery) = &self.gallery {
            return Identifier { id: gallery.id.clone(), uuid: Some(gallery.uuid.clone()) };
        }
        let local = self.local.as_ref().expect("an extension is installed or from the gallery");
        Identifier { id: local.id.clone(), uuid: local.description["uuid"].as_str().map(str::to_owned) }
    }

    pub fn is_builtin(&self) -> bool {
        self.manifest().is_some_and(|manifest| manifest["isBuiltin"] == true)
    }

    pub fn name(&self) -> &str {
        match &self.gallery {
            Some(gallery) => &gallery.name,
            None => self.manifest_str("name").unwrap_or_default(),
        }
    }

    pub fn display_name(&self) -> &str {
        match &self.gallery {
            Some(gallery) if !gallery.display_name.is_empty() => &gallery.display_name,
            Some(gallery) => &gallery.name,
            None => self.manifest_str("displayName").unwrap_or(self.name()),
        }
    }

    pub fn publisher(&self) -> &str {
        match &self.gallery {
            Some(gallery) => &gallery.publisher,
            None => self.manifest_str("publisher").unwrap_or_default(),
        }
    }

    pub fn publisher_display_name(&self) -> &str {
        match &self.gallery {
            Some(gallery) if !gallery.publisher_display_name.is_empty() => &gallery.publisher_display_name,
            Some(gallery) => &gallery.publisher,
            None => self.manifest_str("publisherDisplayName").unwrap_or(self.publisher()),
        }
    }

    pub fn description(&self) -> &str {
        match &self.gallery {
            Some(gallery) => &gallery.description,
            None => self.manifest_str("description").unwrap_or_default(),
        }
    }

    /// The installed version, else the gallery's.
    pub fn version(&self) -> &str {
        self.manifest_str("version").unwrap_or_else(|| self.latest_version())
    }

    pub fn latest_version(&self) -> &str {
        match &self.gallery {
            Some(gallery) => &gallery.version,
            None => self.manifest_str("version").unwrap_or_default(),
        }
    }

    pub fn install_count(&self) -> Option<f64> {
        self.gallery.as_ref().map(|gallery| gallery.install_count)
    }

    pub fn rating(&self) -> Option<f64> {
        self.gallery.as_ref().map(|gallery| gallery.rating)
    }

    pub fn rating_count(&self) -> Option<f64> {
        self.gallery.as_ref().map(|gallery| gallery.rating_count)
    }

    pub fn installed_timestamp(&self) -> Option<u64> {
        self.manifest().and_then(|manifest| manifest["__metadata"]["installedTimestamp"].as_u64().or(manifest["installedTimestamp"].as_u64()))
    }

    pub fn updated(&self) -> bool {
        self.manifest().is_some_and(|manifest| manifest["__metadata"]["updated"] == true)
    }

    pub fn pre_release(&self) -> bool {
        self.manifest().is_some_and(|manifest| manifest["preRelease"] == true)
    }

    /// `outdated`: a newer gallery version of an installed user extension (built-ins update with the product).
    pub fn outdated(&self) -> bool {
        let (Some(gallery), Some(local)) = (&self.gallery, &self.local) else { return false };
        if self.is_builtin() || (!self.pre_release() && gallery.is_pre_release_version) {
            return false;
        }
        let local_version = local.description["version"].as_str().and_then(|version| semver::Version::parse(version).ok());
        let latest = semver::Version::parse(&gallery.version).ok();
        if let (Some(latest), Some(local_version)) = (&latest, &local_version)
            && latest > local_version
        {
            return true;
        }
        // `outdatedTargetPlatform`.
        let target = local.description["targetPlatform"].as_str().unwrap_or("undefined");
        !["undefined", "web"].contains(&target) && gallery.target_platform != "web" && target != gallery.target_platform && latest == local_version
    }

    /// `categories`: the installed manifest's unless outdated, else the gallery's.
    pub fn categories(&self) -> Vec<String> {
        if let Some(categories) = self.manifest().and_then(|manifest| manifest["categories"].as_array()).filter(|_| !self.outdated()) {
            return categories.iter().filter_map(Value::as_str).map(str::to_owned).collect();
        }
        self.gallery.as_ref().map(|gallery| gallery.categories.clone()).unwrap_or_default()
    }

    pub fn is_language_pack(&self) -> bool {
        self.manifest().is_some_and(|manifest| manifest["contributes"]["localizations"].as_array().is_some_and(|items| !items.is_empty()))
    }

    fn matches_name(&self, value: &str) -> bool {
        self.name().to_lowercase().contains(value) || self.display_name().to_lowercase().contains(value)
    }
}

/// extensionQuery.ts `Query`: the text and the `@sort:` it names.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Query {
    pub value: String,
    pub sort_by: String,
}

static SORT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"@sort:(\w+)(-\w*)?").unwrap());

impl Query {
    pub fn new(value: &str, sort_by: &str) -> Query {
        Query { value: value.trim().to_owned(), sort_by: sort_by.to_owned() }
    }

    /// `Query.parse`: the last `@sort:` wins and leaves the text.
    pub fn parse(value: &str) -> Query {
        let sort_by = SORT.captures_iter(value).last().map(|captures| captures[1].to_owned()).unwrap_or_default();
        Query::new(&SORT.replace_all(value, ""), &sort_by)
    }
}

impl std::fmt::Display for Query {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.value.is_empty(), self.sort_by.is_empty()) {
            (_, true) => write!(formatter, "{}", self.value),
            (true, false) => write!(formatter, "@sort:{}", self.sort_by),
            (false, false) => write!(formatter, "{} @sort:{}", self.value, self.sort_by),
        }
    }
}

/// `EXTENSION_CATEGORIES`.
pub const CATEGORIES: [&str; 20] = [
    "AI", "Azure", "Chat", "Data Science", "Debuggers", "Extension Packs", "Education", "Formatters", "Keymaps", "Language Packs", "Linters", "Machine Learning", "Notebooks", "Programming Languages", "SCM Providers", "Snippets", "Testing",
    "Themes", "Visualization", "Other",
];

/// The ids of the extension features registry (extensionFeaturesTab.ts and the contributions that register a
/// renderer), in registration order.
pub const FEATURES: [&str; 14] = ["activationEvents", "authentication", "commands", "configuration", "customEditors", "debuggers", "grammars", "iconThemes", "jsonValidation", "keybindings", "languages", "productIconThemes", "themes", "views"];

/// `NONE_CATEGORY`.
const NONE_CATEGORY: &str = "none";

/// The sort a view's query asks for (`IQueryOptions.sortBy` of `show`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortBy {
    InstallCount,
    WeightedRating,
    Title,
    PublishedDate,
    UpdateDate,
}

impl SortBy {
    pub fn parse(sort_by: &str) -> Option<SortBy> {
        match sort_by {
            "installs" => Some(SortBy::InstallCount),
            "rating" => Some(SortBy::WeightedRating),
            "name" => Some(SortBy::Title),
            "publishedDate" => Some(SortBy::PublishedDate),
            "updateDate" => Some(SortBy::UpdateDate),
            _ => None,
        }
    }
}

fn test(pattern: &str, text: &str) -> bool {
    Regex::new(pattern).expect("a query pattern").is_match(text)
}

pub fn is_local_extensions_query(query: &str, sort_by: &str) -> bool {
    is_installed_extensions_query(query)
        || is_search_installed_extensions_query(query)
        || is_outdated_extensions_query(query)
        || is_enabled_extensions_query(query)
        || is_disabled_extensions_query(query)
        || is_built_in_extensions_query(query)
        || is_search_built_in_extensions_query(query)
        || test(r"(?i)^@builtin:.+$", query.trim())
        || is_search_deprecated_extensions_query(query)
        || is_search_workspace_unsupported_extensions_query(query)
        || is_search_recently_updated_query(query)
        || is_restart_required_query(query)
        || is_search_extension_updates_query(query)
        || is_sort_installed_extensions_query(query, sort_by)
        || is_feature_extensions_query(query)
}

pub fn is_search_built_in_extensions_query(query: &str) -> bool {
    test(r"(?i)@builtin\s.+|.+\s@builtin", query)
}

pub fn is_built_in_extensions_query(query: &str) -> bool {
    test(r"(?i)^@builtin$", query.trim())
}

pub fn is_search_workspace_unsupported_extensions_query(query: &str) -> bool {
    test(r"(?i)^\s*@workspaceUnsupported(:(untrusted|virtual)(Partial)?)?(\s|$)", query)
}

pub fn is_installed_extensions_query(query: &str) -> bool {
    test(r"(?i)@installed$", query) && !test(r"(?i)@mcp|@agentPlugins", query)
}

pub fn is_search_installed_extensions_query(query: &str) -> bool {
    (test(r"(?i)@installed\s.", query) && !test(r"(?i)@mcp|@agentPlugins", query)) || is_feature_extensions_query(query)
}

pub fn is_outdated_extensions_query(query: &str) -> bool {
    test(r"(?i)@outdated", query)
}

pub fn is_enabled_extensions_query(query: &str) -> bool {
    test(r"(?i)@enabled", query) && !test(r"(?i)@builtin", query)
}

pub fn is_disabled_extensions_query(query: &str) -> bool {
    test(r"(?i)@disabled", query) && !test(r"(?i)@builtin", query)
}

pub fn is_search_deprecated_extensions_query(query: &str) -> bool {
    test(r"(?i)@deprecated", query)
}

pub fn is_recommended_extensions_query(query: &str) -> bool {
    test(r"(?i)^@recommended$", query.trim())
}

pub fn is_search_recommended_extensions_query(query: &str) -> bool {
    test(r"(?i)@recommended\s.+", query)
}

pub fn is_sort_installed_extensions_query(query: &str, sort_by: &str) -> bool {
    (!sort_by.is_empty() && query.is_empty()) || (sort_by.is_empty() && test(r"(?i)^@sort:\S*$", query))
}

pub fn is_search_popular_query(query: &str) -> bool {
    test(r"(?i)@popular", query)
}

pub fn is_search_recently_published_query(query: &str) -> bool {
    test(r"(?i)@recentlyPublished", query)
}

pub fn is_search_recently_updated_query(query: &str) -> bool {
    test(r"(?i)@recentlyUpdated", query)
}

pub fn is_restart_required_query(query: &str) -> bool {
    test(r"(?i)@restartrequired", query)
}

pub fn is_search_extension_updates_query(query: &str) -> bool {
    test(r"(?i)@updates", query)
}

pub fn is_sort_update_date_query(query: &str) -> bool {
    test(r"(?i)@sort:updateDate", query)
}

pub fn is_feature_extensions_query(query: &str) -> bool {
    test(r"(?i)@contribute:", query)
}

/// `isRecommendationsQuery`.
pub fn is_recommendations_query(query: &str) -> bool {
    test(r"(?i)@recommended:(workspace|keymaps|languages|remotes|all)|@exe:.+", query) || is_search_recommended_extensions_query(query) || is_recommended_extensions_query(query)
}

/// `normalizedQuery` of the viewlet's search box.
pub fn normalized_query(value: &str) -> String {
    value.trim().replace("@category", "category").replace("@tag:", "tag:").replace("@ext:", "ext:").replace("@featured", "featured")
}

/// `RECENT_UPDATE_DURATION`: 7 days.
const RECENT_UPDATE_DURATION: u64 = 7 * 24 * 60 * 60 * 1000;

/// `parseCategories`: `category:x` and `category:"x y"` taken out of the text; a `-` prefix excludes.
fn parse_categories(value: &str) -> (String, Vec<String>, Vec<String>) {
    static CATEGORY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"\bcategory:("([^"]*)"|([^"]\S*))(\s+|\b|$)"#).unwrap());
    let (mut included, mut excluded) = (Vec::new(), Vec::new());
    for captures in CATEGORY.captures_iter(value) {
        let entry = captures.get(3).or(captures.get(2)).map_or("", |m| m.as_str()).to_lowercase();
        let list = if entry.starts_with('-') { &mut excluded } else { &mut included };
        if !list.contains(&entry) {
            list.push(entry);
        }
    }
    (CATEGORY.replace_all(value, "").into_owned(), included, excluded)
}

fn matches_categories(extension: &Extension, included: &[String], excluded: &[String]) -> bool {
    if included.is_empty() && excluded.is_empty() {
        return true;
    }
    let categories: Vec<String> = extension.categories().iter().map(|category| category.to_lowercase()).collect();
    if categories.is_empty() {
        return included.iter().any(|category| category == NONE_CATEGORY);
    }
    if categories.iter().any(|category| excluded.contains(category)) {
        return false;
    }
    categories.iter().any(|category| included.contains(category))
}

fn strip(value: &str, pattern: &str) -> String {
    let value = Regex::new(pattern).expect("a query pattern").replace_all(value, "");
    SORT.replace_all(&value, "").trim().to_lowercase()
}

/// `filterLocal`: the installed extensions a local query shows, in its order. `now` is `Date.now()`.
pub fn filter_local<'a>(local: &'a [Extension], query: &Query, sort_by: Option<SortBy>, now: u64) -> Vec<&'a Extension> {
    let value = query.value.as_str();
    let include_builtin = test(r"(?i)@builtin", value);
    let (text, included, excluded) = parse_categories(value);
    let in_categories = |extension: &Extension| matches_categories(extension, &included, &excluded);
    let by_display_name = |a: &&Extension, b: &&Extension| locale_compare(a.display_name(), b.display_name());
    let mut result: Vec<&Extension>;
    if test(r"(?i)@installed", value) {
        let text = strip(&text, "@installed");
        let matching = |extension: &Extension| (extension.matches_name(&text) || extension.description().to_lowercase().contains(&text)) && in_categories(extension);
        if sort_by.is_some() {
            result = local.iter().filter(|extension| !extension.is_builtin() && matching(extension)).collect();
            return sort_extensions(result, sort_by);
        }
        result = local.iter().filter(|extension| (!extension.is_builtin() || extension.outdated() || extension.restart_required) && matching(extension)).collect();
        return installed_order(result);
    }
    if test(r"(?i)@outdated", value) {
        let text = strip(&text, "@outdated");
        result = local.iter().filter(|extension| extension.outdated() && extension.matches_name(&text) && in_categories(extension)).collect();
        result.sort_by(by_display_name);
        return sort_extensions(result, sort_by);
    }
    if test(r"(?i)@disabled", value) {
        let text = strip(&text, "(?i)@disabled|@builtin");
        result = local.iter().filter(|extension| (!include_builtin || extension.is_builtin()) && !extension.running && extension.matches_name(&text) && in_categories(extension)).collect();
        result.sort_by(by_display_name);
        return sort_extensions(result, sort_by);
    }
    if test(r"(?i)@enabled", value) {
        let text = strip(&text, "(?i)@enabled|@builtin");
        result = local.iter().filter(|extension| extension.is_builtin() == include_builtin && extension.running && extension.matches_name(&text) && in_categories(extension)).collect();
        result.sort_by(by_display_name);
        return sort_extensions(result, sort_by);
    }
    // Workspace trust and virtual workspaces are off in the port, and Open VSX serves no deprecation list
    // (the control manifest), so these lists are empty as upstream's are then.
    if test(r"(?i)@workspaceUnsupported|@deprecated", value) {
        return Vec::new();
    }
    if test(r"(?i)@recentlyUpdated", value) {
        let text = strip(&text, "@recentlyUpdated");
        result = local
            .iter()
            .filter(|extension| !extension.is_builtin() && !extension.outdated() && extension.updated() && extension.installed_timestamp().is_some_and(|installed| now.saturating_sub(installed) < RECENT_UPDATE_DURATION))
            .filter(|extension| extension.matches_name(&text) && in_categories(extension))
            .collect();
        return sort_extensions(result, sort_by.or(Some(SortBy::UpdateDate)));
    }
    if test(r"(?i)@restartrequired", value) {
        let text = strip(&text, "(?i)@restartrequired");
        result = local.iter().filter(|extension| extension.restart_required && extension.matches_name(&text) && in_categories(extension)).collect();
        return sort_extensions(result, sort_by);
    }
    if test(r"(?i)@contribute:", value) {
        let feature = value.replace("@contribute:", "");
        let feature = feature.trim().split(' ').next().unwrap_or_default();
        if !FEATURES.contains(&feature) {
            return Vec::new();
        }
        return local.iter().filter(|extension| extension.local.as_ref().is_some_and(|local| contributes(&local.description, feature))).collect();
    }
    if include_builtin {
        let text = strip(&text, "(?i)@builtin");
        result = local.iter().filter(|extension| extension.is_builtin() && extension.matches_name(&text) && in_categories(extension)).collect();
        return sort_extensions(result, sort_by);
    }
    Vec::new()
}

/// The feature renderer's `shouldRender`: the manifest contributes it (activation events: has any).
pub fn contributes(manifest: &Value, feature: &str) -> bool {
    let value = match feature {
        "activationEvents" => &manifest["activationEvents"],
        _ => &manifest["contributes"][feature],
    };
    match value {
        Value::Array(items) => !items.is_empty(),
        Value::Object(fields) => !fields.is_empty(),
        Value::Null => false,
        _ => true,
    }
}

/// `filterInstalledExtensions` default order: incompatible, outdated, restart required, the rest; in each group
/// the running ones first, then language packs, then by name.
fn installed_order(extensions: Vec<&Extension>) -> Vec<&Extension> {
    let default_sort = |a: &&Extension, b: &&Extension| -> Ordering {
        match (a.running, b.running) {
            (true, true) => locale_compare(a.display_name(), b.display_name()),
            (false, false) if a.is_language_pack() => Ordering::Less,
            (false, false) if b.is_language_pack() => Ordering::Greater,
            (false, false) => locale_compare(a.display_name(), b.display_name()),
            _ if (a.running && b.is_language_pack()) || (b.running && a.is_language_pack()) => locale_compare(a.display_name(), b.display_name()),
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
        }
    };
    let group = |extension: &Extension| match () {
        _ if extension.enablement == EnablementState::DisabledByInvalidExtension => 0,
        _ if extension.outdated() && extension.enablement.is_enabled() => 2,
        _ if extension.restart_required => 3,
        _ => 4,
    };
    let mut groups: [Vec<&Extension>; 5] = Default::default();
    for extension in extensions {
        groups[group(extension)].push(extension);
    }
    groups.into_iter().flat_map(|mut group| {
        group.sort_by(default_sort);
        group
    }).collect()
}

/// `sortExtensions`: installs and rating descending, update date newest first, else by name.
pub fn sort_extensions(mut extensions: Vec<&Extension>, sort_by: Option<SortBy>) -> Vec<&Extension> {
    match sort_by {
        Some(SortBy::InstallCount) => extensions.sort_by(|a, b| b.install_count().partial_cmp(&a.install_count()).unwrap_or(Ordering::Equal)),
        Some(SortBy::WeightedRating) => extensions.sort_by(|a, b| b.rating().partial_cmp(&a.rating()).unwrap_or(Ordering::Equal)),
        Some(SortBy::UpdateDate) => extensions.sort_by(|a, b| match (a.installed_timestamp(), b.installed_timestamp()) {
            (Some(a), Some(b)) => b.cmp(&a),
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (None, None) => Ordering::Equal,
        }),
        _ => extensions.sort_by(|a, b| locale_compare(a.display_name(), b.display_name())),
    }
    extensions
}

#[cfg(test)]
mod tests;
