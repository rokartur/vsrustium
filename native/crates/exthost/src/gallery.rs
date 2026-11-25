//! extensionGalleryService.ts against Open VSX: the extension query, the latest-version resource, the choice of a
//! compatible version, and the download of a VSIX with its signature archive (extensionDownloader.ts).

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, bail};
use serde_json::{Value, json};

use crate::extension_validator::{is_engine_valid, semver_version};
use crate::request::{Request, Response, request};
use crate::sign::{VerificationCode, verify};
use crate::zip::Archive;

/// product.json `extensionsGallery`: Code - OSS ships none, VSRustium uses Open VSX as VSCodium does.
pub const SERVICE_URL: &str = "https://open-vsx.org/vscode/gallery";
pub const ITEM_URL: &str = "https://open-vsx.org/vscode/item";
pub const PUBLISHER_URL: &str = "https://open-vsx.org/namespace";

const VSIX: &str = "Microsoft.VisualStudio.Services.VSIXPackage";
const MANIFEST: &str = "Microsoft.VisualStudio.Code.Manifest";
const SIGNATURE: &str = "Microsoft.VisualStudio.Services.VsixSignature";
/// Open VSX's asset with the PEM key of `.signature.sig`; the Marketplace has no such asset.
const PUBLIC_KEY: &str = "Microsoft.VisualStudio.Services.PublicKey";
const WEB_EXTENSION_TAG: &str = "__web_extension";
/// zip.ts `CorruptZipMessage`.
const CORRUPT_ZIP_MESSAGE: &str = "end of central directory record signature not found";

/// product.json `nameLong`.
const PRODUCT_NAME: &str = "Code - OSS";
/// `getExtensionBlockedMessage` for a gallery block.
const BLOCKED_MESSAGE: &str = "This extension has been blocked by the Extension Marketplace and must not be used.";

/// `getTargetPlatform(platform, arch)` on macOS.
pub const CURRENT_TARGET_PLATFORM: &str = if cfg!(target_arch = "aarch64") { "darwin-arm64" } else { "darwin-x64" };
const UNDEFINED: &str = "undefined";
const UNIVERSAL: &str = "universal";
const UNKNOWN: &str = "unknown";
const WEB: &str = "web";

#[derive(Debug, Clone, PartialEq)]
pub struct Asset {
    pub uri: String,
    pub fallback_uri: String,
}

/// `IGalleryExtension`.
#[derive(Debug, Clone, PartialEq)]
pub struct GalleryExtension {
    pub id: String,
    pub uuid: String,
    pub name: String,
    pub version: String,
    pub display_name: String,
    pub publisher_id: String,
    pub publisher: String,
    pub publisher_display_name: String,
    pub description: String,
    pub install_count: f64,
    pub rating: f64,
    pub rating_count: f64,
    pub categories: Vec<String>,
    pub tags: Vec<String>,
    pub release_date: String,
    pub last_updated: String,
    pub all_target_platforms: Vec<&'static str>,
    pub target_platform: &'static str,
    pub is_pre_release_version: bool,
    pub has_pre_release_version: bool,
    pub engine: String,
    pub dependencies: Vec<String>,
    pub extension_pack: Vec<String>,
    pub localized_languages: Vec<String>,
    pub download: Asset,
    pub manifest: Option<Asset>,
    pub readme: Option<Asset>,
    pub changelog: Option<Asset>,
    pub license: Option<Asset>,
    pub icon: Option<Asset>,
    pub signature: Option<Asset>,
    pub public_key: Option<String>,
    pub repository: Option<String>,
    pub preview: bool,
    pub is_signed: bool,
    pub blocked: bool,
    pub private: bool,
    pub detail_link: String,
    pub publisher_link: String,
}

impl GalleryExtension {
    fn display_name_or_id(&self) -> &str {
        if self.display_name.is_empty() { &self.id } else { &self.display_name }
    }
}

/// `TargetPlatformToString`.
fn target_platform_label(platform: &str) -> &str {
    match platform {
        "darwin-x64" => "Mac",
        "darwin-arm64" => "Mac Silicon",
        platform => platform,
    }
}

/// `IExtensionInfo`.
#[derive(Debug, Clone, Default)]
pub struct ExtensionInfo {
    pub id: String,
    pub uuid: Option<String>,
    pub version: Option<String>,
    pub pre_release: bool,
}

/// `IQueryOptions`.
#[derive(Debug, Clone, Default)]
pub struct QueryOptions {
    pub text: String,
    pub page_size: Option<usize>,
    /// The pager's page, from 1; 0 reads as 1.
    pub page_number: usize,
    pub sort_by: Option<SortBy>,
    pub sort_order: Option<u8>,
    pub include_pre_release: bool,
}

/// `SortBy` with the values of the gallery manifest's sorting capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortBy {
    NoneOrRelevance = 0,
    LastUpdatedDate = 1,
    Title = 2,
    PublisherName = 3,
    InstallCount = 4,
    AverageRating = 6,
    PublishedDate = 10,
    WeightedRating = 12,
}

/// `FilterType` values of the gallery manifest's filtering capability.
mod filter {
    pub const TAG: u8 = 1;
    pub const EXTENSION_ID: u8 = 4;
    pub const CATEGORY: u8 = 5;
    pub const EXTENSION_NAME: u8 = 7;
    pub const TARGET: u8 = 8;
    pub const FEATURED: u8 = 9;
    pub const SEARCH_TEXT: u8 = 10;
    pub const EXCLUDE_WITH_FLAGS: u8 = 12;
}

