//! Offline MeshCore firmware locker + USB flasher.
//!
//! Mirrors community firmware from GitHub Releases using the official flasher
//! catalog (`flasher.meshcore.io/config.json`). Flashes ESP32 via `esptool` /
//! `esptool.py` and nRF52 via `adafruit-nrfutil` / UF2 path when tools exist.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
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
    "https://api.github.com/repos/meshcore-dev/MeshCore/releases?per_page=40";
const USER_AGENT: &str = "MetisMeshHQ/0.1 (offline firmware locker; local desktop)";
const FAMILIES: &[&str] = &["companion", "repeater", "room-server"];
const KEEP_PER_FAMILY: usize = 2;
const DEFAULT_SYNC_HOURS: u64 = 12;

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
    pub device: Vec<CatalogDevice>,
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
    let mut out = Vec::new();
    for device in &catalog.device {
        let mut roles = Vec::new();
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
            if let Some(gh) = &fw.github {
                roles.push(RoleChoice {
                    role: fw.role.clone(),
                    label,
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
                roles.push(RoleChoice {
                    role: fw.role.clone(),
                    label,
                    tooltip,
                    family: String::new(),
                    patterns: BTreeMap::new(),
                    static_versions,
                });
            }
        }
        if roles.is_empty() {
            continue;
        }
        out.push(DeviceChoice {
            name: device.name.clone(),
            maker: device.maker.clone(),
            platform: device.r#type.clone(),
            roles,
        });
    }
    out.sort_by(|a, b| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()));
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

#[derive(Debug, Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    published_at: Option<String>,
    assets: Vec<GhAsset>,
}

