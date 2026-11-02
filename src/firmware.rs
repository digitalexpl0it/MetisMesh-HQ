//! Offline MeshCore firmware locker + USB flasher.
//!
//! Mirrors community firmware from GitHub Releases using the official flasher
//! catalog (`flasher.meshcore.io/config.json`). Flashes ESP32 via `esptool` /
//! `esptool.py` and nRF52 via `adafruit-nrfutil` / UF2 path when tools exist.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::db::Db;
use crate::usb_serial;

const CATALOG_URL: &str = "https://flasher.meshcore.io/config.json";
const GITHUB_RELEASES: &str =
    "https://api.github.com/repos/meshcore-dev/MeshCore/releases";
const USER_AGENT: &str = "MetisMeshHQ/0.1 (offline firmware locker; local desktop)";
const FAMILIES: &[&str] = &["companion", "repeater", "room-server"];
const KEEP_PER_FAMILY: usize = 2;
const DEFAULT_SYNC_HOURS: u64 = 12;
/// ureq::Response::into_string caps at 10 MB; MeshCore release JSON is larger.
const HTTP_BODY_CAP: u64 = 64 * 1024 * 1024;

// ---------- Status (UI polls these) ----------

#[derive(Debug, Clone, Default)]
pub struct SyncStatus {
    pub running: bool,
    pub phase: String,
    pub detail: String,
    pub last_ok_at: i64,
    pub last_error: String,
    pub asset_count: u64,
    pub bytes_on_disk: u64,
}

#[derive(Debug, Clone, Default)]
pub struct FlashStatus {
    pub running: bool,
    pub phase: String,
    pub detail: String,
    pub log: String,
    pub last_error: String,
}

static SYNC: OnceLock<Mutex<SyncStatus>> = OnceLock::new();
static FLASH: OnceLock<Mutex<FlashStatus>> = OnceLock::new();

fn sync_lock() -> &'static Mutex<SyncStatus> {
    SYNC.get_or_init(|| Mutex::new(SyncStatus::default()))
}

fn flash_lock() -> &'static Mutex<FlashStatus> {
    FLASH.get_or_init(|| Mutex::new(FlashStatus::default()))
}

pub fn sync_status() -> SyncStatus {
    sync_lock()
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .clone()
}

pub fn flash_status() -> FlashStatus {
    flash_lock()
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .clone()
}

fn set_sync(update: impl FnOnce(&mut SyncStatus)) {
    let mut guard = sync_lock().lock().unwrap_or_else(|err| err.into_inner());
    update(&mut guard);
}