/// `Flag` values of the gallery manifest's flags capability.
mod flag {
    pub const INCLUDE_VERSIONS: u32 = 0x1;
    pub const INCLUDE_FILES: u32 = 0x2;
    pub const INCLUDE_CATEGORY_AND_TAGS: u32 = 0x4;
    pub const INCLUDE_VERSION_PROPERTIES: u32 = 0x10;
    pub const EXCLUDE_NON_VALIDATED: u32 = 0x20;
    pub const INCLUDE_ASSET_URI: u32 = 0x80;
    pub const INCLUDE_STATISTICS: u32 = 0x100;
    pub const INCLUDE_LATEST_VERSION_ONLY: u32 = 0x200;
    pub const UNPUBLISHED: u32 = 0x1000;
}

/// `Query`.
#[derive(Debug, Clone)]
struct Query {
    page_number: usize,
    page_size: usize,
    sort_by: SortBy,
    sort_order: u8,
    flags: u32,
    criteria: Vec<(u8, Option<String>)>,
}

impl Query {
    fn new(page_size: usize) -> Self {
        Query { page_number: 1, page_size, sort_by: SortBy::NoneOrRelevance, sort_order: 0, flags: 0, criteria: Vec::new() }
    }

    fn with_filter(mut self, filter_type: u8, values: &[String]) -> Self {
        if values.is_empty() {
            self.criteria.push((filter_type, None));
        }
        self.criteria.extend(values.iter().map(|value| (filter_type, Some(value.clone()))));
        self
    }

    /// The body of `queryRawGalleryExtensions`, with its non-validated, target and unpublished filters.
    fn body(&self) -> Value {
        let mut criteria: Vec<Value> = self.criteria.iter().map(|(filter_type, value)| json!({ "filterType": filter_type, "value": value })).collect();
        criteria.push(json!({ "filterType": filter::TARGET, "value": "Microsoft.VisualStudio.Code" }));
        criteria.push(json!({ "filterType": filter::EXCLUDE_WITH_FLAGS, "value": flag::UNPUBLISHED.to_string() }));
        json!({
            "filters": [{ "criteria": criteria, "pageNumber": self.page_number, "pageSize": self.page_size, "sortBy": self.sort_by as u8, "sortOrder": self.sort_order }],
            "assetTypes": [],
            "flags": self.flags | flag::EXCLUDE_NON_VALIDATED,
        })
    }
}

/// `VersionKind`, or a specific version.
#[derive(Debug, Clone, PartialEq)]
enum VersionKind {
    Release,
    Prerelease,
    Latest,
    Exact(String),
}

struct VersionCriteria {
    target_platform: &'static str,
    compatible: bool,
    version: VersionKind,
}

/// `ExtensionsCriteria`: the pre-release choice is per extension id for `getExtensions`, one value for `query`.
struct ExtensionsCriteria<'a> {
    target_platform: &'static str,
    compatible: bool,
    include_pre_release: &'a dyn Fn(&str) -> bool,
    versions: &'a [ExtensionInfo],
}

pub struct Gallery {
    pub service_url: String,
    pub product_version: String,
}

impl Gallery {
    pub fn open_vsx(product_version: &str) -> Self {
        Gallery { service_url: SERVICE_URL.into(), product_version: product_version.into() }
    }