#[derive(Debug, Deserialize)]
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
    ureq::get(url)
        .set("User-Agent", USER_AGENT)
        .set("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(60))
        .call()
        .map_err(|e| e.to_string())?
        .into_string()
        .map_err(|e| e.to_string())
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

fn sync_interval_secs(db: &Db) -> u64 {
    let hours = db
        .setting("firmware_sync_interval_hours")
        .ok()
        .flatten()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_SYNC_HOURS)
        .max(1);
    hours.saturating_mul(3600)
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

    set_sync(|s| {
        s.phase = "releases".into();
        s.detail = "Listing MeshCore GitHub releases…".into();
    });

    let releases_body = http_get_json(GITHUB_RELEASES).map_err(|e| format!("GitHub: {e}"))?;
    let releases: Vec<GhRelease> =
        serde_json::from_str(&releases_body).map_err(|e| format!("GitHub parse: {e}"))?;

    let mut by_family: BTreeMap<String, Vec<GhRelease>> = BTreeMap::new();
    for rel in releases {
        if rel.draft || rel.prerelease {
            continue;
        }
        let Some(family) = family_of_tag(&rel.tag_name) else {
            continue;
        };
        by_family.entry(family.to_string()).or_default().push(rel);
    }
    for list in by_family.values_mut() {
        list.sort_by(|a, b| {
            b.published_at
                .as_deref()
                .unwrap_or("")
                .cmp(a.published_at.as_deref().unwrap_or(""))
        });
    }

    let mut ordered: Vec<(String, String)> = Vec::new();
    let mut selected: Vec<(String, GhRelease)> = Vec::new();
    for family in FAMILIES {
        if let Some(list) = by_family.remove(*family) {
            for rel in list.into_iter().take(KEEP_PER_FAMILY) {
                ordered.push((family.to_string(), rel.tag_name.clone()));
                selected.push((family.to_string(), rel));
            }
        }
    }
    let keep = keep_tags_per_family(&ordered);
    let keep_tags: BTreeSet<String> = keep.values().flatten().cloned().collect();

    let mut index = FirmwareIndex::load();
    let mut next_assets: Vec<IndexAsset> = Vec::new();

    for (family, rel) in &selected {
        let tag = &rel.tag_name;
        let dest_dir = release_dir(tag);
        fs::create_dir_all(&dest_dir).map_err(|e| e.to_string())?;
        let total = rel.assets.len();
        for (i, asset) in rel.assets.iter().enumerate() {
            set_sync(|s| {
                s.phase = "download".into();
                s.detail = format!(
                    "{tag}: {} ({}/{})",
                    asset.name,
                    i + 1,
                    total.max(1)
                );
            });
            let dest = dest_dir.join(&asset.name);
            if index.has_asset(tag, &asset.name, asset.size) {
                let path = dest.to_string_lossy().into_owned();
                let sha = index
                    .assets
                    .iter()
                    .find(|a| a.tag == *tag && a.name == asset.name)
                    .map(|a| a.sha256.clone())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| sha256_file(&dest).unwrap_or_default());
                next_assets.push(IndexAsset {
                    family: family.clone(),
                    tag: tag.clone(),
                    name: asset.name.clone(),
                    bytes: asset.size,
                    sha256: sha,
                    path,
                });
                continue;
            }
            let sha = download_file(&asset.browser_download_url, &dest, Some(asset.size))?;
            next_assets.push(IndexAsset {
                family: family.clone(),
                tag: tag.clone(),
                name: asset.name.clone(),
                bytes: asset.size,
                sha256: sha,
                path: dest.to_string_lossy().into_owned(),
            });
        }
    }

    // Static CDN files referenced by catalog (e.g. kiss modem).
    set_sync(|s| {
        s.phase = "cdn".into();
        s.detail = "Fetching static catalog firmware files…".into();
    });
    let mut static_names: BTreeSet<String> = BTreeSet::new();
    for device in &catalog.device {
        for fw in &device.firmware {
            if fw.class != "community" {
                continue;
            }
            let Some(versions) = &fw.version else {
                continue;
            };
            for ver in versions.values() {
                for file in &ver.files {
                    static_names.insert(file.name.clone());
                }
            }
        }
    }
    let static_tag = "catalog-static";
    let static_dir = release_dir(static_tag);
    if !static_names.is_empty() {
        fs::create_dir_all(&static_dir).map_err(|e| e.to_string())?;
    }
    for name in static_names {
        let dest = static_dir.join(&name);
        let url = format!("https://flasher.meshcore.io/firmware/{name}");
        set_sync(|s| {
            s.detail = format!("CDN: {name}");
        });
        if dest.is_file() {
            let meta = fs::metadata(&dest).map_err(|e| e.to_string())?;
            let sha = sha256_file(&dest).unwrap_or_default();
            next_assets.push(IndexAsset {
                family: "static".into(),
                tag: static_tag.into(),
                name: name.clone(),
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
            Err(err) => {
                hq_log!(warn, "firmware CDN skip {name}: {err}");
            }
        }
    }

    // Prune release dirs not in keep set (preserve catalog-static).
    set_sync(|s| {
        s.phase = "prune".into();
        s.detail = "Pruning old firmware tags…".into();
    });
    if let Ok(entries) = fs::read_dir(releases_dir()) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == static_tag || keep_tags.contains(&name) {
                continue;
            }
            // Tags may have been sanitized; also match unsanitized keep set.
            let keep = keep_tags.iter().any(|t| sanitize_tag(t) == name);
            if keep {
                continue;
            }
            let _ = fs::remove_dir_all(entry.path());
        }
    }

    index.assets = next_assets;
    index.updated_at = now_unix();
    index.save()?;
    let (count, bytes) = index.refresh_totals();

    set_sync(|s| {
        s.running = false;
        s.phase = "ready".into();
        s.detail = format!("Synced {count} assets · {}", format_bytes(bytes));
        s.last_ok_at = index.updated_at;
        s.asset_count = count;
        s.bytes_on_disk = bytes;
        s.last_error.clear();
    });
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

/// Background loop: startup sync + interval while enabled.
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
                if sync_enabled(&db) {
                    start_sync(db.clone());
                    // Wait until idle before sleeping the interval.
                    loop {
                        thread::sleep(Duration::from_secs(2));
                        if !sync_status().running {
                            break;
                        }
                    }
                }
                let wait = sync_interval_secs(&db);
                thread::sleep(Duration::from_secs(wait));
            }
        })
        .ok();
}

pub fn open_firmware_folder() -> Result<(), String> {
    ensure_dirs()?;
    let path = firmware_root();
    #[cfg(target_os = "windows")]
    {
        Command::new("explorer")
            .arg(&path)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg(&path)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Command::new("xdg-open")
            .arg(&path)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
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
}