fn set_flash(update: impl FnOnce(&mut FlashStatus)) {
    let mut guard = flash_lock().lock().unwrap_or_else(|err| err.into_inner());
    update(&mut guard);
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ---------- Paths ----------

pub fn firmware_root() -> PathBuf {
    crate::data_dir().join("firmware")
}

fn catalog_path() -> PathBuf {
    firmware_root().join("catalog.json")
}

fn index_path() -> PathBuf {
    firmware_root().join("index.json")
}

fn selection_path() -> PathBuf {
    firmware_root().join("selection.json")
}

fn unavailable_path() -> PathBuf {
    firmware_root().join("unavailable.json")
}

fn releases_dir() -> PathBuf {
    firmware_root().join("releases")
}

fn release_dir(tag: &str) -> PathBuf {
    releases_dir().join(sanitize_tag(tag))
}

fn sanitize_tag(tag: &str) -> String {
    tag.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

// ---------- Index ----------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FirmwareIndex {
    pub updated_at: i64,
    pub assets: Vec<IndexAsset>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexAsset {
    pub family: String,
    pub tag: String,
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
    pub path: String,
}

impl FirmwareIndex {
    fn load() -> Self {
        let path = index_path();
        let Ok(raw) = fs::read_to_string(&path) else {
            return Self::default();
        };
        serde_json::from_str(&raw).unwrap_or_default()
    }

    fn save(&self) -> Result<(), String> {
        ensure_dirs()?;
        let raw = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        fs::write(index_path(), raw).map_err(|e| e.to_string())
    }

    fn refresh_totals(&self) -> (u64, u64) {
        let count = self.assets.len() as u64;
        let bytes = self.assets.iter().map(|a| a.bytes).sum();
        (count, bytes)
    }

    fn has_asset(&self, tag: &str, name: &str, bytes: u64) -> bool {
        self.assets.iter().any(|a| {
            a.tag == tag
                && a.name == name
                && a.bytes == bytes
                && Path::new(&a.path).is_file()
        })
    }
}

fn ensure_dirs() -> Result<(), String> {
    fs::create_dir_all(releases_dir()).map_err(|e| e.to_string())
}

// ---------- Catalog (flasher config.json) ----------

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FlasherCatalog {
    #[serde(default)]
    pub role: BTreeMap<String, CatalogRoleMeta>,
    #[serde(default)]
    pub maker: BTreeMap<String, CatalogMaker>,
    #[serde(default)]
    pub device: Vec<CatalogDevice>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CatalogMaker {
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CatalogRoleMeta {
    #[serde(default)]
    pub title: String,
    #[serde(default, rename = "subTitle")]
    pub sub_title: String,
    #[serde(default)]
    pub tooltip: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CatalogDevice {
    #[serde(default)]
    pub maker: String,
    pub name: String,
    #[serde(default)]
    pub r#type: String,
    #[serde(default)]
    pub firmware: Vec<CatalogFirmware>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CatalogFirmware {
    #[serde(default)]
    pub class: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub github: Option<CatalogGithub>,
    /// Static CDN-style version map (kiss modem etc.).
    #[serde(default)]
    pub version: Option<BTreeMap<String, CatalogVersionFiles>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CatalogGithub {
    #[serde(rename = "type")]
    pub family: String,
    #[serde(default)]
    pub files: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CatalogVersionFiles {
    #[serde(default)]
    pub files: Vec<CatalogNamedFile>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CatalogNamedFile {
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
    #[serde(default)]
    pub title: String,
}

#[derive(Debug, Clone)]
pub struct DeviceChoice {
    pub name: String,
    pub maker: String,
    pub platform: String,
    pub roles: Vec<RoleChoice>,
}

#[derive(Debug, Clone)]
pub struct RoleChoice {
    pub role: String,
    pub label: String,
    pub tooltip: String,
    pub family: String,
    /// flash-wipe / flash-update / flash / download regex or exact name patterns
    pub patterns: BTreeMap<String, String>,
    /// When role uses static version map instead of GitHub regex.
    pub static_versions: Vec<StaticVersion>,
}

#[derive(Debug, Clone)]
pub struct StaticVersion {
    pub version: String,
    pub files: Vec<(String, String)>, // kind, name
}

pub fn load_catalog() -> Result<FlasherCatalog, String> {
    let path = catalog_path();
    let raw = fs::read_to_string(&path).map_err(|e| {
        format!("No local firmware catalog yet ({e}). Open Flasher and Sync when online.")
    })?;
    serde_json::from_str(&raw).map_err(|e| format!("Firmware catalog parse error: {e}"))
}

pub fn community_devices(catalog: &FlasherCatalog) -> Vec<DeviceChoice> {
    let mut by_device: BTreeMap<String, DeviceChoice> = BTreeMap::new();
    for offer in community_offers(catalog) {
        let entry = by_device.entry(offer.device.clone()).or_insert_with(|| DeviceChoice {
            name: offer.device.clone(),
            maker: offer.maker_id.clone(),
            platform: offer.platform.clone(),
            roles: Vec::new(),
        });
        entry.roles.push(RoleChoice {
            role: offer.role.clone(),
            label: offer.role_label.clone(),
            tooltip: offer.tooltip.clone(),
            family: offer.family.clone(),
            patterns: offer.patterns.clone(),
            static_versions: offer.static_versions.clone(),
        });
    }
    let mut out: Vec<_> = by_device.into_values().collect();
    out.sort_by(|a, b| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()));
    out
}

/// One catalog row: board × community role (Manage overlay / selective sync).
#[derive(Debug, Clone)]
pub struct FirmwareOffer {
    pub key: String,
    pub device: String,
    pub maker_id: String,
    pub maker_label: String,
    pub platform: String,
    pub role: String,
    pub role_label: String,
    pub role_pill: String,
    pub tooltip: String,
    pub family: String,
    pub patterns: BTreeMap<String, String>,
    pub static_versions: Vec<StaticVersion>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum VersionPolicy {
    #[default]
    Latest,
    Previous,
}

impl VersionPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Latest => "latest",
            Self::Previous => "previous",
        }
    }

    pub fn parse(raw: &str) -> Self {
        if raw.eq_ignore_ascii_case("previous") {
            Self::Previous
        } else {
            Self::Latest
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectionEntry {
    pub key: String,
    pub device: String,
    #[serde(default)]
    pub maker: String,
    pub role: String,
    #[serde(default)]
    pub version: VersionPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FirmwareSelection {
    #[serde(default)]
    pub updated_at: i64,
    /// Operator asked for every community offer (Latest). Restored when reopening Manage.
    #[serde(default)]
    pub download_all: bool,
    #[serde(default)]
    pub entries: Vec<SelectionEntry>,
}

/// True when every catalog offer key is present in the selection entries.
pub fn selection_covers_all_offers(
    selection: &FirmwareSelection,
    offers: &[FirmwareOffer],
) -> bool {
    if offers.is_empty() {
        return false;
    }
    let selected: BTreeSet<&str> = selection.entries.iter().map(|e| e.key.as_str()).collect();
    offers.iter().all(|o| selected.contains(o.key.as_str()))
}

pub fn selection_key(device: &str, role: &str) -> String {
    format!("{device}|{role}")
}

pub fn estimate_full_set_label() -> &'static str {
    "about 1 GB"
}

pub fn role_pill(role: &str) -> String {
    match role {
        "companionBle" => "BLE".into(),
        "companionUsb" => "USB".into(),
        "repeater" => "Repeater".into(),
        "roomServer" => "Room".into(),
        "kissRadio" => "KISS".into(),
        other => other.to_string(),
    }
}

pub fn community_offers(catalog: &FlasherCatalog) -> Vec<FirmwareOffer> {
    let mut out = Vec::new();
    for device in &catalog.device {
        let maker_label = catalog
            .maker
            .get(&device.maker)
            .map(|m| m.name.clone())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| {
                let mut s = device.maker.clone();
                if let Some(c) = s.get_mut(0..1) {
                    c.make_ascii_uppercase();
                }
                s
            });
        for fw in &device.firmware {
            if fw.class != "community" {
                continue;
            }
            let meta = catalog.role.get(&fw.role);
            let title = meta.map(|m| m.title.as_str()).unwrap_or(fw.role.as_str());
            let sub = meta.map(|m| m.sub_title.as_str()).unwrap_or("");
            let label = if sub.is_empty() {
                title.to_string()
            } else {
                format!("{title} ({sub})")
            };
            let tooltip = meta.map(|m| m.tooltip.clone()).unwrap_or_default();
            let key = selection_key(&device.name, &fw.role);
            if let Some(gh) = &fw.github {
                out.push(FirmwareOffer {
                    key,
                    device: device.name.clone(),
                    maker_id: device.maker.clone(),
                    maker_label: maker_label.clone(),
                    platform: device.r#type.clone(),
                    role: fw.role.clone(),
                    role_label: label,
                    role_pill: role_pill(&fw.role),
                    tooltip,
                    family: gh.family.clone(),
                    patterns: gh.files.clone(),
                    static_versions: Vec::new(),
                });
            } else if let Some(versions) = &fw.version {
                let mut static_versions = Vec::new();
                for (ver, files) in versions {
                    static_versions.push(StaticVersion {
                        version: ver.clone(),
                        files: files
                            .files
                            .iter()
                            .map(|f| (f.kind.clone(), f.name.clone()))
                            .collect(),
                    });
                }
                static_versions.sort_by(|a, b| b.version.cmp(&a.version));
                out.push(FirmwareOffer {
                    key,
                    device: device.name.clone(),
                    maker_id: device.maker.clone(),
                    maker_label: maker_label.clone(),
                    platform: device.r#type.clone(),
                    role: fw.role.clone(),
                    role_label: label,
                    role_pill: role_pill(&fw.role),
                    tooltip,
                    family: String::new(),
                    patterns: BTreeMap::new(),
                    static_versions,
                });
            }
        }
    }
    out.sort_by(|a, b| {
        a.maker_label
            .to_ascii_lowercase()
            .cmp(&b.maker_label.to_ascii_lowercase())
            .then(a.device.to_ascii_lowercase().cmp(&b.device.to_ascii_lowercase()))
            .then(a.role.cmp(&b.role))
    });
    out
}

pub fn load_selection() -> FirmwareSelection {
    let Ok(raw) = fs::read_to_string(selection_path()) else {
        return FirmwareSelection::default();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

pub fn selection_file_exists() -> bool {
    selection_path().is_file()
}

pub fn save_selection(sel: &FirmwareSelection) -> Result<(), String> {
    ensure_dirs()?;
    let mut sel = sel.clone();
    sel.updated_at = now_unix();
    let raw = serde_json::to_string_pretty(&sel).map_err(|e| e.to_string())?;
    fs::write(selection_path(), raw).map_err(|e| e.to_string())
}

/// Selected offer keys whose patterns matched no GitHub/CDN asset on the last sync.
pub fn load_unavailable_keys() -> BTreeSet<String> {
    let Ok(raw) = fs::read_to_string(unavailable_path()) else {
        return BTreeSet::new();
    };
    serde_json::from_str::<Vec<String>>(&raw)
        .unwrap_or_default()
        .into_iter()
        .collect()
}

fn save_unavailable_keys(keys: &BTreeSet<String>) -> Result<(), String> {
    ensure_dirs()?;
    let list: Vec<String> = keys.iter().cloned().collect();
    let raw = serde_json::to_string_pretty(&list).map_err(|e| e.to_string())?;
    fs::write(unavailable_path(), raw).map_err(|e| e.to_string())
}

/// Pick latest or previous from a newest-first tag list.
pub fn pick_tag_for_policy(tags: &[String], policy: VersionPolicy) -> Option<String> {
    match policy {
        VersionPolicy::Latest => tags.first().cloned(),
        VersionPolicy::Previous => tags.get(1).cloned().or_else(|| tags.first().cloned()),
    }
}

/// Compile each offer pattern once, then scan the name list (UI/index hot path).
pub fn offer_matches_any_name(offer: &FirmwareOffer, names: &[&str]) -> bool {
    if names.is_empty() {
        return false;
    }
    if !offer.patterns.is_empty() {
        for pattern in offer.patterns.values() {
            let Ok(re) = Regex::new(pattern) else {
                continue;
            };
            if names.iter().any(|n| re.is_match(n)) {
                return true;
            }
        }
        return false;
    }
    offer.static_versions.iter().any(|ver| {
        ver.files
            .iter()
            .any(|(_, n)| names.iter().any(|have| *have == n.as_str()))
    })
}

/// Offer keys that already have at least one matching asset on disk.
pub fn local_offer_keys(offers: &[FirmwareOffer], index: &FirmwareIndex) -> BTreeSet<String> {
    let names: Vec<&str> = index.assets.iter().map(|a| a.name.as_str()).collect();
    let mut out = BTreeSet::new();
    for offer in offers {
        if offer_matches_any_name(offer, &names) {
            out.insert(offer.key.clone());
        }
    }
    out
}

/// When selection.json is missing, seed from index assets that match catalog offers.
pub fn auto_seed_selection_from_index(
    offers: &[FirmwareOffer],
    index: &FirmwareIndex,
) -> FirmwareSelection {
    let on_disk = local_offer_keys(offers, index);
    let mut entries = Vec::new();
    let mut seen = BTreeSet::new();
    for offer in offers {
        if !on_disk.contains(&offer.key) || !seen.insert(offer.key.clone()) {
            continue;
        }
        entries.push(SelectionEntry {
            key: offer.key.clone(),
            device: offer.device.clone(),
            maker: offer.maker_id.clone(),
            role: offer.role.clone(),
            version: VersionPolicy::Latest,
        });
    }
    FirmwareSelection {
        updated_at: now_unix(),
        download_all: false,
        entries,
    }
}

/// Load selection, auto-seeding once if the file is absent and the index has matches.
pub fn load_or_seed_selection(offers: &[FirmwareOffer], index: &FirmwareIndex) -> FirmwareSelection {
    if selection_file_exists() {
        return load_selection();
    }
    let seeded = auto_seed_selection_from_index(offers, index);
    if !seeded.entries.is_empty() {
        let _ = save_selection(&seeded);
    }
    seeded
}

/// Patterns / static file names an offer needs on disk for its version policy.
pub fn wanted_names_for_offer(
    offer: &FirmwareOffer,
    policy: VersionPolicy,
    release_names: &[String],
) -> Vec<String> {
    if !offer.static_versions.is_empty() {
        let idx = match policy {
            VersionPolicy::Latest => 0,
            VersionPolicy::Previous => 1.min(offer.static_versions.len().saturating_sub(1)),
        };
        return offer
            .static_versions
            .get(idx)
            .map(|v| v.files.iter().map(|(_, n)| n.clone()).collect())
            .unwrap_or_default();
    }
    let mut out = Vec::new();
    for pattern in offer.patterns.values() {
        if let Some(name) = match_asset_name(pattern, release_names) {
            if !out.iter().any(|n| n == &name) {
                out.push(name);
            }
        }
    }
    out
}

/// Pick flash-wipe vs flash-update (or nRF flash/download) pattern key.
pub fn flash_kind_key(erase: bool, patterns: &BTreeMap<String, String>) -> &'static str {
    if erase {
        if patterns.contains_key("flash-wipe") {
            return "flash-wipe";
        }
        if patterns.contains_key("flash") {
            return "flash";
        }
    } else {
        if patterns.contains_key("flash-update") {
            return "flash-update";
        }
        if patterns.contains_key("flash") {
            return "flash";
        }
        if patterns.contains_key("download") {
            return "download";
        }
    }
    if patterns.contains_key("flash-wipe") {
        "flash-wipe"
    } else if patterns.contains_key("flash-update") {
        "flash-update"
    } else if patterns.contains_key("flash") {
        "flash"
    } else {
        "download"
    }
}

pub fn match_asset_name(pattern: &str, names: &[String]) -> Option<String> {
    let re = Regex::new(pattern).ok()?;
    let mut hits: Vec<_> = names
        .iter()
        .filter(|n| re.is_match(n))
        .cloned()
        .collect();
    hits.sort();
    hits.into_iter().next()
}

/// Newest-first release tags seen in the local index, grouped by family.
pub fn family_tags_from_index(index: &FirmwareIndex) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for asset in &index.assets {
        if asset.family.is_empty() || asset.family == "static" || asset.tag == "catalog-static" {
            continue;
        }
        out.entry(asset.family.clone())
            .or_default()
            .insert(asset.tag.clone());
    }
    out.into_iter()
        .map(|(family, tags)| {
            let mut list: Vec<String> = tags.into_iter().collect();
            // companion-v1.18.0 sorts after companion-v1.17.x lexicographically for normal tags.
            list.sort_by(|a, b| b.cmp(a));
            (family, list)
        })
        .collect()
}

/// Short label for a release tag (`companion-v1.18.0` → `v1.18.0`).
pub fn short_version_label(tag: &str) -> String {
    for prefix in ["companion-", "repeater-", "room-server-"] {
        if let Some(rest) = tag.strip_prefix(prefix) {
            if rest.starts_with('v') || rest.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                return if rest.starts_with('v') {
                    rest.to_string()
                } else {
                    format!("v{rest}")
                };
            }
            return rest.to_string();
        }
    }
    tag.to_string()
}

/// Select option text for Latest/Previous using known family or static version names.
pub fn version_policy_option_label(
    policy: VersionPolicy,
    family_tags: &[String],
    static_versions: &[StaticVersion],
) -> String {
    let resolve = |policy: VersionPolicy| -> Option<String> {
        if !static_versions.is_empty() {
            let idx = match policy {
                VersionPolicy::Latest => 0,
                VersionPolicy::Previous => 1.min(static_versions.len().saturating_sub(1)),
            };
            static_versions.get(idx).map(|v| v.version.clone())
        } else {
            pick_tag_for_policy(family_tags, policy)
        }
    };
    let tag = resolve(policy);
    let distinct_previous = match (resolve(VersionPolicy::Latest), resolve(VersionPolicy::Previous))
    {
        (Some(a), Some(b)) => a != b,
        _ => false,
    };
    match (policy, tag) {
        (VersionPolicy::Latest, Some(t)) => format!("Latest · {}", short_version_label(&t)),
        (VersionPolicy::Previous, Some(t)) if distinct_previous => {
            format!("Previous · {}", short_version_label(&t))
        }
        (VersionPolicy::Previous, _) => "Previous".into(),
        (VersionPolicy::Latest, None) => "Latest".into(),
    }
}

/// Local tags/assets that satisfy a role pattern for a family.
pub fn local_versions_for_role(
    index: &FirmwareIndex,
    family: &str,
    pattern: &str,
) -> Vec<(String, String)> {
    // (tag, asset_name)
    let Ok(re) = Regex::new(pattern) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for asset in &index.assets {
        if !family.is_empty() && asset.family != family {
            continue;
        }
        if re.is_match(&asset.name) && Path::new(&asset.path).is_file() {
            out.push((asset.tag.clone(), asset.name.clone()));
        }
    }
    out.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    out.dedup();
    out
}

pub fn find_local_asset(index: &FirmwareIndex, tag: &str, name: &str) -> Option<IndexAsset> {
    index
        .assets
        .iter()
        .find(|a| a.tag == tag && a.name == name)
        .cloned()
}

// ---------- GitHub releases ----------

#[derive(Debug, Clone, Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    /// GitHub newest-first pages already order releases; kept for serde completeness.
    #[allow(dead_code)]
    published_at: Option<String>,
    assets: Vec<GhAsset>,
}

#[derive(Debug, Clone, Deserialize)]
struct GhAsset {
    name: String,
    size: u64,
    browser_download_url: String,
}

/// Latest then previous release tags per family (companion / repeater / room-server).
pub fn keep_tags_per_family(releases: &[(String, String)]) -> BTreeMap<String, Vec<String>> {
    // releases: (family, tag) already sorted newest-first within each family
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (family, tag) in releases {
        let list = out.entry(family.clone()).or_default();
        if list.len() >= KEEP_PER_FAMILY {
            continue;
        }
        if !list.iter().any(|t| t == tag) {
            list.push(tag.clone());
        }
    }
    out
}

fn family_of_tag(tag: &str) -> Option<&'static str> {
    for family in FAMILIES {
        if tag.starts_with(&format!("{family}-")) || tag.starts_with(&format!("{family}_")) {
            return Some(*family);
        }
        // tags look like companion-v1.18.0
        if tag.starts_with(family) {
            return Some(*family);
        }
    }
    None
}

fn http_get_json(url: &str) -> Result<String, String> {
    let resp = ureq::get(url)
        .set("User-Agent", USER_AGENT)
        .set("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(120))
        .call()
        .map_err(|e| e.to_string())?;
    let mut reader = resp.into_reader().take(HTTP_BODY_CAP + 1);
    let mut buf = Vec::new();
    reader
        .read_to_end(&mut buf)
        .map_err(|e| format!("Read body: {e}"))?;
    if buf.len() as u64 > HTTP_BODY_CAP {
        return Err(format!(
            "Response larger than {} MB",
            HTTP_BODY_CAP / (1024 * 1024)
        ));
    }
    String::from_utf8(buf).map_err(|e| format!("Body is not UTF-8: {e}"))
}

/// Walk GitHub release pages until we have latest + previous for each family.
fn fetch_meshcore_releases() -> Result<BTreeMap<String, Vec<GhRelease>>, String> {
    let mut by_family: BTreeMap<String, Vec<GhRelease>> = BTreeMap::new();
    // Small pages: each MeshCore release lists hundreds of assets and is huge JSON.
    const PER_PAGE: u32 = 5;
    const MAX_PAGES: u32 = 12;
    for page in 1..=MAX_PAGES {
        set_sync(|s| {
            s.detail = format!("Listing MeshCore GitHub releases (page {page})…");
        });
        let url = format!("{GITHUB_RELEASES}?per_page={PER_PAGE}&page={page}");
        let body = http_get_json(&url).map_err(|e| format!("GitHub: {e}"))?;
        let page_releases: Vec<GhRelease> =
            serde_json::from_str(&body).map_err(|e| format!("GitHub parse: {e}"))?;
        if page_releases.is_empty() {
            break;
        }
        let mut any_useful = false;
        for rel in page_releases {
            if rel.draft || rel.prerelease {
                continue;
            }
            let Some(family) = family_of_tag(&rel.tag_name) else {
                continue;
            };
            any_useful = true;
            let list = by_family.entry(family.to_string()).or_default();
            if list.len() >= KEEP_PER_FAMILY {
                continue;
            }
            // Pages are newest-first; first hits are the keep set.
            list.push(rel);
        }
        let complete = FAMILIES.iter().all(|family| {
            by_family
                .get(*family)
                .map(|list| list.len() >= KEEP_PER_FAMILY)
                .unwrap_or(false)
        });
        if complete {
            break;
        }
        if !any_useful && page > 2 {
            // Unrelated release noise; keep scanning a bit, then stop if empty families remain.
            continue;
        }
    }
    Ok(by_family)
}

fn download_file(url: &str, dest: &Path, expected_size: Option<u64>) -> Result<String, String> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = dest.with_extension("partial");
    let resp = ureq::get(url)
        .set("User-Agent", USER_AGENT)
        .timeout(Duration::from_secs(600))
        .call()
        .map_err(|e| format!("Download failed: {e}"))?;
    let mut reader = resp.into_reader();
    let mut file = File::create(&tmp).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    let mut written: u64 = 0;
    loop {
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        hasher.update(&buf[..n]);
        written += n as u64;
    }
    drop(file);
    if let Some(expected) = expected_size {
        if written != expected {
            let _ = fs::remove_file(&tmp);
            return Err(format!(
                "Size mismatch for {}: got {written}, expected {expected}",
                dest.display()
            ));
        }
    }
    fs::rename(&tmp, dest).map_err(|e| e.to_string())?;
    Ok(hex::encode(hasher.finalize()))
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

// ---------- Sync ----------

fn sync_enabled(db: &Db) -> bool {
    db.setting("firmware_sync_enabled")
        .ok()
        .flatten()
        .map(|v| v != "0" && !v.eq_ignore_ascii_case("false"))
        .unwrap_or(true)
}

/// Allowed background intervals: 12h, 1 day, 30 days.
pub fn normalize_sync_interval_hours(hours: u64) -> u64 {
    match hours {
        24 | 720 => hours,
        _ => DEFAULT_SYNC_HOURS,
    }
}

pub fn sync_interval_hours(db: &Db) -> u64 {
    let hours = db
        .setting("firmware_sync_interval_hours")
        .ok()
        .flatten()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_SYNC_HOURS);
    normalize_sync_interval_hours(hours)
}

/// True when `last_ok_at` is missing or older than the interval (pure helper for tests).
pub fn sync_due_at(last_ok_at: i64, now: i64, interval_secs: u64) -> bool {
    if last_ok_at <= 0 {
        return true;
    }
    now.saturating_sub(last_ok_at) >= interval_secs as i64
}

fn sync_interval_secs(db: &Db) -> u64 {
    sync_interval_hours(db).saturating_mul(3600)
}

/// True when background sync is enabled and the interval has elapsed (or never synced).
pub fn sync_is_due(db: &Db) -> bool {
    if !sync_enabled(db) {
        return false;
    }
    let last = {
        let status_at = sync_status().last_ok_at;
        if status_at > 0 {
            status_at
        } else {
            db.setting("firmware_last_sync_at")
                .ok()
                .flatten()
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(0)
        }
    };
    sync_due_at(last, now_unix(), sync_interval_secs(db))
}

/// Refresh disk totals into the status mutex from the current index.
pub fn refresh_status_from_disk() {
    let index = FirmwareIndex::load();
    let (count, bytes) = index.refresh_totals();
    set_sync(|s| {
        s.asset_count = count;
        s.bytes_on_disk = bytes;
        if s.phase.is_empty() && !s.running {
            s.phase = if count == 0 {
                "empty".into()
            } else {
                "ready".into()
            };
            if s.detail.is_empty() {
                s.detail = format!("{count} assets · {}", format_bytes(bytes));
            }
        }
    });
}

pub fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

fn run_sync(_db: &Db) -> Result<(), String> {
    ensure_dirs()?;
    set_sync(|s| {
        s.running = true;
        s.phase = "catalog".into();
        s.detail = "Fetching flasher catalog…".into();
        s.last_error.clear();
    });

    let catalog_body = http_get_json(CATALOG_URL).map_err(|e| format!("Catalog: {e}"))?;
    let catalog: FlasherCatalog =
        serde_json::from_str(&catalog_body).map_err(|e| format!("Catalog parse: {e}"))?;
    fs::write(catalog_path(), &catalog_body).map_err(|e| e.to_string())?;
    let offers = community_offers(&catalog);

    set_sync(|s| {
        s.phase = "releases".into();
        s.detail = "Listing MeshCore GitHub releases…".into();
    });

    let mut by_family = fetch_meshcore_releases()?;
    // Newest-first tag lists + full release records for asset URLs.
    let mut ordered: Vec<(String, String)> = Vec::new();
    let mut releases_by_tag: BTreeMap<String, GhRelease> = BTreeMap::new();
    for family in FAMILIES {
        if let Some(list) = by_family.remove(*family) {
            for rel in list.into_iter().take(KEEP_PER_FAMILY) {
                ordered.push((family.to_string(), rel.tag_name.clone()));
                releases_by_tag.insert(rel.tag_name.clone(), rel);
            }
        }
    }
    let family_tags = keep_tags_per_family(&ordered);
    if releases_by_tag.is_empty() {
        return Err(
            "No companion/repeater/room-server releases found on GitHub. Check network and try Sync now."
                .into(),
        );
    }

    let mut index = FirmwareIndex::load();
    let selection = load_or_seed_selection(&offers, &index);
    if selection.entries.is_empty() {
        index.assets.clear();
        index.updated_at = now_unix();
        index.save()?;
        let _ = save_unavailable_keys(&BTreeSet::new());
        prune_to_keep(&BTreeSet::new(), &BTreeSet::new())?;
        let (count, bytes) = index.refresh_totals();
        set_sync(|s| {
            s.running = false;
            s.phase = "ready".into();
            s.detail =
                "Nothing selected — open Manage firmware to choose boards and roles.".into();
            s.last_ok_at = index.updated_at;
            s.asset_count = count;
            s.bytes_on_disk = bytes;
            s.last_error.clear();
        });
        return Ok(());
    }

    let offer_by_key: BTreeMap<_, _> = offers.iter().map(|o| (o.key.clone(), o)).collect();
    // (tag, name) -> (family, optional github asset for download)
    let mut needed: BTreeMap<(String, String), (String, Option<GhAsset>)> = BTreeMap::new();
    let mut unavailable: BTreeSet<String> = BTreeSet::new();

    for entry in &selection.entries {
        let Some(offer) = offer_by_key.get(&entry.key) else {
            unavailable.insert(entry.key.clone());
            continue;
        };
        if !offer.static_versions.is_empty() {
            let names = wanted_names_for_offer(offer, entry.version, &[]);
            if names.is_empty() {
                unavailable.insert(entry.key.clone());
                continue;
            }
            for name in names {
                needed
                    .entry(("catalog-static".into(), name))
                    .or_insert_with(|| ("static".into(), None));
            }
            continue;
        }
        let tags = family_tags.get(&offer.family).cloned().unwrap_or_default();
        let Some(tag) = pick_tag_for_policy(&tags, entry.version) else {
            unavailable.insert(entry.key.clone());
            continue;
        };
        let Some(rel) = releases_by_tag.get(&tag) else {
            unavailable.insert(entry.key.clone());
            continue;
        };
        let release_names: Vec<String> = rel.assets.iter().map(|a| a.name.clone()).collect();
        let names = wanted_names_for_offer(offer, entry.version, &release_names);
        if names.is_empty() {
            // Catalog lists the role, but this MeshCore release has no matching asset.
            unavailable.insert(entry.key.clone());
            hq_log!(
                warn,
                "firmware: no published asset for {} ({}) on {tag}",
                offer.device,
                offer.role_pill
            );
            continue;
        }
        for name in names {
            let gh = rel.assets.iter().find(|a| a.name == name).cloned();
            needed
                .entry((tag.clone(), name))
                .or_insert_with(|| (offer.family.clone(), gh));
        }
    }
    let _ = save_unavailable_keys(&unavailable);

    let mut next_assets: Vec<IndexAsset> = Vec::new();
    let total = needed.len().max(1);
    let static_tag = "catalog-static";

    for (i, ((tag, name), (family, gh))) in needed.into_iter().enumerate() {
        set_sync(|s| {
            s.phase = "download".into();
            s.detail = format!("{tag}: {name} ({}/{})", i + 1, total);
        });
        let dest_dir = release_dir(&tag);
        fs::create_dir_all(&dest_dir).map_err(|e| e.to_string())?;
        let dest = dest_dir.join(&name);

        if tag == static_tag {
            let url = format!("https://flasher.meshcore.io/firmware/{name}");
            if dest.is_file() {
                let meta = fs::metadata(&dest).map_err(|e| e.to_string())?;
                let sha = sha256_file(&dest).unwrap_or_default();
                next_assets.push(IndexAsset {
                    family: "static".into(),
                    tag: static_tag.into(),
                    name,
                    bytes: meta.len(),
                    sha256: sha,
                    path: dest.to_string_lossy().into_owned(),
                });
                continue;
            }
            match download_file(&url, &dest, None) {
                Ok(sha) => {
                    let meta = fs::metadata(&dest).map_err(|e| e.to_string())?;
                    next_assets.push(IndexAsset {
                        family: "static".into(),
                        tag: static_tag.into(),
                        name,
                        bytes: meta.len(),
                        sha256: sha,
                        path: dest.to_string_lossy().into_owned(),
                    });
                }
                Err(err) => hq_log!(warn, "firmware CDN skip {name}: {err}"),
            }
            continue;
        }

        let Some(asset) = gh else {
            hq_log!(warn, "firmware: missing GitHub asset {name} on {tag}");
            continue;
        };
        if index.has_asset(&tag, &asset.name, asset.size) && dest.is_file() {
            let sha = index
                .assets
                .iter()
                .find(|a| a.tag == tag && a.name == asset.name)
                .map(|a| a.sha256.clone())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| sha256_file(&dest).unwrap_or_default());
            next_assets.push(IndexAsset {
                family: family.clone(),
                tag: tag.clone(),
                name: asset.name.clone(),
                bytes: asset.size,
                sha256: sha,
                path: dest.to_string_lossy().into_owned(),
            });
            continue;
        }
        let sha = download_file(&asset.browser_download_url, &dest, Some(asset.size))?;
        next_assets.push(IndexAsset {
            family,
            tag,
            name: asset.name,
            bytes: asset.size,
            sha256: sha,
            path: dest.to_string_lossy().into_owned(),
        });
    }

    set_sync(|s| {
        s.phase = "prune".into();
        s.detail = "Pruning firmware not in your selection…".into();
    });
    let keep_files: BTreeSet<(String, String)> = next_assets
        .iter()
        .map(|a| (a.tag.clone(), a.name.clone()))
        .collect();
    let keep_tags: BTreeSet<String> = next_assets.iter().map(|a| a.tag.clone()).collect();
    prune_to_keep(&keep_tags, &keep_files)?;

    index.assets = next_assets;
    index.updated_at = now_unix();
    index.save()?;
    let (count, bytes) = index.refresh_totals();
    let n_sel = selection.entries.len();
    let n_unavail = unavailable.len();
    let detail = if n_unavail == 0 {
        format!("{n_sel} selected · {count} assets · {}", format_bytes(bytes))
    } else {
        format!(
            "{n_sel} selected · {count} assets · {} · {n_unavail} role(s) not published upstream",
            format_bytes(bytes)
        )
    };

    set_sync(|s| {
        s.running = false;
        s.phase = "ready".into();
        s.detail = detail;
        s.last_ok_at = index.updated_at;
        s.asset_count = count;
        s.bytes_on_disk = bytes;
        s.last_error.clear();
    });
    Ok(())
}

fn prune_to_keep(
    keep_tags: &BTreeSet<String>,
    keep_files: &BTreeSet<(String, String)>,
) -> Result<(), String> {
    let Ok(entries) = fs::read_dir(releases_dir()) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let tag_dir_name = entry.file_name().to_string_lossy().into_owned();
        let tag_keep = keep_tags
            .iter()
            .any(|t| sanitize_tag(t) == tag_dir_name || t.as_str() == tag_dir_name);
        if !tag_keep {
            let _ = fs::remove_dir_all(entry.path());
            continue;
        }
        let Ok(files) = fs::read_dir(entry.path()) else {
            continue;
        };
        for file in files.flatten() {
            let name = file.file_name().to_string_lossy().into_owned();
            let tag_key = keep_tags
                .iter()
                .find(|t| sanitize_tag(t) == tag_dir_name || t.as_str() == tag_dir_name)
                .cloned()
                .unwrap_or_else(|| tag_dir_name.clone());
            if !keep_files.contains(&(tag_key, name)) {
                let _ = fs::remove_file(file.path());
            }
        }
    }
    Ok(())
}

fn finish_sync_err(err: String) {
    set_sync(|s| {
        s.running = false;
        s.phase = "error".into();
        s.detail = err.clone();
        s.last_error = err;
    });
}

/// Kick a sync if one is not already running.
pub fn start_sync(db: Arc<Db>) {
    {
        let guard = sync_lock().lock().unwrap_or_else(|e| e.into_inner());
        if guard.running {
            return;
        }
    }
    set_sync(|s| {
        s.running = true;
        s.phase = "starting".into();
        s.detail = "Starting firmware sync…".into();
        s.last_error.clear();
    });
    thread::Builder::new()
        .name("firmware-sync".into())
        .spawn(move || {
            let result = run_sync(&db);
            if let Ok(()) = &result {
                let _ = db.set_setting("firmware_last_sync_at", &now_unix().to_string());
            }
            match result {
                Ok(()) => {}
                Err(err) => {
                    hq_log!(warn, "firmware sync: {err}");
                    finish_sync_err(err);
                    refresh_status_from_disk();
                }
            }
        })
        .ok();
}

/// Background loop: sync when due (interval / never-synced), not on every launch.
pub fn spawn_sync_loop(db: Arc<Db>) {
    refresh_status_from_disk();
    if let Ok(Some(raw)) = db.setting("firmware_last_sync_at") {
        if let Ok(at) = raw.parse::<i64>() {
            set_sync(|s| {
                s.last_ok_at = at;
            });
        }
    }
    thread::Builder::new()
        .name("firmware-sync-loop".into())
        .spawn(move || {
            // Small delay so the window can open before a multi-GB pull.
            thread::sleep(Duration::from_secs(8));
            loop {
                if sync_is_due(&db) {
                    start_sync(db.clone());
                    // Wait until idle before polling again.
                    loop {
                        thread::sleep(Duration::from_secs(2));
                        if !sync_status().running {
                            break;
                        }
                    }
                }
                // Short sleep so Settings interval / enable changes apply without restart.
                thread::sleep(Duration::from_secs(60));
            }
        })
        .ok();
}

/// Absolute path string for the firmware locker (for UI / copy).
pub fn firmware_root_display() -> String {
    let _ = ensure_dirs();
    let path = firmware_root();
    std::fs::canonicalize(&path)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// Open the firmware folder in a **detached** file manager.
///
/// Must not block or share stdio with the HQ WebView — spawning a modal file
/// dialog from the UI thread made the desktop window shake/fight focus.
pub fn open_firmware_folder() -> Result<(), String> {
    ensure_dirs()?;
    let path = std::fs::canonicalize(firmware_root())
        .unwrap_or_else(|_| firmware_root());
    // Detach on a worker so the WebView event loop never waits on the FM.
    thread::Builder::new()
        .name("open-firmware-dir".into())
        .spawn(move || {
            if let Err(err) = open_folder_detached(&path) {
                hq_log!(warn, "open firmware folder: {err}");
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// `file://` URI with spaces / non-ASCII percent-encoded (D-Bus needs this).
fn path_as_file_uri(path: &Path) -> String {
    let abs = path.to_string_lossy();
    let mut out = String::from("file://");
    for b in abs.as_bytes() {
        let c = *b;
        let keep = c.is_ascii_alphanumeric()
            || matches!(c, b'/' | b'-' | b'_' | b'.' | b'~');
        if keep {
            out.push(c as char);
        } else {
            out.push('%');
            out.push(char::from_digit((c >> 4) as u32, 16).unwrap_or('0'));
            out.push(char::from_digit((c & 0xf) as u32, 16).unwrap_or('0'));
        }
    }
    out
}

fn spawn_detached(mut cmd: Command) -> Result<(), String> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn open_folder_detached(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        return spawn_detached({
            let mut c = Command::new("explorer");
            c.arg(path);
            c
        });
    }
    #[cfg(target_os = "macos")]
    {
        return spawn_detached({
            let mut c = Command::new("open");
            c.arg(path);
            c
        });
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // Prefer path-based openers first — they handle spaces. D-Bus ShowFolders
        // used to "succeed" with an unencoded `MetisMesh HQ` URI and open nothing.
        let path_s = path.display().to_string();
        for (bin, args) in [
            ("gio", vec!["open".into(), path_s.clone()]),
            ("xdg-open", vec![path_s.clone()]),
            ("nautilus", vec![path_s.clone()]),
            ("thunar", vec![path_s.clone()]),
            ("dolphin", vec![path_s.clone()]),
        ] {
            let mut cmd = Command::new(bin);
            cmd.args(&args);
            if spawn_detached(cmd).is_ok() {
                return Ok(());
            }
        }
        let uri = path_as_file_uri(path);
        let dbus_ok = Command::new("dbus-send")
            .args([
                "--session",
                "--dest=org.freedesktop.FileManager1",
                "--type=method_call",
                "/org/freedesktop/FileManager1",
                "org.freedesktop.FileManager1.ShowFolders",
                &format!("array:string:{uri}"),
                "string:",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if dbus_ok {
            return Ok(());
        }
        return Err(format!(
            "Could not open folder {} (tried gio, xdg-open, FileManager1)",
            path.display()
        ));
    }
    #[allow(unreachable_code)]
    Ok(())
}

// ---------- Flash ----------

#[derive(Debug, Clone)]
pub struct FlashRequest {
    pub platform: String,
    pub port: String,
    pub asset_path: PathBuf,
    pub erase: bool,
    /// When true, asset is UF2 — do not run a serial flasher.
    pub uf2: bool,
}

fn find_tool(names: &[&str]) -> Option<PathBuf> {
    for name in names {
        if let Ok(output) = Command::new("which").arg(name).output() {
            if output.status.success() {
                let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path.is_empty() {
                    return Some(PathBuf::from(path));
                }
            }
        }
        // Windows: where.exe
        if let Ok(output) = Command::new("where").arg(name).output() {
            if output.status.success() {
                let path = String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if !path.is_empty() {
                    return Some(PathBuf::from(path));
                }
            }
        }
    }
    None
}

fn run_command_logged(cmd: &mut Command) -> Result<(), String> {
    let output = cmd.output().map_err(|e| format!("Could not run flasher: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut log = String::new();
    if !stdout.trim().is_empty() {
        log.push_str(&stdout);
    }
    if !stderr.trim().is_empty() {
        if !log.is_empty() {
            log.push('\n');
        }
        log.push_str(&stderr);
    }
    set_flash(|s| {
        s.log = log.clone();
        if !log.is_empty() {
            s.detail = log.lines().last().unwrap_or("…").to_string();
        }
    });
    if output.status.success() {
        Ok(())
    } else {
        Err(if log.trim().is_empty() {
            format!("Flasher exited with {}", output.status)
        } else {
            log.chars().take(800).collect()
        })
    }
}

fn flash_esp32(port: &str, image: &Path, erase: bool) -> Result<(), String> {
    let tool = find_tool(&["esptool.py", "esptool"])
        .ok_or_else(|| {
            "Install esptool (`pip install esptool` or your OS package) to flash ESP32 boards."
                .to_string()
        })?;
    let offset = if erase { "0x0" } else { "0x10000" };
    set_flash(|s| {
        s.phase = "flashing".into();
        s.detail = format!("Writing {} at {offset}…", image.display());
    });
    let mut cmd = Command::new(&tool);
    cmd.arg("--port")
        .arg(port)
        .arg("write_flash")
        .arg(offset)
        .arg(image);
    run_command_logged(&mut cmd)
}

fn flash_nrf52_dfu(port: &str, zip: &Path) -> Result<(), String> {
    let tool = find_tool(&["adafruit-nrfutil", "nrfutil"]).ok_or_else(|| {
        format!(
            "No nRF52 DFU tool found. Install adafruit-nrfutil, or copy the UF2 manually. Package: {}",
            zip.display()
        )
    })?;
    set_flash(|s| {
        s.phase = "flashing".into();
        s.detail = format!("DFU {}…", zip.display());
    });
    let mut cmd = Command::new(&tool);
    if tool.file_name().and_then(|n| n.to_str()) == Some("adafruit-nrfutil") {
        cmd.args([
            "--verbose",
            "dfu",
            "serial",
            "--package",
        ])
        .arg(zip)
        .args(["-p", port, "-b", "115200", "--singlebank", "--touch", "1200"]);
    } else {
        // Generic nrfutil — best-effort; operators may need a newer invocation.
        cmd.args(["dfu", "usb-serial", "-pkg"])
            .arg(zip)
            .args(["-p", port]);
    }
    run_command_logged(&mut cmd)
}

fn run_flash(req: FlashRequest) -> Result<(), String> {
    if !req.asset_path.is_file() {
        return Err(format!("Firmware file missing: {}", req.asset_path.display()));
    }
    let meta = fs::metadata(&req.asset_path).map_err(|e| e.to_string())?;
    if meta.len() == 0 {
        return Err("Firmware file is empty.".into());
    }

    if req.uf2 {
        return Err(format!(
            "This board uses UF2. Put it in bootloader mode and copy:\n{}",
            req.asset_path.display()
        ));
    }

    let platform = req.platform.to_ascii_lowercase();
    if platform.contains("nrf") {
        flash_nrf52_dfu(&req.port, &req.asset_path)
    } else if platform.contains("esp") || platform.is_empty() {
        flash_esp32(&req.port, &req.asset_path, req.erase)
    } else {
        Err(format!(
            "Unsupported platform `{}` for in-app flash.",
            req.platform
        ))
    }
}

/// Disconnect any radio-pool USB session on this path, then flash.
pub fn start_flash(radio: crate::radio::RadioHandle, req: FlashRequest) {
    {
        let guard = flash_lock().lock().unwrap_or_else(|e| e.into_inner());
        if guard.running {
            return;
        }
    }
    set_flash(|s| {
        s.running = true;
        s.phase = "prepare".into();
        s.detail = "Preparing USB port…".into();
        s.log.clear();
        s.last_error.clear();
    });
    thread::Builder::new()
        .name("firmware-flash".into())
        .spawn(move || {
            // Drop any live companion session on this serial path.
            for slot in radio.pool_snapshot() {
                if slot.transport == "usb"
                    && (slot.address == req.port
                        || slot.address.eq_ignore_ascii_case(&req.port))
                {
                    radio.disconnect_radio(slot.id);
                    thread::sleep(Duration::from_millis(800));
                    break;
                }
            }
            // Brief settle for CDC re-enumerate.
            thread::sleep(Duration::from_millis(400));
            match run_flash(req) {
                Ok(()) => {
                    set_flash(|s| {
                        s.running = false;
                        s.phase = "done".into();
                        s.detail = "Flash complete. Replug or reboot the board if it does not reset.".into();
                        s.last_error.clear();
                    });
                }
                Err(err) => {
                    hq_log!(warn, "firmware flash: {err}");
                    set_flash(|s| {
                        s.running = false;
                        s.phase = "error".into();
                        s.detail = err.clone();
                        s.last_error = err;
                    });
                }
            }
        })
        .ok();
}

/// Resolve the on-disk asset for a flash selection.
pub fn resolve_flash_asset(
    index: &FirmwareIndex,
    role: &RoleChoice,
    tag: &str,
    erase: bool,
) -> Result<(PathBuf, bool), String> {
    if !role.static_versions.is_empty() {
        let ver = role
            .static_versions
            .iter()
            .find(|v| v.version == tag)
            .or_else(|| role.static_versions.first())
            .ok_or_else(|| "No static firmware version available.".to_string())?;
        let want_kind = if erase { "flash-wipe" } else { "flash-update" };
        let name = ver
            .files
            .iter()
            .find(|(k, _)| k == want_kind)
            .or_else(|| ver.files.first())
            .map(|(_, n)| n.clone())
            .ok_or_else(|| "No firmware file listed for that version.".to_string())?;
        let asset = find_local_asset(index, "catalog-static", &name)
            .or_else(|| {
                index
                    .assets
                    .iter()
                    .find(|a| a.name == name)
                    .cloned()
            })
            .ok_or_else(|| {
                format!("Missing {name}. Sync firmware while online, then try again.")
            })?;
        let uf2 = name.to_ascii_lowercase().ends_with(".uf2");
        return Ok((PathBuf::from(asset.path), uf2));
    }

    let key = flash_kind_key(erase, &role.patterns);
    let pattern = role
        .patterns
        .get(key)
        .ok_or_else(|| format!("No `{key}` pattern for this role."))?;
    let names: Vec<String> = index
        .assets
        .iter()
        .filter(|a| a.tag == tag || role.family.is_empty() || a.family == role.family)
        .filter(|a| tag.is_empty() || a.tag == tag)
        .map(|a| a.name.clone())
        .collect();
    let name = match_asset_name(pattern, &names).ok_or_else(|| {
        format!("No local file matches {pattern}. Sync firmware while online, then try again.")
    })?;
    let asset = find_local_asset(index, tag, &name).ok_or_else(|| {
        format!("Indexed asset {name} for {tag} is missing on disk.")
    })?;
    let uf2 = name.to_ascii_lowercase().ends_with(".uf2");
    Ok((PathBuf::from(asset.path), uf2))
}

pub fn load_index() -> FirmwareIndex {
    FirmwareIndex::load()
}

pub fn list_usb_ports_for_flash() -> Vec<usb_serial::UsbPort> {
    usb_serial::list_usb_ports().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keep_tags_takes_latest_two_per_family() {
        let ordered = vec![
            ("companion".into(), "companion-v1.18.0".into()),
            ("companion".into(), "companion-v1.17.1".into()),
            ("companion".into(), "companion-v1.17.0".into()),
            ("repeater".into(), "repeater-v1.18.0".into()),
        ];
        let keep = keep_tags_per_family(&ordered);
        assert_eq!(
            keep.get("companion").map(|v| v.as_slice()),
            Some(
                [
                    "companion-v1.18.0".to_string(),
                    "companion-v1.17.1".to_string()
                ]
                .as_slice()
            )
        );
        assert_eq!(
            keep.get("repeater").map(|v| v.as_slice()),
            Some(["repeater-v1.18.0".to_string()].as_slice())
        );
    }

    #[test]
    fn flash_kind_respects_erase_flag() {
        let mut patterns = BTreeMap::new();
        patterns.insert("flash-wipe".into(), ".*-merged\\.bin".into());
        patterns.insert("flash-update".into(), ".*-[a-f0-9]{7}\\.bin".into());
        assert_eq!(flash_kind_key(false, &patterns), "flash-update");
        assert_eq!(flash_kind_key(true, &patterns), "flash-wipe");
    }

    #[test]
    fn match_tbeam_supreme_ble_patterns() {
        let names = vec![
            "T_Beam_S3_Supreme_SX1262_companion_radio_ble-v1.18.0-81dbae5-merged.bin".into(),
            "T_Beam_S3_Supreme_SX1262_companion_radio_ble-v1.18.0-81dbae5.bin".into(),
            "T_Beam_S3_Supreme_SX1262_companion_radio_usb-v1.18.0-81dbae5.bin".into(),
        ];
        let wipe = match_asset_name(
            "T_Beam_S3_Supreme_SX1262_companion_radio_ble.*?-merged\\.bin",
            &names,
        );
        let update = match_asset_name(
            "T_Beam_S3_Supreme_SX1262_companion_radio_ble.*?-[a-f0-9]{7}\\.bin",
            &names,
        );
        assert!(wipe.unwrap().ends_with("-merged.bin"));
        assert_eq!(
            update.as_deref(),
            Some("T_Beam_S3_Supreme_SX1262_companion_radio_ble-v1.18.0-81dbae5.bin")
        );
    }

    #[test]
    fn family_of_tag_parses_release_prefixes() {
        assert_eq!(family_of_tag("companion-v1.18.0"), Some("companion"));
        assert_eq!(family_of_tag("repeater-v1.17.1"), Some("repeater"));
        assert_eq!(family_of_tag("room-server-v1.18.0"), Some("room-server"));
        assert_eq!(family_of_tag("other-v1"), None);
    }

    #[test]
    fn index_skips_present_asset() {
        let mut index = FirmwareIndex::default();
        index.assets.push(IndexAsset {
            family: "companion".into(),
            tag: "companion-v1.18.0".into(),
            name: "a.bin".into(),
            bytes: 10,
            sha256: "abc".into(),
            path: "/tmp/does-not-need-to-exist-for-false".into(),
        });
        // Path missing → has_asset false
        assert!(!index.has_asset("companion-v1.18.0", "a.bin", 10));
    }

    #[test]
    fn selection_key_round_trip() {
        let key = selection_key("LilyGo T-Beam Supreme (SX1262)", "companionBle");
        assert_eq!(key, "LilyGo T-Beam Supreme (SX1262)|companionBle");
        let (device, role) = key.split_once('|').unwrap();
        assert_eq!(selection_key(device, role), key);
    }

    #[test]
    fn version_policy_picks_latest_then_previous() {
        let tags = vec![
            "companion-v1.18.0".into(),
            "companion-v1.17.1".into(),
        ];
        assert_eq!(
            pick_tag_for_policy(&tags, VersionPolicy::Latest).as_deref(),
            Some("companion-v1.18.0")
        );
        assert_eq!(
            pick_tag_for_policy(&tags, VersionPolicy::Previous).as_deref(),
            Some("companion-v1.17.1")
        );
        assert_eq!(
            pick_tag_for_policy(&tags[..1], VersionPolicy::Previous).as_deref(),
            Some("companion-v1.18.0")
        );
    }

    fn tbeam_ble_offer() -> FirmwareOffer {
        let mut patterns = BTreeMap::new();
        patterns.insert(
            "flash-wipe".into(),
            "T_Beam_S3_Supreme_SX1262_companion_radio_ble.*?-merged\\.bin".into(),
        );
        patterns.insert(
            "flash-update".into(),
            "T_Beam_S3_Supreme_SX1262_companion_radio_ble.*?-[a-f0-9]{7}\\.bin".into(),
        );
        FirmwareOffer {
            key: selection_key("LilyGo T-Beam Supreme (SX1262)", "companionBle"),
            device: "LilyGo T-Beam Supreme (SX1262)".into(),
            maker_id: "lilygo".into(),
            maker_label: "LilyGo".into(),
            platform: "esp32".into(),
            role: "companionBle".into(),
            role_label: "Companion (BLE)".into(),
            role_pill: "BLE".into(),
            tooltip: String::new(),
            family: "companion".into(),
            patterns,
            static_versions: Vec::new(),
        }
    }

    #[test]
    fn selective_match_pulls_only_ble_assets() {
        let offer = tbeam_ble_offer();
        let names = vec![
            "T_Beam_S3_Supreme_SX1262_companion_radio_ble-v1.18.0-81dbae5-merged.bin".into(),
            "T_Beam_S3_Supreme_SX1262_companion_radio_ble-v1.18.0-81dbae5.bin".into(),
            "T_Beam_S3_Supreme_SX1262_companion_radio_usb-v1.18.0-81dbae5.bin".into(),
            "T_Beam_S3_Supreme_SX1262_companion_radio_usb-v1.18.0-81dbae5-merged.bin".into(),
        ];
        let wanted = wanted_names_for_offer(&offer, VersionPolicy::Latest, &names);
        assert_eq!(wanted.len(), 2);
        assert!(wanted.iter().all(|n| n.contains("_ble-")));
        assert!(wanted.iter().all(|n| !n.contains("_usb-")));
        assert!(offer_matches_any_name(&offer, &[names[0].as_str()]));
        assert!(!offer_matches_any_name(&offer, &[names[2].as_str()]));
    }

    #[test]
    fn auto_seed_from_matching_index_asset() {
        let offer = tbeam_ble_offer();
        let mut index = FirmwareIndex::default();
        index.assets.push(IndexAsset {
            family: "companion".into(),
            tag: "companion-v1.18.0".into(),
            name: "T_Beam_S3_Supreme_SX1262_companion_radio_ble-v1.18.0-81dbae5.bin".into(),
            bytes: 100,
            sha256: "abc".into(),
            path: "/tmp/x.bin".into(),
        });
        let seeded = auto_seed_selection_from_index(&[offer.clone()], &index);
        assert_eq!(seeded.entries.len(), 1);
        assert_eq!(seeded.entries[0].key, offer.key);
        assert_eq!(seeded.entries[0].version, VersionPolicy::Latest);

        let empty = auto_seed_selection_from_index(
            &[offer],
            &FirmwareIndex::default(),
        );
        assert!(empty.entries.is_empty());
    }

    #[test]
    fn estimate_full_set_is_about_one_gb() {
        assert_eq!(estimate_full_set_label(), "about 1 GB");
    }

    #[test]
    fn version_option_labels_include_short_tag() {
        assert_eq!(short_version_label("companion-v1.18.0"), "v1.18.0");
        let tags = vec![
            "companion-v1.18.0".into(),
            "companion-v1.17.1".into(),
        ];
        assert_eq!(
            version_policy_option_label(VersionPolicy::Latest, &tags, &[]),
            "Latest · v1.18.0"
        );
        assert_eq!(
            version_policy_option_label(VersionPolicy::Previous, &tags, &[]),
            "Previous · v1.17.1"
        );
        assert_eq!(
            version_policy_option_label(VersionPolicy::Previous, &tags[..1], &[]),
            "Previous"
        );
    }

    #[test]
    fn sync_interval_hours_normalize() {
        assert_eq!(normalize_sync_interval_hours(12), 12);
        assert_eq!(normalize_sync_interval_hours(24), 24);
        assert_eq!(normalize_sync_interval_hours(720), 720);
        assert_eq!(normalize_sync_interval_hours(1), 12);
        assert_eq!(normalize_sync_interval_hours(48), 12);
    }

    #[test]
    fn sync_due_respects_interval() {
        assert!(sync_due_at(0, 1_000, 12 * 3600));
        assert!(!sync_due_at(1_000, 1_000 + 60, 12 * 3600));
        assert!(sync_due_at(1_000, 1_000 + 12 * 3600, 12 * 3600));
        assert!(!sync_due_at(1_000, 1_000 + 23 * 3600, 24 * 3600));
        assert!(sync_due_at(1_000, 1_000 + 24 * 3600, 24 * 3600));
    }
}