    /// `resolveMarketplaceHeaders` without the telemetry ids.
    fn headers(&self) -> [(&'static str, String); 2] {
        [("X-Market-Client-Id", format!("VSCode {}", self.product_version)), ("User-Agent", format!("VSCode {} (Code - OSS)", self.product_version))]
    }

    fn get(&self, url: &str, accept: &str) -> anyhow::Result<Response> {
        let [client, agent] = self.headers();
        let headers = [(client.0, client.1.as_str()), (agent.0, agent.1.as_str()), ("Accept", accept), ("Content-Type", "application/json")];
        request(&Request { url, method: "GET", headers: &headers, body: None, timeout: Duration::from_secs(60) })
    }

    /// `query`: the text's `category:`, `tag:` and `featured` become filters; returns the page and the total.
    pub fn query(&self, options: &QueryOptions) -> anyhow::Result<(Vec<GalleryExtension>, usize)> {
        let mut query = Query::new(options.page_size.unwrap_or(50));
        query.page_number = options.page_number.max(1);
        let mut text = options.text.clone();
        if text.is_empty() {
            query.sort_by = SortBy::InstallCount;
        } else {
            let (rest, filters) = take_query_filters(&text);
            for (filter_type, value) in filters {
                query = query.with_filter(filter_type, &value.into_iter().collect::<Vec<_>>());
            }
            text = rest.trim().chars().take(200).collect();
            if !text.is_empty() {
                query = query.with_filter(filter::SEARCH_TEXT, &[text]);
            }
            query.sort_by = SortBy::NoneOrRelevance;
        }
        if let Some(sort_by) = options.sort_by {
            query.sort_by = sort_by;
        }
        if let Some(sort_order) = options.sort_order {
            query.sort_order = sort_order;
        }
        let include_pre_release = options.include_pre_release;
        let criteria = ExtensionsCriteria { target_platform: CURRENT_TARGET_PLATFORM, compatible: false, include_pre_release: &|_| include_pre_release, versions: &[] };
        self.query_gallery_extensions(query, &criteria)
    }

    /// `getExtensions`: the latest-version resource for ids with a uuid and no version, the query for the rest
    /// and for what the resource could not answer.
    pub fn get_extensions(&self, infos: &[ExtensionInfo], compatible: bool) -> anyhow::Result<Vec<GalleryExtension>> {
        let mut result = Vec::new();
        let mut to_query = Vec::new();
        for info in infos {
            if !is_extension_identifier(&info.id) {
                continue;
            }
            if info.version.is_some() || info.uuid.is_none() {
                to_query.push(info.clone());
                continue;
            }
            match self.latest_gallery_extension(info, compatible) {
                Ok(Some(extension)) => result.push(extension),
                Ok(None) => to_query.push(info.clone()),
                Err(error) => {
                    eprintln!("[gallery] Error while getting the latest version for the extension {}. {error:#}", info.id);
                    to_query.push(info.clone());
                }
            }
        }
        if !to_query.is_empty() {
            result.extend(self.get_extensions_using_query_api(&to_query, compatible)?);
        }
        let uuids: Vec<&str> = result.iter().map(|extension| extension.uuid.as_str()).collect();
        let by_name: Vec<ExtensionInfo> =
            infos.iter().filter(|info| info.uuid.as_deref().is_some_and(|uuid| !uuids.contains(&uuid))).map(|info| ExtensionInfo { uuid: None, ..info.clone() }).collect();
        if !by_name.is_empty() {
            result.extend(self.get_extensions_using_query_api(&by_name, compatible)?);
        }
        Ok(result)
    }

    /// `getCompatibleExtension`: the extension itself when it fits this product, else its latest compatible version.
    pub fn get_compatible_extension(&self, extension: &GalleryExtension, include_pre_release: bool) -> anyhow::Result<Option<GalleryExtension>> {
        if extension.blocked {
            return Ok(Some(extension.clone()));
        }
        let kind = if include_pre_release { VersionKind::Latest } else { VersionKind::Release };
        if is_version_kind(extension.is_pre_release_version, &extension.version, &kind)
            && is_target_platform_compatible(extension.target_platform, &extension.all_target_platforms, CURRENT_TARGET_PLATFORM)
            && is_engine_valid(&extension.engine, &self.product_version, None)
        {
            return Ok(Some(extension.clone()));
        }
        let criteria = ExtensionsCriteria { target_platform: CURRENT_TARGET_PLATFORM, compatible: true, include_pre_release: &|_| include_pre_release, versions: &[] };
        let mut query = Query::new(1).with_filter(filter::EXTENSION_ID, std::slice::from_ref(&extension.uuid));
        query.flags = flag::INCLUDE_VERSIONS;
        Ok(self.query_gallery_extensions(query, &criteria)?.0.into_iter().next())
    }

    /// `extensionsWorkbenchService._install(id)` then `checkAndGetCompatibleVersion`: the gallery version to download.
    pub fn installable_extension(&self, id: &str, version: Option<&str>, pre_release: bool) -> anyhow::Result<GalleryExtension> {
        let info = ExtensionInfo { id: id.into(), uuid: None, version: version.map(Into::into), pre_release };
        let Some(extension) = self.get_extensions(&[info], false)?.into_iter().next() else {
            match version {
                Some(version) => bail!("The extension '{id}' cannot be installed because the requested version '{version}' was not found."),
                None => bail!("The extension '{id}' cannot be installed because it was not found."),
            }
        };
        if extension.blocked {
            bail!("{BLOCKED_MESSAGE}");
        }
        let all = &extension.all_target_platforms;
        if !all.iter().any(|platform| is_target_platform_compatible(platform, all, CURRENT_TARGET_PLATFORM)) {
            bail!("The '{}' extension is not available in {PRODUCT_NAME} for the {} platform.", extension.id, target_platform_label(CURRENT_TARGET_PLATFORM));
        }
        let compatible = self.compatible_version(&extension, version.is_some(), pre_release)?;
        let Some(compatible) = compatible else {
            if !pre_release && extension.has_pre_release_version && extension.is_pre_release_version {
                bail!("Can't install release version of '{}' extension because it has no release version.", extension.display_name_or_id());
            }
            bail!("Can't install '{}' extension because it is not compatible with the current version of {PRODUCT_NAME} (version {}).", extension.id, self.product_version);
        };
        if compatible.blocked {
            bail!("{BLOCKED_MESSAGE}");
        }
        Ok(compatible)
    }

    /// `getCompatibleVersion`.
    fn compatible_version(&self, extension: &GalleryExtension, same_version: bool, pre_release: bool) -> anyhow::Result<Option<GalleryExtension>> {
        let info = |version: Option<String>, pre_release| ExtensionInfo { id: extension.id.clone(), uuid: Some(extension.uuid.clone()), version, pre_release };
        if !same_version
            && extension.has_pre_release_version
            && extension.is_pre_release_version != pre_release
            && let Some(found) = self.get_extensions(&[info(None, pre_release)], true)?.into_iter().next()
        {
            return Ok(Some(found));
        }
        let kind = if pre_release { VersionKind::Latest } else { VersionKind::Release };
        if !extension.blocked
            && is_version_kind(extension.is_pre_release_version, &extension.version, &kind)
            && is_target_platform_compatible(extension.target_platform, &extension.all_target_platforms, CURRENT_TARGET_PLATFORM)
            && is_engine_valid(&extension.engine, &self.product_version, None)
        {
            return Ok(Some(extension.clone()));
        }
        if same_version {
            return Ok(self.get_extensions(&[info(Some(extension.version.clone()), false)], true)?.into_iter().next());
        }
        self.get_compatible_extension(extension, pre_release)
    }

    fn get_extensions_using_query_api(&self, infos: &[ExtensionInfo], compatible: bool) -> anyhow::Result<Vec<GalleryExtension>> {
        let ids: Vec<String> = infos.iter().filter_map(|info| info.uuid.clone()).collect();
        let names: Vec<String> = infos.iter().filter(|info| info.uuid.is_none()).map(|info| info.id.clone()).collect();
        let mut query = Query::new(infos.len());
        if !ids.is_empty() {
            query = query.with_filter(filter::EXTENSION_ID, &ids);
        }
        if !names.is_empty() {
            query = query.with_filter(filter::EXTENSION_NAME, &names);
        }
        let versions: Vec<ExtensionInfo> = infos.iter().filter(|info| info.version.is_some()).cloned().collect();
        let pre_release = |id: &str| infos.iter().any(|info| info.id.eq_ignore_ascii_case(id) && info.pre_release);
        let criteria = ExtensionsCriteria { target_platform: CURRENT_TARGET_PLATFORM, compatible, include_pre_release: &pre_release, versions: &versions };
        Ok(self.query_gallery_extensions(query, &criteria)?.0)
    }

    /// `queryGalleryExtensions`: latest versions first, then all versions of the extensions whose latest does not fit.
    fn query_gallery_extensions(&self, mut query: Query, criteria: &ExtensionsCriteria) -> anyhow::Result<(Vec<GalleryExtension>, usize)> {
        if query.flags & flag::INCLUDE_VERSIONS == 0 {
            query.flags |= flag::INCLUDE_LATEST_VERSION_ONLY;
        }
        if !criteria.versions.is_empty() {
            query.flags = (query.flags & !flag::INCLUDE_LATEST_VERSION_ONLY) | flag::INCLUDE_VERSIONS;
        }
        query.flags |= flag::INCLUDE_ASSET_URI | flag::INCLUDE_CATEGORY_AND_TAGS | flag::INCLUDE_FILES | flag::INCLUDE_STATISTICS | flag::INCLUDE_VERSION_PROPERTIES;
        let (raw_extensions, total) = self.query_raw_gallery_extensions(&query)?;
        let has_all_versions = query.flags & flag::INCLUDE_LATEST_VERSION_ONLY == 0;

        let mut result: Vec<(usize, GalleryExtension)> = Vec::new();
        let mut need_all_versions: Vec<(String, usize)> = Vec::new();
        for (index, mut raw) in raw_extensions.into_iter().enumerate() {
            let id = raw_id(&raw);
            let all_target_platforms = all_target_platforms(&raw);
            let include_pre_release = (criteria.include_pre_release)(&id);
            if criteria.compatible && is_not_web_extension_in_web_target_platform(&all_target_platforms, criteria.target_platform) {
                continue;
            }
            let version = match criteria.versions.iter().find(|info| info.id.eq_ignore_ascii_case(&id)).and_then(|info| info.version.clone()) {
                Some(version) => VersionKind::Exact(version),
                None if include_pre_release => VersionKind::Latest,
                None => VersionKind::Release,
            };
            let version_criteria = VersionCriteria { target_platform: criteria.target_platform, compatible: criteria.compatible, version };
            let versions = std::mem::take(&mut raw["versions"]);
            let mut versions = match versions {
                Value::Array(versions) => versions,
                _ => Vec::new(),
            };
            let chosen = self.valid_version(&id, &mut versions, &version_criteria, &all_target_platforms);
            raw["versions"] = Value::Array(versions.clone());
            let extension = chosen.map(|version| to_extension(&raw, &versions[version], &all_target_platforms));
            if has_all_versions {
                if let Some(extension) = extension {
                    result.push((index, extension));
                }
                continue;
            }
            match extension {
                Some(extension)
                    if !(extension.is_pre_release_version && !include_pre_release)
                        && !(!extension.is_pre_release_version && extension.target_platform != criteria.target_platform && extension.has_pre_release_version) =>
                {
                    result.push((index, extension))
                }
                _ => need_all_versions.push((raw["extensionId"].as_str().unwrap_or_default().to_owned(), index)),
            }
        }
        if !need_all_versions.is_empty() {
            let ids: Vec<String> = need_all_versions.iter().map(|(uuid, _)| uuid.clone()).collect();
            let mut all_versions = Query::new(ids.len()).with_filter(filter::EXTENSION_ID, &ids);
            all_versions.flags = (query.flags & !flag::INCLUDE_LATEST_VERSION_ONLY) | flag::INCLUDE_VERSIONS;
            for extension in self.query_gallery_extensions(all_versions, criteria)?.0 {
                if let Some((_, index)) = need_all_versions.iter().find(|(uuid, _)| *uuid == extension.uuid) {
                    result.push((*index, extension));
                }
            }
        }
        result.sort_by_key(|(index, _)| *index);
        Ok((result.into_iter().map(|(_, extension)| extension).collect(), total))
    }

    /// `queryRawGalleryExtensions`: a 4xx answers no extensions.
    fn query_raw_gallery_extensions(&self, query: &Query) -> anyhow::Result<(Vec<Value>, usize)> {
        let body = query.body().to_string();
        let [client, agent] = self.headers();
        let headers = [(client.0, client.1.as_str()), (agent.0, agent.1.as_str()), ("Content-Type", "application/json"), ("Accept", "application/json;api-version=3.0-preview.1")];
        let url = format!("{}/extensionquery", self.service_url);
        let response = request(&Request { url: &url, method: "POST", headers: &headers, body: Some(body.as_bytes()), timeout: Duration::from_secs(60) })?;
        if (400..500).contains(&response.status) {
            return Ok((Vec::new(), 0));
        }
        let result = response.as_json()?;
        let first = &result["results"][0];
        let total = first["resultMetadata"]
            .as_array()
            .and_then(|metadata| metadata.iter().find(|item| item["metadataType"] == "ResultCount"))
            .and_then(|item| item["metadataItems"].as_array()?.iter().find(|item| item["name"] == "TotalCount")?["count"].as_u64())
            .unwrap_or(0) as usize;
        Ok((first["extensions"].as_array().cloned().unwrap_or_default(), total))
    }

    /// `getLatestGalleryExtension`: `None` when the resource has no fitting version and the query should answer.
    fn latest_gallery_extension(&self, info: &ExtensionInfo, compatible: bool) -> anyhow::Result<Option<GalleryExtension>> {
        let (publisher, name) = info.id.split_once('.').context("invalid extension id")?;
        let url = format!("{}/vscode/{publisher}/{name}/latest", self.service_url);
        let response = self.get(&url, "application/json;api-version=7.2-preview")?;
        if response.status == 404 {
            return Ok(None);
        }
        if response.status != 200 {
            bail!("Unexpected HTTP response: {}", response.status);
        }
        let mut raw = response.as_json()?;
        let Some(versions) = raw["versions"].as_array() else { return Ok(None) };
        if versions.iter().any(|version| !version["files"].is_array()) {
            return Ok(None);
        }
        let all_target_platforms = all_target_platforms(&raw);
        let mut latest = filter_latest_extension_versions_for_target_platform(versions, CURRENT_TARGET_PLATFORM, &all_target_platforms);
        let id = raw_id(&raw);
        let criteria = |version| VersionCriteria { target_platform: CURRENT_TARGET_PLATFORM, compatible, version };
        let requested = if info.pre_release { VersionKind::Prerelease } else { VersionKind::Release };
        let found = self.valid_version(&id, &mut latest, &criteria(requested), &all_target_platforms);
        let chosen = if !info.pre_release {
            found
        } else {
            let release = self.valid_version(&id, &mut latest, &criteria(VersionKind::Release), &all_target_platforms);
            match (found, release) {
                (Some(pre_release), Some(release)) => Some(if version_gt(&latest[release], &latest[pre_release]) { release } else { pre_release }),
                (pre_release, Some(release)) if compatible => {
                    let any = self.valid_version(&id, &mut latest, &VersionCriteria { compatible: false, ..criteria(VersionKind::Prerelease) }, &all_target_platforms);
                    match any {
                        Some(any) if !version_gt(&latest[release], &latest[any]) => pre_release,
                        _ => Some(release),
                    }
                }
                (pre_release, release) => pre_release.or(release),
            }
        };
        // `versions[0]` of the resource is the latest of all, as `toExtension` reads it.
        let has_pre_release_version = versions.first().is_some_and(is_pre_release_version);
        raw["versions"] = Value::Array(latest.clone());
        Ok(chosen.map(|index| {
            let mut extension = to_extension(&raw, &latest[index], &all_target_platforms);
            extension.has_pre_release_version = has_pre_release_version;
            extension
        }))
    }

    /// `getValidRawGalleryExtensionVersion`: the index of the first version that `isValidVersion` accepts.
    fn valid_version(&self, id: &str, versions: &mut [Value], criteria: &VersionCriteria, all_target_platforms: &[&'static str]) -> Option<usize> {
        sort_extension_versions(versions, criteria.target_platform);
        if criteria.compatible && is_not_web_extension_in_web_target_platform(all_target_platforms, criteria.target_platform) {
            return None;
        }
        for (index, version) in versions.iter_mut().enumerate() {
            if criteria.compatible && property(version, "Microsoft.VisualStudio.Code.Engine").is_none_or(str::is_empty) {
                self.set_engine_if_not_exists(id, version);
            }
            if self.is_valid_version(version, criteria, all_target_platforms) {
                return Some(index);
            }
            if let VersionKind::Exact(exact) = &criteria.version
                && version["version"] == exact.as_str()
            {
                return None;
            }
        }
        if matches!(criteria.version, VersionKind::Exact(_)) || criteria.compatible {
            return None;
        }
        versions.iter().position(|version| !is_blocked(version))
    }

    /// `isValidVersion` without `extensionProperties` (product.json has none) and without `extensions.allowed`.
    fn is_valid_version(&self, version: &Value, criteria: &VersionCriteria, all_target_platforms: &[&'static str]) -> bool {
        if is_blocked(version) {
            return false;
        }
        if !is_version_kind(is_pre_release_version(version), version["version"].as_str().unwrap_or_default(), &criteria.version) {
            return false;
        }
        if !is_target_platform_compatible(target_platform_of(version), all_target_platforms, criteria.target_platform) {
            return false;
        }
        if criteria.compatible {
            let engine = property(version, "Microsoft.VisualStudio.Code.Engine").unwrap_or_default();
            if engine.is_empty() || !is_engine_valid(engine, &self.product_version, None) {
                return false;
            }
        }
        true
    }

    /// `setEngineIfNotExists`: the engine from the version's `package.json` asset.
    fn set_engine_if_not_exists(&self, id: &str, version: &mut Value) {
        let Some(asset) = version_asset(version, MANIFEST) else { return };
        let manifest = match self.get_asset(&asset).and_then(|response| response.as_json()) {
            Ok(manifest) => manifest,
            Err(error) => return eprintln!("[gallery] Error while getting the engine for the version {} of {id}. {error:#}", version["version"]),
        };
        let Some(engine) = manifest["engines"]["vscode"].as_str() else { return };
        if !version["properties"].is_array() {
            version["properties"] = json!([]);
        }
        version["properties"].as_array_mut().expect("set above").push(json!({ "key": "Microsoft.VisualStudio.Code.Engine", "value": engine }));
    }

    /// `getAsset`: the asset's uri, then its fallback uri.
    fn get_asset(&self, asset: &Asset) -> anyhow::Result<Response> {
        let first = self.get(&asset.uri, "*/*");
        match first {
            Ok(response) if response.status == 200 => Ok(response),
            _ => {
                let response = self.get(&asset.fallback_uri, "*/*")?;
                if response.status != 200 {
                    bail!("Expected 200, got back {} instead.", response.status);
                }
                Ok(response)
            }
        }
    }

    pub fn get_readme(&self, extension: &GalleryExtension) -> anyhow::Result<String> {
        self.get_text(extension.readme.as_ref())
    }

    pub fn get_changelog(&self, extension: &GalleryExtension) -> anyhow::Result<String> {
        self.get_text(extension.changelog.as_ref())
    }

    pub fn get_manifest(&self, extension: &GalleryExtension) -> anyhow::Result<Option<Value>> {
        let Some(asset) = &extension.manifest else { return Ok(None) };
        Ok(Some(self.get_asset(asset)?.as_json()?))
    }

    fn get_text(&self, asset: Option<&Asset>) -> anyhow::Result<String> {
        let Some(asset) = asset else { return Ok(String::new()) };
        Ok(self.get_asset(asset)?.as_text()?.unwrap_or_default())
    }

    /// `ExtensionsDownloader.download` into `folder` (`CachedExtensionVSIXs`) and the signature check of
    /// `downloadExtension`. A VSIX that fails the check is deleted.
    pub fn download(&self, extension: &GalleryExtension, operation: Operation, folder: &Path, verify_signature: bool) -> anyhow::Result<PathBuf> {
        let (location, code) = self.download_and_verify(extension, operation, folder, verify_signature)?;
        check_verification(code, extension.private).inspect_err(|_| drop(std::fs::remove_file(&location)))?;
        Ok(location)
    }

    /// `None` is "not verified": `extensions.verifySignature` is off.
    fn download_and_verify(&self, extension: &GalleryExtension, operation: Operation, folder: &Path, verify_signature: bool) -> anyhow::Result<(PathBuf, Option<VerificationCode>)> {
        std::fs::create_dir_all(folder)?;
        let location = folder.join(extension_key(extension).to_lowercase());
        let vsix = match std::fs::read(&location) {
            Ok(vsix) => vsix,
            Err(_) => {
                let vsix = self.download_vsix(extension, operation).context("Download")?;
                std::fs::write(&location, &vsix)?;
                vsix
            }
        };
        if !verify_signature {
            return Ok((location, None));
        }
        let (Some(signature), Some(public_key)) = (&extension.signature, &extension.public_key) else {
            return Ok((location, Some(VerificationCode::NotSigned)));
        };
        let code = self.download_signature_and_verify(&vsix, signature, public_key);
        if !matches!(code, Ok(code) if code != VerificationCode::PackageIsInvalidZip && code != VerificationCode::SignatureArchiveIsInvalidZip) {
            let _ = std::fs::remove_file(&location);
        }
        match code? {
            VerificationCode::PackageIsInvalidZip | VerificationCode::SignatureArchiveIsInvalidZip => bail!("{CORRUPT_ZIP_MESSAGE}"),
            code => Ok((location, Some(code))),
        }
    }

    fn download_signature_and_verify(&self, vsix: &[u8], signature: &Asset, public_key: &str) -> anyhow::Result<VerificationCode> {
        let archive = self.get_asset(signature).context("DownloadSignature")?.body;
        let key = self.get(public_key, "*/*")?.as_text()?.unwrap_or_default();
        Ok(verify(vsix, &archive, &key))
    }

    /// `downloadVSIX`: two attempts, each checked to be a zip with `extension/package.json`.
    fn download_vsix(&self, extension: &GalleryExtension, operation: Operation) -> anyhow::Result<Vec<u8>> {
        let parameter = match operation {
            Operation::Install => "install",
            Operation::Update => "update",
        };
        let with_parameter = |uri: &str| format!("{uri}{}{parameter}=true", if uri.contains('?') { '&' } else { '?' });
        let asset = Asset { uri: with_parameter(&extension.download.uri), fallback_uri: with_parameter(&extension.download.fallback_uri) };
        let mut last_error = None;
        for _ in 0..2 {
            match self.get_asset(&asset).and_then(|response| {
                Archive::open(response.body.clone())?.entry("extension/package.json").context("extension/package.json not found in zip")?;
                Ok(response.body)
            }) {
                Ok(vsix) => return Ok(vsix),
                Err(error) => last_error = Some(error),
            }
        }
        Err(last_error.expect("two attempts ran"))
    }
}

/// The check in `downloadExtension`: the gallery manifest says `allPublicRepositorySigned`, so anything but
/// `Success` refuses the install, `NotSigned` too unless the extension is private.
fn check_verification(code: Option<VerificationCode>, private: bool) -> anyhow::Result<()> {
    match code {
        None | Some(VerificationCode::Success) => Ok(()),
        Some(VerificationCode::NotSigned) if private => Ok(()),
        Some(code) => bail!("Signature verification failed with '{}' error.", code.name()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Install,
    Update,
}

/// `ExtensionKey.toString()`: `<id>-<version>`, with `-<targetPlatform>` unless the gallery names none.
pub fn extension_key(extension: &GalleryExtension) -> String {
    match extension.target_platform {
        UNDEFINED => format!("{}-{}", extension.id, extension.version),
        platform => format!("{}-{}-{platform}", extension.id, extension.version),
    }
}

/// `EXTENSION_IDENTIFIER_REGEX`.
pub fn is_extension_identifier(id: &str) -> bool {
    let part = |part: &str| part.chars().next().is_some_and(|first| first.is_ascii_alphanumeric()) && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    id.split_once('.').is_some_and(|(publisher, name)| part(publisher) && part(name))
}

/// The `category:`, `tag:` and `featured` words of a query text as filters, and the text left.
fn take_query_filters(text: &str) -> (String, Vec<(u8, Option<String>)>) {
    let mut filters = Vec::new();
    let mut rest = Vec::new();
    let mut words = text.split(' ').peekable();
    while let Some(word) = words.next() {
        let (filter_type, value) = if let Some(value) = word.strip_prefix("category:") {
            (filter::CATEGORY, value)
        } else if let Some(value) = word.strip_prefix("tag:") {
            (filter::TAG, value)
        } else if word == "featured" {
            filters.push((filter::FEATURED, None));
            continue;
        } else {
            rest.push(word.to_owned());
            continue;
        };
        let value = match value.strip_prefix('"') {
            Some(quoted) if quoted.ends_with('"') => quoted.trim_end_matches('"').to_owned(),
            Some(quoted) => {
                let mut value = quoted.to_owned();
                for next in words.by_ref() {
                    value.push(' ');
                    value.push_str(next.trim_end_matches('"'));
                    if next.ends_with('"') {
                        break;
                    }
                }
                value
            }
            None => value.to_owned(),
        };
        filters.push((filter_type, Some(value)));
    }
    (rest.join(" "), filters)
}

fn raw_id(raw: &Value) -> String {
    format!("{}.{}", raw["publisher"]["publisherName"].as_str().unwrap_or_default(), raw["extensionName"].as_str().unwrap_or_default()).to_lowercase()
}

fn property<'a>(version: &'a Value, key: &str) -> Option<&'a str> {
    version["properties"].as_array()?.iter().find(|property| property["key"] == key)?["value"].as_str()
}

fn is_pre_release_version(version: &Value) -> bool {
    property(version, "Microsoft.VisualStudio.Code.PreRelease") == Some("true")
}

/// `isBlocked`: a `blocking` object or a `blocked` token in the version's flags.
fn is_blocked(version: &Value) -> bool {
    !version["blocking"].is_null() || version["flags"].as_str().is_some_and(|flags| flags.split(',').any(|flag| flag.trim().eq_ignore_ascii_case("blocked")))
}

fn is_version_kind(is_pre_release: bool, version: &str, kind: &VersionKind) -> bool {
    match kind {
        VersionKind::Exact(exact) => version == exact,
        VersionKind::Release => !is_pre_release,
        VersionKind::Prerelease => is_pre_release,
        VersionKind::Latest => true,
    }
}

fn version_gt(a: &Value, b: &Value) -> bool {
    let parse = |version: &Value| semver_version(version["version"].as_str().unwrap_or_default());
    matches!((parse(a), parse(b)), (Some(a), Some(b)) if a > b)
}

/// `toTargetPlatform`.
fn to_target_platform(platform: &str) -> &'static str {
    const KNOWN: [&str; 14] =
        ["win32-x64", "win32-arm64", "linux-x64", "linux-arm64", "linux-armhf", "alpine-x64", "alpine-arm64", "darwin-x64", "darwin-arm64", "web", "universal", "undefined", "unknown", "win32-ia32"];
    KNOWN.into_iter().find(|known| *known == platform).unwrap_or(UNKNOWN)
}

/// `getTargetPlatformForExtensionVersion`.
fn target_platform_of(version: &Value) -> &'static str {
    match version["targetPlatform"].as_str() {
        Some(platform) if !platform.is_empty() => to_target_platform(platform),
        _ => UNDEFINED,
    }
}

/// `getAllTargetPlatforms`: `web` only for extensions tagged as web extensions.
fn all_target_platforms(raw: &Value) -> Vec<&'static str> {
    let mut platforms: Vec<&'static str> = Vec::new();
    for version in raw["versions"].as_array().into_iter().flatten() {
        let platform = target_platform_of(version);
        if !platforms.contains(&platform) {
            platforms.push(platform);
        }
    }
    let is_web_extension = raw["tags"].as_array().is_some_and(|tags| tags.iter().any(|tag| tag == WEB_EXTENSION_TAG));
    platforms.retain(|platform| *platform != WEB);
    if is_web_extension {
        platforms.push(WEB);
    }
    platforms
}

fn is_not_web_extension_in_web_target_platform(all_target_platforms: &[&str], product_target_platform: &str) -> bool {
    product_target_platform == WEB && !all_target_platforms.contains(&WEB)
}

/// `isTargetPlatformCompatible`.
fn is_target_platform_compatible(extension_target_platform: &str, all_target_platforms: &[&str], product_target_platform: &str) -> bool {
    if is_not_web_extension_in_web_target_platform(all_target_platforms, product_target_platform) {
        return false;
    }
    match extension_target_platform {
        UNDEFINED | UNIVERSAL => true,
        UNKNOWN => false,
        platform => platform == product_target_platform,
    }
}

/// `sortExtensionVersions`: versions of the same number keep their order, the preferred platform's build first.
fn sort_extension_versions(versions: &mut [Value], preferred_target_platform: &str) {
    for index in 1..versions.len() {
        if versions[index]["version"] != versions[index - 1]["version"] || target_platform_of(&versions[index]) != preferred_target_platform {
            continue;
        }
        let mut insertion = index;
        while insertion > 0 && versions[insertion - 1]["version"] == versions[index]["version"] {
            insertion -= 1;
        }
        versions[insertion..=index].rotate_right(1);
    }
}

/// `filterLatestExtensionVersionsForTargetPlatform`.
fn filter_latest_extension_versions_for_target_platform(versions: &[Value], target_platform: &str, all_target_platforms: &[&str]) -> Vec<Value> {
    let mut latest: Vec<Value> = Vec::new();
    let mut pre_release_index: Option<usize> = None;
    let mut release_index: Option<usize> = None;
    for version in versions {
        let platform = target_platform_of(version);
        if !is_target_platform_compatible(platform, all_target_platforms, target_platform) {
            latest.push(version.clone());
            continue;
        }
        let slot = if is_pre_release_version(version) { &mut pre_release_index } else { &mut release_index };
        match *slot {
            None => {
                *slot = Some(latest.len());
                latest.push(version.clone());
            }
            Some(index) if platform == target_platform && latest[index]["version"] == version["version"] => latest[index] = version.clone(),
            Some(_) => {}
        }
    }
    latest
}

fn version_asset(version: &Value, asset_type: &str) -> Option<Asset> {
    version["files"].as_array()?.iter().find(|file| file["assetType"] == asset_type)?;
    let suffix = version["targetPlatform"].as_str().filter(|platform| !platform.is_empty()).map(|platform| format!("?targetPlatform={platform}")).unwrap_or_default();
    Some(Asset {
        uri: format!("{}/{asset_type}{suffix}", version["assetUri"].as_str().unwrap_or_default()),
        fallback_uri: format!("{}/{asset_type}{suffix}", version["fallbackAssetUri"].as_str().unwrap_or_default()),
    })
}

/// `getDownloadAsset`: always the fallback uri, so the gallery counts the download.
fn download_asset(version: &Value) -> Asset {
    let fallback = version["fallbackAssetUri"].as_str().unwrap_or_default();
    let platform = version["targetPlatform"].as_str().filter(|platform| !platform.is_empty());
    Asset {
        uri: format!("{fallback}/{VSIX}?redirect=true{}", platform.map(|platform| format!("&targetPlatform={platform}")).unwrap_or_default()),
        fallback_uri: format!("{fallback}/{VSIX}{}", platform.map(|platform| format!("?targetPlatform={platform}")).unwrap_or_default()),
    }
}

/// `getRepositoryAsset`: the `Links.Source` property when it looks like a git URL.
fn repository(version: &Value) -> Option<String> {
    let value = property(version, "Microsoft.VisualStudio.Services.Links.Source")?;
    let is_git = (value.starts_with("git") || value.starts_with("ssh") || value.starts_with("http")) && value.contains(".git");
    is_git.then(|| value.to_owned())
}

fn list_property(version: &Value, key: &str, lowercase: bool) -> Vec<String> {
    property(version, key)
        .filter(|value| !value.is_empty())
        .map(|value| value.split(',').map(|item| if lowercase { item.to_lowercase() } else { item.to_owned() }).collect())
        .unwrap_or_default()
}

fn statistic(raw: &Value, name: &str) -> f64 {
    raw["statistics"].as_array().and_then(|statistics| statistics.iter().find(|statistic| statistic["statisticName"] == name)).and_then(|statistic| statistic["value"].as_f64()).unwrap_or(0.0)
}

fn strings(value: &Value) -> Vec<String> {
    value.as_array().into_iter().flatten().filter_map(|item| item.as_str().map(str::to_owned)).collect()
}

/// `toExtension`.
fn to_extension(raw: &Value, version: &Value, all_target_platforms: &[&'static str]) -> GalleryExtension {
    let latest = &raw["versions"][0];
    let publisher = raw["publisher"]["publisherName"].as_str().unwrap_or_default().to_owned();
    let name = raw["extensionName"].as_str().unwrap_or_default().to_owned();
    let signature = version_asset(version, SIGNATURE);
    let public_key = version["files"].as_array().and_then(|files| files.iter().find(|file| file["assetType"] == PUBLIC_KEY)).and_then(|file| file["source"].as_str()).map(str::to_owned);
    GalleryExtension {
        id: format!("{publisher}.{name}").to_lowercase(),
        uuid: raw["extensionId"].as_str().unwrap_or_default().to_owned(),
        version: version["version"].as_str().unwrap_or_default().to_owned(),
        display_name: raw["displayName"].as_str().unwrap_or_default().to_owned(),
        publisher_id: raw["publisher"]["publisherId"].as_str().unwrap_or_default().to_owned(),
        publisher_display_name: raw["publisher"]["displayName"].as_str().unwrap_or_default().to_owned(),
        description: raw["shortDescription"].as_str().unwrap_or_default().to_owned(),
        install_count: statistic(raw, "install"),
        rating: statistic(raw, "averagerating"),
        rating_count: statistic(raw, "ratingcount"),
        categories: strings(&raw["categories"]),
        tags: strings(&raw["tags"]),
        release_date: raw["releaseDate"].as_str().unwrap_or_default().to_owned(),
        last_updated: raw["lastUpdated"].as_str().unwrap_or_default().to_owned(),
        all_target_platforms: all_target_platforms.to_vec(),
        target_platform: target_platform_of(version),
        is_pre_release_version: is_pre_release_version(version),
        has_pre_release_version: is_pre_release_version(latest),
        engine: property(version, "Microsoft.VisualStudio.Code.Engine").unwrap_or_default().to_owned(),
        dependencies: list_property(version, "Microsoft.VisualStudio.Code.ExtensionDependencies", true),
        extension_pack: list_property(version, "Microsoft.VisualStudio.Code.ExtensionPack", true),
        localized_languages: list_property(version, "Microsoft.VisualStudio.Code.LocalizedLanguages", false),
        download: download_asset(version),
        manifest: version_asset(version, MANIFEST),
        readme: version_asset(version, "Microsoft.VisualStudio.Services.Content.Details"),
        changelog: version_asset(version, "Microsoft.VisualStudio.Services.Content.Changelog"),
        license: version_asset(version, "Microsoft.VisualStudio.Services.Content.License"),
        icon: version_asset(version, "Microsoft.VisualStudio.Services.Icons.Default"),
        is_signed: signature.is_some(),
        signature,
        public_key,
        repository: repository(version),
        preview: raw["flags"].as_str().is_some_and(|flags| flags.contains("preview")),
        blocked: !raw["blocking"].is_null() || raw["flags"].as_str().is_some_and(|flags| flags.split(',').any(|flag| flag.trim() == "blocked")) || is_blocked(version),
        private: property(latest, "PrivateMarketplace") == Some("true"),
        detail_link: format!("{ITEM_URL}?itemName={publisher}.{name}"),
        publisher_link: format!("{PUBLISHER_URL}/{publisher}"),
        publisher,
        name,
    }
}

#[cfg(test)]
mod tests;
