//! Offline Survival Guides: downloadable HQ-native packs, FTS search, and export.
//!
//! Packs live under `data_dir()/guides/` and never merge into `metis.db`.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use base64::Engine;
use pulldown_cmark::{html, Options, Parser};
use regex::Regex;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Outbound UA for guide downloads. Do **not** put "Metis" here — Ready.gov / Akamai
/// and other CDNs 403 User-Agents containing that substring (same lesson as NWS/USGS).
const USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36 LocalHQ/0.1";
const MAX_DOWNLOAD_BYTES: u64 = 80 * 1024 * 1024;
const DOWNLOAD_ATTEMPTS: u32 = 3;
const SEED_CATALOG: &str = include_str!("../assets/guides/catalog.seed.json");
/// Former seed packs dropped from Marketplace (uninstalled rows are pruned on load/sync).
const RETIRED_SEED_PACK_IDS: &[&str] = &["pd-shelters-shacks"];

const BUNDLED_HQ_BASICS_MANIFEST: &str =
    include_str!("../assets/guides/sample/hq-basics/manifest.json");
const BUNDLED_WATER: &str = include_str!("../assets/guides/sample/hq-basics/articles/water.md");
const BUNDLED_WATER_FILTER: &str =
    include_str!("../assets/guides/sample/hq-basics/articles/water-filter.md");
const BUNDLED_SHELTER: &str =
    include_str!("../assets/guides/sample/hq-basics/articles/shelter.md");
const BUNDLED_FIRST_AID: &str =
    include_str!("../assets/guides/sample/hq-basics/articles/first-aid.md");
const BUNDLED_RADIO: &str = include_str!("../assets/guides/sample/hq-basics/articles/radio.md");
const BUNDLED_CHEMICAL_SIP: &str =
    include_str!("../assets/guides/sample/hq-basics/articles/chemical-sip.md");

fn bundled_files(key: &str) -> Option<Vec<(&'static str, &'static str)>> {
    match key {
        "hq-basics" => Some(vec![
            ("manifest.json", BUNDLED_HQ_BASICS_MANIFEST),
            ("articles/water.md", BUNDLED_WATER),
            ("articles/water-filter.md", BUNDLED_WATER_FILTER),
            ("articles/shelter.md", BUNDLED_SHELTER),
            ("articles/first-aid.md", BUNDLED_FIRST_AID),
            ("articles/radio.md", BUNDLED_RADIO),
            ("articles/chemical-sip.md", BUNDLED_CHEMICAL_SIP),
        ]),
        _ => None,
    }
}

// ---------- Paths ----------

pub fn guides_root() -> PathBuf {
    let dir = crate::data_dir().join("guides");
    let _ = fs::create_dir_all(&dir);
    dir
}

fn catalog_path() -> PathBuf {
    guides_root().join("catalog.json")
}

fn packs_dir() -> PathBuf {
    let dir = guides_root().join("packs");
    let _ = fs::create_dir_all(&dir);
    dir
}

fn downloads_dir() -> PathBuf {
    let dir = guides_root().join("downloads");
    let _ = fs::create_dir_all(&dir);
    dir
}

fn index_db_path() -> PathBuf {
    guides_root().join("index.db")
}

pub fn pack_dir(pack_id: &str) -> PathBuf {
    packs_dir().join(sanitize_id(pack_id))
}

fn sanitize_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

// ---------- Catalog / manifest types ----------

/// Fixed Marketplace / Guides filter vocabulary (ids are stable; labels are UI-facing).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuideCategory {
    pub id: &'static str,
    pub label: &'static str,
}

pub const GUIDE_CATEGORIES: &[GuideCategory] = &[
    GuideCategory {
        id: "water",
        label: "Water",
    },
    GuideCategory {
        id: "medical",
        label: "Medical",
    },
    GuideCategory {
        id: "radio",
        label: "Radio",
    },
    GuideCategory {
        id: "shelter",
        label: "Shelter",
    },
    GuideCategory {
        id: "hazards",
        label: "Hazards",
    },
    GuideCategory {
        id: "knots",
        label: "Knots",
    },
    GuideCategory {
        id: "preparedness",
        label: "Preparedness",
    },
    GuideCategory {
        id: "fieldcraft",
        label: "Field craft",
    },
];

pub fn guide_category_label(id: &str) -> Option<&'static str> {
    GUIDE_CATEGORIES
        .iter()
        .find(|c| c.id == id)
        .map(|c| c.label)
}

pub fn is_known_guide_category(id: &str) -> bool {
    GUIDE_CATEGORIES.iter().any(|c| c.id == id)
}

/// Keep only known category ids, de-duplicated, stable order from [`GUIDE_CATEGORIES`].
pub fn normalize_guide_categories(raw: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for cat in GUIDE_CATEGORIES {
        if raw.iter().any(|r| r.trim() == cat.id) {
            out.push(cat.id.to_string());
        }
    }
    out
}

fn categories_index_value(cats: &[String]) -> String {
    normalize_guide_categories(cats).join(" ")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuidesCatalog {
    pub version: u32,
    #[serde(default)]
    pub updated_at: i64,
    #[serde(default)]
    pub remote_url: String,
    #[serde(default)]
    pub packs: Vec<CatalogPack>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogPack {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub license: String,
    #[serde(default)]
    pub description: String,
    /// `hq-native` (v1) or `zim` (stub / future).
    #[serde(default = "default_format")]
    pub format: String,
    #[serde(default)]
    pub download_url: Option<String>,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub size_bytes: Option<u64>,
    /// Remote files (usually PDFs) fetched on install; HQ synthesizes a local pack.
    #[serde(default)]
    pub files: Vec<CatalogRemoteFile>,
    /// Key into embedded sample packs (`hq-basics`).
    #[serde(default)]
    pub bundled: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Fixed vocabulary ids from [`GUIDE_CATEGORIES`] (packs may list several).
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub installed_version: Option<String>,
    #[serde(default)]
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogRemoteFile {
    pub id: String,
    pub title: String,
    pub url: String,
    pub sha256: String,
    #[serde(default)]
    pub size_bytes: Option<u64>,
    /// Relative path under the pack root (default `attachments/<id>.pdf`).
    #[serde(default)]
    pub path: String,
}

impl CatalogPack {
    pub fn is_installable(&self) -> bool {
        if self.format == "zim" {
            return false;
        }
        if self.bundled.as_ref().is_some_and(|s| !s.trim().is_empty()) {
            return true;
        }
        if self
            .download_url
            .as_ref()
            .is_some_and(|u| !u.trim().is_empty())
        {
            return true;
        }
        self.files.iter().any(|f| !f.url.trim().is_empty() && !f.sha256.trim().is_empty())
    }

    pub fn matches_category(&self, category_id: Option<&str>) -> bool {
        let Some(want) = category_id.map(str::trim).filter(|s| !s.is_empty()) else {
            return true;
        };
        normalize_guide_categories(&self.categories)
            .iter()
            .any(|c| c == want)
    }

    pub fn category_labels(&self) -> Vec<&'static str> {
        normalize_guide_categories(&self.categories)
            .into_iter()
            .filter_map(|id| guide_category_label(&id))
            .collect()
    }
}

fn default_format() -> String {
    "hq-native".into()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub license: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub articles: Vec<ArticleMeta>,
    #[serde(default)]
    pub attachments: Vec<AttachmentMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArticleMeta {
    pub id: String,
    pub title: String,
    pub path: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentMeta {
    pub id: String,
    pub title: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub pack_id: String,
    pub pack_name: String,
    pub article_id: String,
    pub title: String,
    pub snippet: String,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedArticle {
    pub pack_id: String,
    pub pack_name: String,
    pub article: ArticleMeta,
    pub body_markdown: String,
    pub body_html: String,
    /// All articles in the pack (for pills / prev-next).
    pub siblings: Vec<ArticleMeta>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JobStatus {
    pub running: bool,
    pub phase: String,
    pub detail: String,
    pub pack_id: String,
    pub error: String,
    /// 0–100 when `bytes_total > 0`; otherwise treat as indeterminate.
    pub percent: u8,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

fn job_lock() -> &'static Mutex<JobStatus> {
    static LOCK: Mutex<JobStatus> = Mutex::new(JobStatus {
        running: false,
        phase: String::new(),
        detail: String::new(),
        pack_id: String::new(),
        error: String::new(),
        percent: 0,
        bytes_done: 0,
        bytes_total: 0,
    });
    &LOCK
}

pub fn job_status() -> JobStatus {
    job_lock().lock().map(|g| g.clone()).unwrap_or_default()
}

fn set_job(status: JobStatus) {
    if let Ok(mut g) = job_lock().lock() {
        *g = status;
    }
}

fn job_busy(phase: &str, detail: impl Into<String>, pack_id: impl Into<String>) -> JobStatus {
    JobStatus {
        running: true,
        phase: phase.into(),
        detail: detail.into(),
        pack_id: pack_id.into(),
        error: String::new(),
        percent: 0,
        bytes_done: 0,
        bytes_total: 0,
    }
}

fn job_idle(phase: &str, detail: impl Into<String>, pack_id: impl Into<String>) -> JobStatus {
    JobStatus {
        running: false,
        phase: phase.into(),
        detail: detail.into(),
        pack_id: pack_id.into(),
        error: String::new(),
        percent: 0,
        bytes_done: 0,
        bytes_total: 0,
    }
}

fn job_fail(pack_id: impl Into<String>, error: impl Into<String>) -> JobStatus {
    JobStatus {
        running: false,
        phase: "error".into(),
        detail: String::new(),
        pack_id: pack_id.into(),
        error: error.into(),
        percent: 0,
        bytes_done: 0,
        bytes_total: 0,
    }
}

fn patch_job_progress(detail: &str, bytes_done: u64, bytes_total: u64) {
    if let Ok(mut g) = job_lock().lock() {
        if !detail.is_empty() {
            g.detail = detail.to_string();
        }
        g.bytes_done = bytes_done;
        g.bytes_total = bytes_total;
        g.percent = if bytes_total > 0 {
            ((bytes_done.min(bytes_total) * 100) / bytes_total) as u8
        } else {
            0
        };
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ---------- Catalog load / save / sync ----------

fn parse_seed() -> GuidesCatalog {
    serde_json::from_str(SEED_CATALOG).unwrap_or_else(|_| GuidesCatalog {
        version: 1,
        updated_at: 0,
        remote_url: String::new(),
        packs: vec![],
    })
}

pub fn load_catalog() -> GuidesCatalog {
    let path = catalog_path();
    let mut catalog = if path.is_file() {
        match fs::read_to_string(&path) {
            Ok(raw) => serde_json::from_str(&raw).unwrap_or_else(|_| parse_seed()),
            Err(_) => parse_seed(),
        }
    } else {
        let seed = parse_seed();
        let _ = save_catalog(&seed);
        seed
    };
    // App updates ship new seed rows; keep on-disk install state but add missing packs.
    if merge_bundled_seed(&mut catalog) {
        let _ = save_catalog(&catalog);
    }
    refresh_installed_flags(&mut catalog);
    catalog
}

/// Ensure every bundled seed pack id exists in `catalog`. Does not overwrite installed
/// rows. Refreshes metadata for uninstalled packs that already exist so URL/hash updates
/// in the seed land without a wipe. Drops uninstalled [`RETIRED_SEED_PACK_IDS`].
fn merge_bundled_seed(catalog: &mut GuidesCatalog) -> bool {
    let seed = parse_seed();
    let mut changed = false;
    if catalog.remote_url.trim().is_empty() && !seed.remote_url.trim().is_empty() {
        catalog.remote_url = seed.remote_url.clone();
        changed = true;
    }
    let before = catalog.packs.len();
    catalog.packs.retain(|p| {
        if !RETIRED_SEED_PACK_IDS.contains(&p.id.as_str()) {
            return true;
        }
        let installed = p.status == "installed" || p.installed_version.is_some();
        // Still retire even if marked installed — content was removed from the allowlist.
        if installed {
            let _ = remove_pack_files_only(&p.id);
        }
        false
    });
    if catalog.packs.len() != before {
        changed = true;
    }
    for sp in &seed.packs {
        match catalog.packs.iter_mut().find(|p| p.id == sp.id) {
            None => {
                catalog.packs.push(sp.clone());
                changed = true;
            }
            Some(existing) => {
                let installed = existing.status == "installed"
                    || existing.installed_version.is_some();
                if installed {
                    continue;
                }
                let status = existing.status.clone();
                let installed_version = existing.installed_version.clone();
                if existing.name != sp.name
                    || existing.version != sp.version
                    || existing.description != sp.description
                    || existing.files != sp.files
                    || existing.download_url != sp.download_url
                    || existing.sha256 != sp.sha256
                    || existing.size_bytes != sp.size_bytes
                    || existing.bundled != sp.bundled
                    || existing.tags != sp.tags
                    || existing.categories != sp.categories
                    || existing.source != sp.source
                    || existing.license != sp.license
                    || existing.format != sp.format
                {
                    *existing = sp.clone();
                    existing.status = status;
                    existing.installed_version = installed_version;
                    changed = true;
                }
            }
        }
    }
    changed
}

pub fn save_catalog(catalog: &GuidesCatalog) -> Result<(), String> {
    let path = catalog_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let raw = serde_json::to_string_pretty(catalog).map_err(|e| e.to_string())?;
    fs::write(&path, raw).map_err(|e| e.to_string())
}

fn refresh_installed_flags(catalog: &mut GuidesCatalog) {
    for pack in &mut catalog.packs {
        let dir = pack_dir(&pack.id);
        let manifest_path = dir.join("manifest.json");
        if manifest_path.is_file() {
            if let Ok(m) = load_manifest_file(&manifest_path) {
                pack.installed_version = Some(m.version);
                pack.status = "installed".into();
                continue;
            }
        }
        if pack.status == "installed" {
            pack.status = "available".into();
            pack.installed_version = None;
        } else if pack.status.is_empty() || pack.status == "unavailable" || pack.status == "available"
        {
            pack.status = if pack.is_installable() {
                "available".into()
            } else {
                "unavailable".into()
            };
        }
    }
}

/// Merge remote (or seed) pack list into the on-disk catalog without dropping install state.
pub fn sync_catalog() -> Result<GuidesCatalog, String> {
    set_job(job_busy("catalog", "Refreshing guide catalog…", ""));
    let seed = parse_seed();
    let remote_url = {
        let local = load_catalog();
        if !local.remote_url.trim().is_empty() {
            local.remote_url
        } else {
            seed.remote_url.clone()
        }
    };
    let fetched = if remote_url.trim().is_empty() {
        None
    } else {
        match http_get_text(&remote_url) {
            Ok(body) => match serde_json::from_str::<GuidesCatalog>(&body) {
                Ok(c) => Some(c),
                Err(err) => {
                    hq_log!(warn, "guides catalog parse failed: {err}");
                    None
                }
            },
            Err(err) => {
                hq_log!(warn, "guides catalog sync offline/failed: {err}");
                None
            }
        }
    };
    let incoming = fetched.unwrap_or(seed);
    let mut local = load_catalog();
    let installed: std::collections::HashMap<String, (Option<String>, String)> = local
        .packs
        .iter()
        .filter_map(|p| {
            if p.status == "installed" || p.installed_version.is_some() {
                Some((
                    p.id.clone(),
                    (p.installed_version.clone(), p.status.clone()),
                ))
            } else {
                None
            }
        })
        .collect();
    local.version = incoming.version.max(1);
    local.remote_url = if incoming.remote_url.trim().is_empty() {
        remote_url
    } else {
        incoming.remote_url
    };
    local.updated_at = unix_now();
    local.packs = incoming.packs;
    for pack in &mut local.packs {
        if let Some((ver, st)) = installed.get(&pack.id) {
            pack.installed_version = ver.clone();
            pack.status = st.clone();
        }
    }
    // Remote GitHub seed may lag a local build; never drop newer bundled packs.
    let _ = merge_bundled_seed(&mut local);
    refresh_installed_flags(&mut local);
    save_catalog(&local)?;
    set_job(job_idle(
        "done",
        format!("{} packs in catalog", local.packs.len()),
        "",
    ));
    Ok(local)
}

fn http_get_text(url: &str) -> Result<String, String> {
    let mut req = ureq::get(url)
        .set("User-Agent", USER_AGENT)
        .set("Accept", "application/json, text/plain, */*;q=0.8")
        .timeout(Duration::from_secs(30));
    if let Some(referer) = referer_for_url(url) {
        req = req.set("Referer", referer);
    }
    let resp = req.call().map_err(|e| format!("HTTP: {e}"))?;
    let status = resp.status();
    if !(200..300).contains(&status) {
        return Err(format!("{url}: status code {status}"));
    }
    resp.into_string().map_err(|e| format!("Read body: {e}"))
}

fn referer_for_url(url: &str) -> Option<&'static str> {
    let lower = url.to_ascii_lowercase();
    if lower.contains("://www.ready.gov/") || lower.contains("://ready.gov/") {
        Some("https://www.ready.gov/")
    } else if lower.contains("gutenberg.org") || lower.contains("pglaf.org") {
        Some("https://www.gutenberg.org/")
    } else {
        None
    }
}

/// Prefer the catalog URL, then known Gutenberg mirrors when the primary host is down.
fn download_url_candidates(url: &str) -> Vec<String> {
    let mut out = vec![url.to_string()];
    let lower = url.to_ascii_lowercase();
    let push_alt = |out: &mut Vec<String>, alt: String| {
        if !out.iter().any(|u| u == &alt) {
            out.push(alt);
        }
    };
    if let Some(rest) = lower
        .strip_prefix("https://www.gutenberg.org/")
        .or_else(|| lower.strip_prefix("http://www.gutenberg.org/"))
        .or_else(|| lower.strip_prefix("https://gutenberg.org/"))
        .or_else(|| lower.strip_prefix("http://gutenberg.org/"))
    {
        // Keep original path casing from `url` when possible.
        let path = url
            .find("gutenberg.org/")
            .map(|i| &url[i + "gutenberg.org/".len()..])
            .unwrap_or(rest);
        push_alt(&mut out, format!("https://gutenberg.pglaf.org/{path}"));
    } else if let Some(i) = lower.find("gutenberg.pglaf.org/") {
        let path = &url[i + "gutenberg.pglaf.org/".len()..];
        push_alt(
            &mut out,
            format!("https://www.gutenberg.org/{path}"),
        );
    }
    out
}

// ---------- Install / remove ----------

pub fn install_pack(pack_id: &str) -> Result<(), String> {
    let id = pack_id.trim();
    if id.is_empty() {
        return Err("Missing pack id.".into());
    }
    let catalog = load_catalog();
    let entry = catalog
        .packs
        .iter()
        .find(|p| p.id == id)
        .cloned()
        .ok_or_else(|| format!("Pack `{id}` is not in the catalog."))?;
    if entry.format == "zim" {
        return Err("Kiwix .zim packs are not supported in this version.".into());
    }
    set_job(job_busy("install", format!("Installing {}…", entry.name), id));
    let result: Result<(), String> = (|| {
        let dest = pack_dir(id);
        if dest.exists() {
            fs::remove_dir_all(&dest).map_err(|e| e.to_string())?;
        }
        fs::create_dir_all(&dest).map_err(|e| e.to_string())?;

        if let Some(key) = entry.bundled.as_deref() {
            set_job(job_busy("copy", "Copying bundled pack…", id));
            install_bundled(key, &dest)?;
        } else if !entry.files.is_empty() {
            set_job(job_busy("download", "Downloading remote files…", id));
            install_remote_files(&entry, &dest)?;
        } else if let Some(url) = entry.download_url.as_deref() {
            if url.trim().is_empty() {
                return Err("This pack has no download URL yet.".into());
            }
            set_job(job_busy("download", "Downloading pack…", id));
            let zip_path = downloads_dir().join(format!("{}.zip", sanitize_id(id)));
            download_file(
                url,
                &zip_path,
                entry.size_bytes,
                entry.sha256.as_deref(),
            )?;
            set_job(job_busy("extract", "Unpacking pack…", id));
            extract_zip(&zip_path, &dest)?;
            let _ = fs::remove_file(&zip_path);
            normalize_pack_root(&dest)?;
        } else {
            return Err("This pack is not downloadable yet.".into());
        }

        let manifest = load_manifest_file(&dest.join("manifest.json"))
            .map_err(|e| format!("Pack manifest: {e}"))?;
        if manifest.id != id && sanitize_id(&manifest.id) != sanitize_id(id) {
            hq_log!(
                warn,
                "guides pack id mismatch: catalog={id} manifest={}",
                manifest.id
            );
        }
        set_job(job_busy("index", "Indexing articles…", id));
        index_pack(&manifest, &dest)?;

        let mut catalog = load_catalog();
        if let Some(row) = catalog.packs.iter_mut().find(|p| p.id == id) {
            row.installed_version = Some(manifest.version.clone());
            row.status = "installed".into();
        }
        save_catalog(&catalog)?;
        Ok(())
    })();

    match &result {
        Ok(()) => set_job(job_idle("done", format!("Installed {id}"), id)),
        Err(err) => {
            let _ = fs::remove_dir_all(pack_dir(id));
            set_job(job_fail(id, err.clone()));
        }
    }
    result
}

fn remove_pack_files_only(pack_id: &str) -> Result<(), String> {
    let id = pack_id.trim();
    if id.is_empty() {
        return Err("Missing pack id.".into());
    }
    let dest = pack_dir(id);
    if dest.exists() {
        fs::remove_dir_all(&dest).map_err(|e| e.to_string())?;
    }
    purge_pack_from_index(id)?;
    Ok(())
}

pub fn remove_pack(pack_id: &str) -> Result<(), String> {
    let id = pack_id.trim();
    if id.is_empty() {
        return Err("Missing pack id.".into());
    }
    remove_pack_files_only(id)?;
    let mut catalog = load_catalog();
    if let Some(row) = catalog.packs.iter_mut().find(|p| p.id == id) {
        row.installed_version = None;
        row.status = if row.is_installable() {
            "available".into()
        } else {
            "unavailable".into()
        };
    }
    save_catalog(&catalog)?;
    Ok(())
}

fn install_remote_files(entry: &CatalogPack, dest: &Path) -> Result<(), String> {
    if entry.files.is_empty() {
        return Err("Pack has no remote files.".into());
    }
    let mut attachments = Vec::new();
    let mut articles = Vec::new();
    for (i, file) in entry.files.iter().enumerate() {
        if file.url.trim().is_empty() {
            return Err(format!("Remote file `{}` is missing a URL.", file.id));
        }
        if file.sha256.trim().is_empty() {
            return Err(format!(
                "Remote file `{}` is missing sha256 (required).",
                file.id
            ));
        }
        set_job(job_busy(
            "download",
            format!(
                "Downloading {} ({}/{})…",
                file.title,
                i + 1,
                entry.files.len()
            ),
            &entry.id,
        ));
        let rel = default_remote_rel_path(file)?;
        let out = dest.join(&rel);
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        download_file(
            file.url.trim(),
            &out,
            file.size_bytes,
            Some(file.sha256.trim()),
        )?;
        if remote_file_is_article(&rel) {
            if remote_file_is_html(&rel) {
                set_job(job_busy(
                    "download",
                    format!("Fetching images for {}…", file.title),
                    &entry.id,
                ));
                fetch_html_relative_assets(&out, file.url.trim(), dest)?;
            }
            articles.push(ArticleMeta {
                id: file.id.clone(),
                title: file.title.clone(),
                path: rel,
                tags: entry.tags.clone(),
                summary: entry.description.clone(),
            });
        } else {
            attachments.push(AttachmentMeta {
                id: file.id.clone(),
                title: file.title.clone(),
                path: rel,
            });
        }
    }

    write_synthesized_manifest(entry, dest, articles, attachments)
}

fn remote_file_is_html(rel: &str) -> bool {
    let l = rel.to_ascii_lowercase();
    l.ends_with(".html") || l.ends_with(".htm")
}

/// Download relative `img src` / CSS-less assets next to an HTML article (e.g. Gutenberg `images/`).
fn fetch_html_relative_assets(
    html_path: &Path,
    page_url: &str,
    _pack_root: &Path,
) -> Result<(), String> {
    let html = fs::read_to_string(html_path).map_err(|e| e.to_string())?;
    let base = page_url_base(page_url);
    let html_dir = html_path
        .parent()
        .ok_or_else(|| "HTML article has no parent directory.".to_string())?;
    let mut fetched = 0usize;
    for src in extract_relative_asset_srcs(&html) {
        let dest = html_dir.join(&src);
        if dest.exists() {
            continue;
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let url = format!("{base}{src}");
        match download_file(&url, &dest, None, None) {
            Ok(_) => fetched += 1,
            Err(err) => {
                hq_log!(warn, "guides html asset skipped ({src}): {err}");
                let _ = fs::remove_file(&dest);
            }
        }
    }
    if fetched > 0 {
        hq_log!(info, "guides fetched {fetched} HTML sidecar assets");
    }
    Ok(())
}

fn page_url_base(page_url: &str) -> String {
    match page_url.rsplit_once('/') {
        Some((base, _)) => format!("{base}/"),
        None => page_url.to_string(),
    }
}

fn extract_relative_asset_srcs(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let lower = html.to_ascii_lowercase();
    // Scan original string for src="..." using lowercase index alignment (ASCII attrs).
    let bytes = html.as_bytes();
    let lower_bytes = lower.as_bytes();
    let mut i = 0;
    while i + 5 < lower_bytes.len() {
        if lower_bytes[i..].starts_with(b"src=\"") {
            let start = i + 5;
            if let Some(rel) = lower_bytes[start..].iter().position(|&b| b == b'"') {
                let raw = &html[start..start + rel];
                if let Some(norm) = normalize_relative_asset_src(raw) {
                    if !out.iter().any(|s| s == &norm) {
                        out.push(norm);
                    }
                }
                i = start + rel + 1;
                continue;
            }
        }
        if lower_bytes[i..].starts_with(b"src='") {
            let start = i + 5;
            if let Some(rel) = bytes[start..].iter().position(|&b| b == b'\'') {
                let raw = &html[start..start + rel];
                if let Some(norm) = normalize_relative_asset_src(raw) {
                    if !out.iter().any(|s| s == &norm) {
                        out.push(norm);
                    }
                }
                i = start + rel + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

fn normalize_relative_asset_src(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty()
        || s.starts_with("data:")
        || s.starts_with("http://")
        || s.starts_with("https://")
        || s.starts_with('#')
        || s.contains("..")
    {
        return None;
    }
    let s = s.trim_start_matches("./");
    if s.is_empty() {
        return None;
    }
    Some(s.to_string())
}

fn default_remote_rel_path(file: &CatalogRemoteFile) -> Result<String, String> {
    if !file.path.trim().is_empty() {
        let p = file.path.trim().trim_start_matches(['/', '\\']);
        if p.is_empty() || p.contains("..") {
            return Err(format!("Invalid path for remote file `{}`.", file.id));
        }
        return Ok(p.to_string());
    }
    let url_l = file.url.to_ascii_lowercase();
    let ext = if url_l.contains(".txt") {
        "txt"
    } else if url_l.contains(".md") {
        "md"
    } else if url_l.contains(".html") || url_l.contains(".htm") {
        "html"
    } else {
        "pdf"
    };
    if ext == "pdf" {
        Ok(format!("attachments/{}.pdf", sanitize_id(&file.id)))
    } else {
        Ok(format!("articles/{}.{}", sanitize_id(&file.id), ext))
    }
}

fn remote_file_is_article(rel: &str) -> bool {
    let l = rel.to_ascii_lowercase();
    l.ends_with(".md") || l.ends_with(".txt") || l.ends_with(".html") || l.ends_with(".htm")
}

fn write_synthesized_manifest(
    entry: &CatalogPack,
    dest: &Path,
    mut articles: Vec<ArticleMeta>,
    attachments: Vec<AttachmentMeta>,
) -> Result<(), String> {
    let about_path = "articles/about.md";
    let about_body = synthesize_about_markdown(entry, &attachments, &articles);
    let about_full = dest.join(about_path);
    if let Some(parent) = about_full.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(&about_full, about_body).map_err(|e| e.to_string())?;

    if !articles.iter().any(|a| a.id == "about") {
        // PDF-only packs use the pack name as the sole row title; multi-article packs
        // get a short "About" label (hidden from search when siblings exist).
        let about_title = if articles.is_empty() {
            entry.name.clone()
        } else {
            "About".into()
        };
        articles.insert(
            0,
            ArticleMeta {
                id: "about".into(),
                title: about_title,
                path: about_path.into(),
                tags: entry.tags.clone(),
                summary: entry.description.clone(),
            },
        );
    }

    let manifest = PackManifest {
        id: entry.id.clone(),
        name: entry.name.clone(),
        version: entry.version.clone(),
        source: entry.source.clone(),
        license: entry.license.clone(),
        description: entry.description.clone(),
        categories: normalize_guide_categories(&entry.categories),
        articles,
        attachments,
    };
    let manifest_raw = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
    fs::write(dest.join("manifest.json"), manifest_raw).map_err(|e| e.to_string())?;
    Ok(())
}

fn synthesize_about_markdown(
    entry: &CatalogPack,
    attachments: &[AttachmentMeta],
    articles: &[ArticleMeta],
) -> String {
    let mut body = String::new();
    body.push_str(&format!("# {}\n\n", entry.name));
    body.push_str(&format!(
        "**Source:** {}  \n**License:** {}  \n**Version:** {}\n\n",
        entry.source, entry.license, entry.version
    ));
    if !entry.description.trim().is_empty() {
        body.push_str(entry.description.trim());
        body.push_str("\n\n");
    }
    body.push_str(
        "This pack was downloaded from upstream public URLs and stored only on this computer. \
         Metis Mesh HQ does not host the files.\n\n",
    );
    if !articles.is_empty() {
        body.push_str("## Articles in this pack\n\n");
        for art in articles {
            if art.id == "about" {
                continue;
            }
            body.push_str(&format!("- **{}** (`{}`)\n", art.title, art.path));
        }
        body.push('\n');
    }
    if !attachments.is_empty() {
        body.push_str("## PDF attachments\n\n");
        body.push_str("Open these in your system viewer from the Attachments section.\n\n");
        for att in attachments {
            body.push_str(&format!("- **{}** (`{}`)\n", att.title, att.path));
        }
        body.push('\n');
    }
    body.push_str(
        "## Disclaimer\n\nCommunity / government reference material only — not medical or legal advice. \
         Metis Mesh HQ is not FEMA, NWS, or law enforcement. Reprint US government materials without implying endorsement.\n",
    );
    body
}

fn install_bundled(key: &str, dest: &Path) -> Result<(), String> {
    let files = bundled_files(key).ok_or_else(|| format!("Unknown bundled pack `{key}`."))?;
    for (rel, body) in files {
        let path = dest.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        fs::write(&path, body).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// If the zip contained a single top-level folder, hoist its contents up.
fn normalize_pack_root(dest: &Path) -> Result<(), String> {
    if dest.join("manifest.json").is_file() {
        return Ok(());
    }
    let mut kids: Vec<PathBuf> = fs::read_dir(dest)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    if kids.len() != 1 || !kids[0].is_dir() {
        return Err("Pack zip has no manifest.json at the root.".into());
    }
    let inner = kids.remove(0);
    if !inner.join("manifest.json").is_file() {
        return Err("Pack zip has no manifest.json.".into());
    }
    for entry in fs::read_dir(&inner).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let to = dest.join(entry.file_name());
        fs::rename(entry.path(), &to).map_err(|e| e.to_string())?;
    }
    let _ = fs::remove_dir_all(&inner);
    Ok(())
}

fn load_manifest_file(path: &Path) -> Result<PackManifest, String> {
    let raw = fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&raw).map_err(|e| e.to_string())
}

pub fn load_installed_manifest(pack_id: &str) -> Result<PackManifest, String> {
    load_manifest_file(&pack_dir(pack_id).join("manifest.json"))
}

pub fn list_installed_packs() -> Vec<CatalogPack> {
    load_catalog()
        .packs
        .into_iter()
        .filter(|p| p.status == "installed")
        .collect()
}

// ---------- Download / zip ----------

fn download_file(
    url: &str,
    dest: &Path,
    expected_size: Option<u64>,
    expected_sha256: Option<&str>,
) -> Result<String, String> {
    let mut last_err = String::new();
    let candidates = download_url_candidates(url);
    for (ci, candidate) in candidates.iter().enumerate() {
        for attempt in 1..=DOWNLOAD_ATTEMPTS {
            match download_file_once(candidate, dest, expected_size, expected_sha256) {
                Ok(sha) => {
                    if ci > 0 {
                        hq_log!(
                            info,
                            "guides download used mirror {} for {}",
                            candidate,
                            url
                        );
                    }
                    return Ok(sha);
                }
                Err(err) => {
                    hq_log!(
                        warn,
                        "guides download attempt {attempt}/{DOWNLOAD_ATTEMPTS} ({candidate}) failed: {err}"
                    );
                    let _ = fs::remove_file(dest);
                    let hard = download_err_is_hard(&err);
                    last_err = err;
                    // 403/404/5xx won't recover on retry — jump to the next mirror now.
                    if hard {
                        break;
                    }
                }
            }
        }
    }
    Err(format!(
        "Download failed after trying {} URL(s): {last_err}",
        candidates.len()
    ))
}

fn download_err_is_hard(err: &str) -> bool {
    let e = err.to_ascii_lowercase();
    e.contains("status code 403")
        || e.contains("status code 404")
        || e.contains("status code 410")
        || e.contains("status code 5")
        || e.contains("corrupt (sha256")
}

fn download_file_once(
    url: &str,
    dest: &Path,
    expected_size: Option<u64>,
    expected_sha256: Option<&str>,
) -> Result<String, String> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = dest.with_extension("partial");
    let _ = fs::remove_file(&tmp);
    let accept = if url.to_ascii_lowercase().contains(".pdf") {
        "application/pdf,*/*;q=0.8"
    } else {
        "*/*"
    };
    let mut req = ureq::get(url)
        .set("User-Agent", USER_AGENT)
        .set("Accept", accept)
        .timeout(Duration::from_secs(600));
    if let Some(referer) = referer_for_url(url) {
        req = req.set("Referer", referer);
    }
    let resp = req
        .call()
        .map_err(|e| format!("Download failed: {url}: {e}"))?;
    let status = resp.status();
    if !(200..300).contains(&status) {
        return Err(format!("{url}: status code {status}"));
    }
    let header_len = resp
        .header("Content-Length")
        .and_then(|v| v.parse::<u64>().ok());
    let total = expected_size.or(header_len).unwrap_or(0);
    patch_job_progress("", 0, total);
    let mut reader = resp.into_reader();
    let mut file = File::create(&tmp).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    let mut written: u64 = 0;
    let mut last_report = 0u64;
    loop {
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        written += n as u64;
        if written > MAX_DOWNLOAD_BYTES {
            let _ = fs::remove_file(&tmp);
            return Err(format!(
                "Download exceeds {} MB limit.",
                MAX_DOWNLOAD_BYTES / (1024 * 1024)
            ));
        }
        if let Some(expected) = expected_size {
            if written > expected {
                let _ = fs::remove_file(&tmp);
                return Err("Download larger than expected size.".into());
            }
        }
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        hasher.update(&buf[..n]);
        if written.saturating_sub(last_report) >= 64 * 1024 || (total > 0 && written >= total) {
            last_report = written;
            patch_job_progress("", written, total);
        }
    }
    patch_job_progress("", written, if total > 0 { total } else { written });
    drop(file);
    if let Some(expected) = expected_size {
        if written != expected {
            let _ = fs::remove_file(&tmp);
            return Err(format!(
                "incomplete or corrupt (size): got {written}, expected {expected}"
            ));
        }
    }
    let sha = hex::encode(hasher.finalize());
    if let Some(expected) = expected_sha256 {
        if !sha.eq_ignore_ascii_case(expected) {
            let _ = fs::remove_file(&tmp);
            return Err(format!(
                "corrupt (sha256 mismatch): got {sha}, expected {expected}"
            ));
        }
    }
    fs::rename(&tmp, dest).map_err(|e| e.to_string())?;
    Ok(sha)
}

fn extract_zip(archive: &Path, dest: &Path) -> Result<(), String> {
    let file = File::open(archive).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
        let name = entry
            .enclosed_name()
            .ok_or_else(|| "Unsafe zip entry path.".to_string())?
            .to_path_buf();
        let out = dest.join(&name);
        if entry.is_dir() {
            fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut outfile = File::create(&out).map_err(|e| e.to_string())?;
        std::io::copy(&mut entry, &mut outfile).map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ---------- FTS index ----------

fn open_index() -> Result<Connection, String> {
    let path = index_db_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let conn = Connection::open(&path).map_err(|e| e.to_string())?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         CREATE TABLE IF NOT EXISTS articles (
             pack_id TEXT NOT NULL,
             article_id TEXT NOT NULL,
             title TEXT NOT NULL,
             pack_name TEXT NOT NULL,
             tags TEXT NOT NULL,
             body TEXT NOT NULL,
             categories TEXT NOT NULL DEFAULT '',
             PRIMARY KEY (pack_id, article_id)
         );
         CREATE VIRTUAL TABLE IF NOT EXISTS articles_fts USING fts5(
             pack_id UNINDEXED,
             article_id UNINDEXED,
             title,
             pack_name UNINDEXED,
             tags,
             body,
             content='articles',
             content_rowid='rowid'
         );
         CREATE TRIGGER IF NOT EXISTS articles_ai AFTER INSERT ON articles BEGIN
             INSERT INTO articles_fts(rowid, pack_id, article_id, title, pack_name, tags, body)
             VALUES (new.rowid, new.pack_id, new.article_id, new.title, new.pack_name, new.tags, new.body);
         END;
         CREATE TRIGGER IF NOT EXISTS articles_ad AFTER DELETE ON articles BEGIN
             INSERT INTO articles_fts(articles_fts, rowid, pack_id, article_id, title, pack_name, tags, body)
             VALUES ('delete', old.rowid, old.pack_id, old.article_id, old.title, old.pack_name, old.tags, old.body);
         END;
         CREATE TRIGGER IF NOT EXISTS articles_au AFTER UPDATE ON articles BEGIN
             INSERT INTO articles_fts(articles_fts, rowid, pack_id, article_id, title, pack_name, tags, body)
             VALUES ('delete', old.rowid, old.pack_id, old.article_id, old.title, old.pack_name, old.tags, old.body);
             INSERT INTO articles_fts(rowid, pack_id, article_id, title, pack_name, tags, body)
             VALUES (new.rowid, new.pack_id, new.article_id, new.title, new.pack_name, new.tags, new.body);
         END;",
    )
    .map_err(|e| e.to_string())?;
    // Older installs created `articles` without categories — add the column when missing.
    let has_categories = conn
        .prepare("SELECT categories FROM articles LIMIT 1")
        .is_ok();
    if !has_categories {
        conn.execute(
            "ALTER TABLE articles ADD COLUMN categories TEXT NOT NULL DEFAULT ''",
            [],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(conn)
}

fn resolve_categories_for_index(manifest: &PackManifest) -> String {
    if !manifest.categories.is_empty() {
        return categories_index_value(&manifest.categories);
    }
    load_catalog()
        .packs
        .iter()
        .find(|p| p.id == manifest.id)
        .map(|p| categories_index_value(&p.categories))
        .unwrap_or_default()
}

fn index_pack(manifest: &PackManifest, root: &Path) -> Result<(), String> {
    let conn = open_index()?;
    conn.execute(
        "DELETE FROM articles WHERE pack_id = ?1",
        params![manifest.id],
    )
    .map_err(|e| e.to_string())?;
    let categories = resolve_categories_for_index(manifest);
    for article in &manifest.articles {
        let path = resolve_pack_path(root, &article.path)?;
        let raw = fs::read_to_string(&path).unwrap_or_default();
        let path_l = article.path.to_ascii_lowercase();
        // Index searchable text — strip HTML tags so FTS isn't polluted with markup.
        let body = if path_l.ends_with(".html") || path_l.ends_with(".htm") {
            html_to_export_markdown(&raw)
        } else if path_l.ends_with(".txt") {
            plain_text_to_markdown(&raw)
        } else {
            raw
        };
        let tags = article.tags.join(" ");
        conn.execute(
            "INSERT INTO articles (pack_id, article_id, title, pack_name, tags, body, categories)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                manifest.id,
                article.id,
                article.title,
                manifest.name,
                tags,
                body,
                categories
            ],
        )
        .map_err(|e| e.to_string())?;
    }
    // Index attachment titles (filename only — not PDF body).
    for att in &manifest.attachments {
        let stub = format!("{} {}", att.title, att.path);
        conn.execute(
            "INSERT INTO articles (pack_id, article_id, title, pack_name, tags, body, categories)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                manifest.id,
                format!("att:{}", att.id),
                att.title,
                manifest.name,
                "pdf attachment",
                stub,
                categories
            ],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn purge_pack_from_index(pack_id: &str) -> Result<(), String> {
    let conn = open_index()?;
    conn.execute("DELETE FROM articles WHERE pack_id = ?1", params![pack_id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn resolve_pack_path(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let rel = rel.trim_start_matches(['/', '\\']);
    if rel.is_empty() || rel.contains("..") {
        return Err("Invalid article path.".into());
    }
    let path = root.join(rel);
    let root_canon = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let path_canon = path.canonicalize().map_err(|e| format!("Missing file: {e}"))?;
    if !path_canon.starts_with(&root_canon) {
        return Err("Article path escapes pack directory.".into());
    }
    Ok(path_canon)
}

/// Category token match on space-separated `articles.categories` (not FTS).
const CATEGORY_MATCH: &str =
    "(?{n} IS NULL OR instr(' ' || categories || ' ', ' ' || ?{n} || ' ') > 0)";
const CATEGORY_MATCH_A: &str =
    "(?{n} IS NULL OR instr(' ' || a.categories || ' ', ' ' || ?{n} || ' ') > 0)";

pub fn search(
    query: &str,
    pack_filter: Option<&str>,
    category_filter: Option<&str>,
) -> Result<Vec<SearchHit>, String> {
    let conn = open_index()?;
    let q = query.trim();
    let mut hits = Vec::new();
    let cat = category_filter
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter(|s| is_known_guide_category(s));
    // Hide synthesized "about" when the pack also has real articles — same pack, not a duplicate.
    // About stays reachable from the reader sibling pills. PDF-only packs keep about as the row.
    const HIDE_REDUNDANT_ABOUT: &str = "
                 AND NOT (
                   article_id = 'about'
                   AND EXISTS (
                     SELECT 1 FROM articles other
                     WHERE other.pack_id = articles.pack_id
                       AND other.article_id != 'about'
                       AND other.article_id NOT LIKE 'att:%'
                   )
                 )";
    const HIDE_REDUNDANT_ABOUT_A: &str = "
                 AND NOT (
                   a.article_id = 'about'
                   AND EXISTS (
                     SELECT 1 FROM articles other
                     WHERE other.pack_id = a.pack_id
                       AND other.article_id != 'about'
                       AND other.article_id NOT LIKE 'att:%'
                   )
                 )";

    if q.is_empty() {
        let cat_sql = CATEGORY_MATCH.replace("{n}", "2");
        let sql = format!(
            "SELECT pack_id, article_id, title, pack_name, tags, substr(body, 1, 160)
             FROM articles
             WHERE (?1 IS NULL OR pack_id = ?1)
               AND {cat_sql}
               AND article_id NOT LIKE 'att:%'
               {HIDE_REDUNDANT_ABOUT}
             ORDER BY pack_name COLLATE NOCASE, title COLLATE NOCASE
             LIMIT 200"
        );
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![pack_filter, cat], |row| {
                Ok(SearchHit {
                    pack_id: row.get(0)?,
                    article_id: row.get(1)?,
                    title: row.get(2)?,
                    pack_name: row.get(3)?,
                    tags: split_tags(row.get::<_, String>(4)?),
                    snippet: row.get::<_, String>(5)?.replace('\n', " "),
                })
            })
            .map_err(|e| e.to_string())?;
        for row in rows {
            hits.push(row.map_err(|e| e.to_string())?);
        }
        return Ok(hits);
    }

    let fts_query = fts_query_from_user(q);
    if fts_query.is_empty() {
        return search("", pack_filter, category_filter);
    }
    let cat_sql = CATEGORY_MATCH_A.replace("{n}", "3");
    let sql = if pack_filter.is_some() {
        format!(
            "SELECT a.pack_id, a.article_id, a.title, a.pack_name, a.tags,
                    snippet(articles_fts, 5, '', '', '…', 24)
             FROM articles_fts
             JOIN articles a ON a.rowid = articles_fts.rowid
             WHERE articles_fts MATCH ?1 AND a.pack_id = ?2 AND {cat_sql}
               AND a.article_id NOT LIKE 'att:%'
               {HIDE_REDUNDANT_ABOUT_A}
             ORDER BY rank
             LIMIT 100"
        )
    } else {
        let cat_sql2 = CATEGORY_MATCH_A.replace("{n}", "2");
        format!(
            "SELECT a.pack_id, a.article_id, a.title, a.pack_name, a.tags,
                    snippet(articles_fts, 5, '', '', '…', 24)
             FROM articles_fts
             JOIN articles a ON a.rowid = articles_fts.rowid
             WHERE articles_fts MATCH ?1 AND {cat_sql2}
               AND a.article_id NOT LIKE 'att:%'
               {HIDE_REDUNDANT_ABOUT_A}
             ORDER BY rank
             LIMIT 100"
        )
    };
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let map_row = |row: &rusqlite::Row<'_>| -> rusqlite::Result<SearchHit> {
        Ok(SearchHit {
            pack_id: row.get(0)?,
            article_id: row.get(1)?,
            title: row.get(2)?,
            pack_name: row.get(3)?,
            tags: split_tags(row.get::<_, String>(4)?),
            snippet: row.get::<_, String>(5)?.replace('\n', " "),
        })
    };
    let rows = if let Some(pf) = pack_filter {
        stmt.query_map(params![fts_query, pf, cat], map_row)
    } else {
        stmt.query_map(params![fts_query, cat], map_row)
    }
    .map_err(|e| e.to_string())?;
    for row in rows {
        hits.push(row.map_err(|e| e.to_string())?);
    }
    Ok(hits)
}

fn split_tags(raw: String) -> Vec<String> {
    raw.split_whitespace()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

fn fts_query_from_user(q: &str) -> String {
    q.split_whitespace()
        .filter(|t| !t.is_empty())
        .map(|t| {
            let cleaned: String = t
                .chars()
                .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
                .collect();
            if cleaned.is_empty() {
                return String::new();
            }
            format!("\"{cleaned}\"*")
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

// ---------- Load / render article ----------

pub fn load_article(pack_id: &str, article_id: &str) -> Result<LoadedArticle, String> {
    let manifest = load_installed_manifest(pack_id)?;
    let article = manifest
        .articles
        .iter()
        .find(|a| a.id == article_id)
        .cloned()
        .ok_or_else(|| format!("Article `{article_id}` not found in pack."))?;
    let root = pack_dir(pack_id);
    let path = resolve_pack_path(&root, &article.path)?;
    let raw = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let path_l = article.path.to_ascii_lowercase();
    let article_dir = path.parent();
    let (markdown, html) = if path_l.ends_with(".html") || path_l.ends_with(".htm") {
        // Keep structure + local images for the reader; derive Markdown for Export MD/PDF.
        let display = html_article_to_safe_html(&raw, &root, article_dir)?;
        let export_md = html_to_export_markdown(&raw);
        (export_md, display)
    } else if path_l.ends_with(".txt") {
        let md = plain_text_to_markdown(&raw);
        let html = markdown_to_safe_html(&md, &root, article_dir)?;
        (md, html)
    } else {
        let md = raw;
        let html = markdown_to_safe_html(&md, &root, article_dir)?;
        (md, html)
    };
    Ok(LoadedArticle {
        pack_id: pack_id.into(),
        pack_name: manifest.name,
        article,
        body_markdown: markdown,
        body_html: html,
        siblings: manifest.articles,
    })
}

/// Normalize curly quotes / dashes so WebView never shows mojibake for common Gutenberg glyphs.
fn normalize_guide_plaintext(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{2032}' => out.push('\''),
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{2033}' => out.push('"'),
            '\u{2013}' | '\u{2014}' | '\u{2212}' => out.push('-'),
            '\u{2026}' => out.push_str("..."),
            '\u{00A0}' => out.push(' '),
            '\u{FFFD}' => {} // drop replacement chars from prior bad decodes
            c => out.push(c),
        }
    }
    out
}

/// Page refs at end of an index line: `93`, `14-17`, `42, 45, 47`, `49 ff.`, `_Frontispiece_`.
fn index_line_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?x)
            ^\s*
            (?P<topic>.+?)
            \s+
            (?P<pages>
                \d{1,3}(?:-\d{1,3})?(?:\s*ff\.)?
                (?:,\s*\d{1,3}(?:-\d{1,3})?(?:\s*ff\.)?)*
                | _Frontispiece_
            )
            \s*$",
        )
        .expect("index line regex")
    })
}

fn split_index_entry(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.len() > 120 {
        return None;
    }
    let caps = index_line_re().captures(trimmed)?;
    let topic = caps.name("topic")?.as_str().trim();
    let pages = caps.name("pages")?.as_str().trim();
    if topic.is_empty() || pages.is_empty() {
        return None;
    }
    if topic.split_whitespace().count() > 12 {
        return None;
    }
    // Keep surrounding quotes on topic when present (e.g. "punkies").
    Some((topic.to_string(), pages.to_string()))
}

fn is_index_heading_line(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() || t.len() > 48 {
        return false;
    }
    if split_index_entry(t).is_some() {
        return false;
    }
    // Short label like "Recipes", "canvas", "Fire" — no terminal punctuation of prose.
    if t.ends_with('.') || t.ends_with(',') || t.ends_with(';') {
        return false;
    }
    let letters: String = t.chars().filter(|c| c.is_alphabetic()).collect();
    !letters.is_empty() && t.split_whitespace().count() <= 5
}

fn is_index_block(lines: &[&str]) -> bool {
    if lines.len() < 2 {
        return false;
    }
    let with_pages = lines
        .iter()
        .filter(|l| split_index_entry(l).is_some())
        .count();
    // Majority of lines carry page numbers (Gutenberg index / recipes list).
    with_pages >= 2 && with_pages * 2 >= lines.len()
}

fn emit_index_table(lines: &[&str], out: &mut String) {
    out.push_str("| Topic | Pages |\n| --- | --- |\n");
    for line in lines {
        if let Some((topic, pages)) = split_index_entry(line) {
            let topic = topic.replace('|', "\\|");
            let pages = pages.replace('|', "\\|");
            out.push_str("| ");
            out.push_str(&topic);
            out.push_str(" | ");
            out.push_str(&pages);
            out.push_str(" |\n");
        } else if is_index_heading_line(line) {
            let topic = line.trim().replace('|', "\\|");
            out.push_str("| **");
            out.push_str(&topic);
            out.push_str("** |  |\n");
        }
    }
    out.push('\n');
}

/// Turn Gutenberg-style plain text into readable Markdown (paragraphs + light headings).
pub fn plain_text_to_markdown(raw: &str) -> String {
    let text = normalize_guide_plaintext(raw)
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let mut out = String::with_capacity(text.len() + 64);
    for block in text.split("\n\n") {
        let lines: Vec<&str> = block
            .lines()
            .map(str::trim_end)
            .filter(|l| !l.is_empty())
            .collect();
        if lines.is_empty() {
            continue;
        }
        if is_index_block(&lines) {
            emit_index_table(&lines, &mut out);
            continue;
        }
        let joined = lines.join(" ");
        let trimmed = joined.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Short ALL-CAPS / chapter-like lines → heading.
        let is_heading = {
            let letters: String = trimmed
                .chars()
                .filter(|c| c.is_alphabetic())
                .collect();
            let upper = !letters.is_empty()
                && letters.chars().all(|c| c.is_uppercase())
                && trimmed.len() <= 80
                && lines.len() == 1;
            let chapter = trimmed.to_ascii_lowercase().starts_with("chapter ")
                || trimmed.to_ascii_lowercase().starts_with("part ");
            upper || chapter
        };
        if is_heading {
            out.push_str("## ");
            out.push_str(trimmed);
            out.push_str("\n\n");
        } else if trimmed.chars().all(|c| c == '*' || c == '-' || c == '=' || c.is_whitespace())
            && trimmed.len() >= 3
        {
            out.push_str("---\n\n");
        } else if lines.len() == 1 {
            if let Some((topic, pages)) = split_index_entry(trimmed) {
                // Lone index row — keep as a compact two-column line.
                out.push_str("| ");
                out.push_str(&topic.replace('|', "\\|"));
                out.push_str(" | ");
                out.push_str(&pages.replace('|', "\\|"));
                out.push_str(" |\n\n");
                continue;
            }
            out.push_str(trimmed);
            out.push_str("\n\n");
        } else {
            out.push_str(trimmed);
            out.push_str("\n\n");
        }
    }
    // Merge consecutive single-row tables into one table for denser index sections.
    coalesce_adjacent_index_rows(&out)
}

/// Turn runs of single `| topic | pages |` paragraphs into one table with a header.
fn coalesce_adjacent_index_rows(md: &str) -> String {
    let mut out = String::with_capacity(md.len() + 32);
    let mut pending: Vec<(String, String)> = Vec::new();
    let flush = |pending: &mut Vec<(String, String)>, out: &mut String| {
        if pending.is_empty() {
            return;
        }
        out.push_str("| Topic | Pages |\n| --- | --- |\n");
        for (t, p) in pending.iter() {
            out.push_str("| ");
            out.push_str(t);
            out.push_str(" | ");
            out.push_str(p);
            out.push_str(" |\n");
        }
        out.push('\n');
        pending.clear();
    };
    for block in md.split("\n\n") {
        let t = block.trim();
        // Single table row without header: `| topic | pages |`
        let is_lone_row = t.starts_with('|')
            && t.ends_with('|')
            && !t.contains("---")
            && t.matches('|').count() == 3
            && !t.contains('\n');
        if is_lone_row {
            let parts: Vec<&str> = t.split('|').map(str::trim).filter(|s| !s.is_empty()).collect();
            if parts.len() == 2 {
                pending.push((parts[0].to_string(), parts[1].to_string()));
                continue;
            }
        }
        flush(&mut pending, &mut out);
        if !t.is_empty() {
            out.push_str(t);
            out.push_str("\n\n");
        }
    }
    flush(&mut pending, &mut out);
    out
}

/// Reader HTML: extract `<body>`, drop Gutenberg chrome / scripts / styles, keep safe markup + images.
fn html_article_to_safe_html(
    raw: &str,
    pack_root: &Path,
    article_dir: Option<&Path>,
) -> Result<String, String> {
    let body = extract_html_body(raw);
    let cleaned = strip_pg_chrome(&body);
    let sanitized = sanitize_html(&cleaned);
    // Page markers (`[Pg 12]`) become spans; TOC hash links become in-app jump buttons.
    let anchored = rewrite_guide_anchors(&sanitized);
    // Contents / Illustrations: wrap chapter labels & titles so the whole row is an obvious jump.
    let linked = linkify_guide_toc_rows(&anchored);
    // Serve illustrations via `metis://` (same idea as html_view local_file) — no data-URL bloat.
    Ok(rewrite_local_images(&linked, pack_root, article_dir))
}

/// Gutenberg uses `<a id="Page_N">[Pg N]</a>` as destinations and `<a href="#Page_N">` as TOC/index
/// jumps. Convert id-only anchors to spans so they don't look like dead links; keep hash links.
fn rewrite_guide_anchors(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let lb = lower.as_bytes();
    let mut out = String::with_capacity(html.len() + 32);
    let mut i = 0usize;
    while i < lb.len() {
        let is_a = lb[i..].starts_with(b"<a ")
            || lb[i..].starts_with(b"<a>")
            || lb[i..].starts_with(b"<a\n")
            || lb[i..].starts_with(b"<a\t")
            || lb[i..].starts_with(b"<a\r");
        if !is_a {
            // Advance one UTF-8 char (i is always a char boundary).
            let ch = html[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }
        let Some(gt) = lower[i..].find('>') else {
            out.push_str(&html[i..]);
            break;
        };
        let tag_end = i + gt; // index of '>'
        let open = &html[i..=tag_end];
        let open_l = open.to_ascii_lowercase();
        let has_href = open_l.contains("href=")
            && !open_l.contains("href=\"\"")
            && !open_l.contains("href=''");
        let hash_href = attr_value(open.trim_start_matches('<').trim_end_matches('>'), "href")
            .map(|h| h.starts_with('#'))
            .unwrap_or(false);
        let id_attr = attr_value(open.trim_start_matches('<').trim_end_matches('>'), "id");

        // Find matching </a>
        let after = tag_end + 1;
        let Some(rel_close) = lower[after..].find("</a>") else {
            out.push_str(open);
            i = after;
            continue;
        };
        let close_at = after + rel_close;
        let inner = &html[after..close_at];
        let after_close = close_at + 4;

        if hash_href {
            // TOC / index jump — <button>, not <a href>, so Dioxus/wry never treats
            // the click as navigation (http(s) clicks are force-opened in the OS browser).
            let href = attr_value(open.trim_start_matches('<').trim_end_matches('>'), "href")
                .unwrap_or_default();
            let jump = href.trim_start_matches('#');
            out.push_str(&guides_jump_open_tag(jump));
            out.push_str(inner);
            out.push_str("</button>");
        } else if !has_href {
            // Print-page marker, e.g. [Pg 12]: keep id as TOC destination and make it a
            // jump control so clicking snaps that page start under the sticky chrome.
            let id = id_attr.unwrap_or_default();
            if id.is_empty() {
                out.push_str("<span class=\"guides-pgmark\">");
                out.push_str(inner);
                out.push_str("</span>");
            } else {
                out.push_str("<button type=\"button\" class=\"guides-pgmark guides-jump\" id=\"");
                out.push_str(&escape_attr(&id));
                out.push_str("\" data-guides-jump=\"");
                out.push_str(&escape_attr(&id));
                out.push_str("\">");
                out.push_str(inner);
                out.push_str("</button>");
            }
        } else {
            // Remote / non-hash anchors — drop navigation; keep visible text.
            out.push_str("<span class=\"guides-dead-link\">");
            out.push_str(inner);
            out.push_str("</span>");
        }
        i = after_close;
    }
    out
}

fn escape_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// In-app jump control — never an `<a href>` (avoids wry → system browser).
fn guides_jump_open_tag(jump_id: &str) -> String {
    format!(
        "<button type=\"button\" class=\"guides-jump\" data-guides-jump=\"{}\">",
        escape_attr(jump_id)
    )
}

/// In Contents/Illustrations tables, wrap the label cell with the same `#Page_N` jump as the
/// page-number cell so chapter names and illustration titles are clearly clickable.
fn linkify_guide_toc_rows(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len() + 64);
    let mut cursor = 0usize;
    let mut search_from = 0usize;
    let mut prev_href: Option<String> = None;
    while let Some(rel) = lower[search_from..].find("<tr") {
        let tr_start = search_from + rel;
        out.push_str(&html[cursor..tr_start]);
        let Some(gt) = lower[tr_start..].find('>') else {
            out.push_str(&html[tr_start..]);
            return out;
        };
        let after_open = tr_start + gt + 1;
        let Some(rel_close) = lower[after_open..].find("</tr>") else {
            out.push_str(&html[tr_start..]);
            return out;
        };
        let tr_end = after_open + rel_close + 5; // include </tr>
        let row = &html[tr_start..tr_end];
        let row_l = row.to_ascii_lowercase();
        let own_href = find_hash_href(&row_l).and_then(|at| extract_hash_href(row, at));
        let wrap_href = own_href.clone().or_else(|| {
            // Contents title rows sit under the CHAPTER+page row — reuse prior jump.
            if row_looks_like_toc_title(row) {
                prev_href.clone()
            } else {
                None
            }
        });
        out.push_str(&linkify_one_toc_row(row, wrap_href.as_deref()));
        if own_href.is_some() {
            prev_href = own_href;
        } else if !row_looks_like_toc_title(row) {
            prev_href = None;
        }
        cursor = tr_end;
        search_from = tr_end;
    }
    out.push_str(&html[cursor..]);
    out
}

fn row_looks_like_toc_title(row: &str) -> bool {
    let lower = row.to_ascii_lowercase();
    if find_hash_href(&lower).is_some() {
        return false;
    }
    let cells = split_td_cells(row);
    if cells.is_empty() {
        return false;
    }
    let inner = cells[0].1.trim();
    if inner.is_empty() || inner.len() < 8 {
        return false;
    }
    // Title blurbs are prose, not "CHAPTER VII" headers.
    !inner.to_ascii_uppercase().starts_with("CHAPTER ")
        && !inner.eq_ignore_ascii_case("page")
}

fn linkify_one_toc_row(row: &str, wrap_href: Option<&str>) -> String {
    let Some(href) = wrap_href.filter(|h| h.starts_with('#')) else {
        return row.to_string();
    };
    let jump = href.trim_start_matches('#');
    let lower = row.to_ascii_lowercase();
    let cells = split_td_cells(row);
    if cells.is_empty() {
        return row.to_string();
    }
    let (c0_open, c0_inner, c0_close) = &cells[0];
    if !c0_open.to_ascii_lowercase().starts_with("<td") {
        return row.to_string();
    }
    if c0_inner.trim().is_empty() {
        return row.to_string();
    }
    if c0_inner.to_ascii_lowercase().contains("href=\"#")
        || c0_inner.to_ascii_lowercase().contains("data-guides-jump")
    {
        return row.to_string();
    }
    let Some(td_at) = lower.find("<td") else {
        return row.to_string();
    };
    let mut rebuilt = String::with_capacity(row.len() + 64);
    rebuilt.push_str(&row[..td_at]);
    rebuilt.push_str(c0_open);
    rebuilt.push_str(&guides_jump_open_tag(jump));
    rebuilt.push_str(c0_inner);
    rebuilt.push_str("</button>");
    rebuilt.push_str(c0_close);
    for (open, inner, close) in cells.iter().skip(1) {
        rebuilt.push_str(open);
        rebuilt.push_str(inner);
        rebuilt.push_str(close);
    }
    if let Some(last) = lower.rfind("</td>") {
        rebuilt.push_str(&row[last + 5..]);
    } else if let Some(last) = lower.rfind("</th>") {
        rebuilt.push_str(&row[last + 5..]);
    }
    rebuilt
}

fn find_hash_href(lower_row: &str) -> Option<usize> {
    // After rewrite_guide_anchors, jumps are data-guides-jump="…"; also accept raw href="#…".
    lower_row
        .find("data-guides-jump=\"")
        .or_else(|| lower_row.find("data-guides-jump='"))
        .or_else(|| lower_row.find("href=\"#"))
        .or_else(|| lower_row.find("href='#"))
}

fn extract_hash_href(row: &str, href_at: usize) -> Option<String> {
    let rest = &row[href_at..];
    let rest_l = rest.to_ascii_lowercase();
    let (prefix, quote, add_hash) = if rest_l.starts_with("data-guides-jump=\"") {
        ("data-guides-jump=\"", '"', true)
    } else if rest_l.starts_with("data-guides-jump='") {
        ("data-guides-jump='", '\'', true)
    } else if rest_l.starts_with("href=\"#") {
        ("href=\"", '"', false)
    } else if rest_l.starts_with("href='#") {
        ("href='", '\'', false)
    } else {
        return None;
    };
    let val_start = prefix.len();
    let end = rest[val_start..].find(quote)?;
    let mut val = rest[val_start..val_start + end].to_string();
    if add_hash && !val.starts_with('#') {
        val.insert(0, '#');
    }
    Some(val)
}

fn split_td_cells(row: &str) -> Vec<(String, String, String)> {
    let lower = row.to_ascii_lowercase();
    let lb = lower.as_bytes();
    let mut cells = Vec::new();
    let mut i = 0usize;
    while i < lb.len() {
        let open_tag = if lb[i..].starts_with(b"<td") {
            "td"
        } else if lb[i..].starts_with(b"<th") {
            "th"
        } else {
            i += 1;
            continue;
        };
        // Only count real tags: <td / <td> / <th …
        let next = lb.get(i + 3).copied().unwrap_or(0);
        if next != b'>' && next != b' ' && next != b'\n' && next != b'\t' && next != b'/' {
            i += 1;
            continue;
        }
        let open_end = match lower[i..].find('>') {
            Some(n) => i + n + 1,
            None => break,
        };
        let close = format!("</{open_tag}>");
        let Some(rel_close) = lower[open_end..].find(&close) else {
            break;
        };
        let close_at = open_end + rel_close;
        let close_end = close_at + close.len();
        cells.push((
            row[i..open_end].to_string(),
            row[open_end..close_at].to_string(),
            row[close_at..close_end].to_string(),
        ));
        i = close_end;
    }
    cells
}

fn extract_html_body(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let start = lower
        .find("<body")
        .and_then(|i| lower[i..].find('>').map(|j| i + j + 1))
        .unwrap_or(0);
    let end = lower.rfind("</body>").unwrap_or(html.len());
    if start < end {
        html[start..end].to_string()
    } else {
        html.to_string()
    }
}

/// Remove Project Gutenberg header/footer blocks when present (id=pg-header / pg-footer).
fn strip_pg_chrome(html: &str) -> String {
    let mut out = html.to_string();
    for id in ["pg-header", "pg-footer"] {
        out = strip_element_by_id(&out, id);
    }
    // Drop embedded author CSS — our reader styles the article.
    out = strip_tags_with_content(&out, "style");
    out = strip_tags_with_content(&out, "script");
    out
}

fn strip_element_by_id(html: &str, id: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let needle = format!("id=\"{id}\"");
    let needle2 = format!("id='{id}'");
    let Some(id_at) = lower
        .find(&needle)
        .or_else(|| lower.find(&needle2))
    else {
        return html.to_string();
    };
    // Walk back to the opening '<' (ASCII — always a char boundary).
    let open = html[..id_at].rfind('<').unwrap_or(id_at);
    let after_open = match html[open..].find('>') {
        Some(n) => open + n + 1,
        None => return html.to_string(),
    };
    // Match nested <div>…</div> on bytes so UTF-8 text (em dashes, etc.) cannot panic.
    let bytes = lower.as_bytes();
    if after_open > bytes.len() {
        return html.to_string();
    }
    let mut depth = 1i32;
    let mut i = after_open;
    while i < bytes.len() && depth > 0 {
        if bytes[i..].starts_with(b"<div") {
            let next = bytes.get(i + 4).copied().unwrap_or(0);
            if next == b'>' || next == b' ' || next == b'\n' || next == b'\t' || next == b'/' {
                depth += 1;
            }
            i += 4;
            continue;
        }
        if bytes[i..].starts_with(b"</div>") {
            depth -= 1;
            if depth == 0 {
                let end = i + b"</div>".len();
                if !html.is_char_boundary(open) || !html.is_char_boundary(end) {
                    return html.to_string();
                }
                let mut out = String::with_capacity(html.len());
                out.push_str(&html[..open]);
                out.push_str(&html[end..]);
                return out;
            }
            i += 6;
            continue;
        }
        i += 1;
    }
    html.to_string()
}

fn strip_tags_with_content(html: &str, tag: &str) -> String {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut cursor = 0usize;
    while let Some(rel) = lower[cursor..].find(&open) {
        let start = cursor + rel;
        out.push_str(&html[cursor..start]);
        let after = start + open.len();
        if let Some(rel_close) = lower[after..].find(&close) {
            cursor = after + rel_close + close.len();
        } else if let Some(gt) = lower[after..].find('>') {
            cursor = after + gt + 1;
        } else {
            cursor = start;
            break;
        }
    }
    out.push_str(&html[cursor..]);
    out
}

/// Export / PDF text: structured Markdown derived from HTML (headings, paragraphs, image alts).
fn html_to_export_markdown(html: &str) -> String {
    let body = strip_pg_chrome(&extract_html_body(html));
    let mut out = String::with_capacity(body.len() / 2);
    let mut in_tag = false;
    let mut tag = String::new();
    let mut pending_space = false;
    let flush_space = |out: &mut String, pending: &mut bool| {
        if *pending && !out.ends_with(['\n', ' ']) && !out.is_empty() {
            out.push(' ');
        }
        *pending = false;
    };
    for c in body.chars() {
        if c == '<' {
            in_tag = true;
            tag.clear();
            continue;
        }
        if c == '>' && in_tag {
            in_tag = false;
            let t = tag.to_ascii_lowercase();
            let name = t
                .trim()
                .trim_start_matches('/')
                .split(|ch: char| ch.is_whitespace() || ch == '/')
                .next()
                .unwrap_or("");
            let closing = t.trim_start().starts_with('/');
            match name {
                "br" => {
                    out.push('\n');
                    pending_space = false;
                }
                "p" | "div" | "tr" | "li" | "blockquote" if closing => {
                    out.push_str("\n\n");
                    pending_space = false;
                }
                "h1" if !closing => {
                    out.push_str("\n\n# ");
                    pending_space = false;
                }
                "h2" if !closing => {
                    out.push_str("\n\n## ");
                    pending_space = false;
                }
                "h3" if !closing => {
                    out.push_str("\n\n### ");
                    pending_space = false;
                }
                "h4" | "h5" | "h6" if !closing => {
                    out.push_str("\n\n#### ");
                    pending_space = false;
                }
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" if closing => {
                    out.push_str("\n\n");
                    pending_space = false;
                }
                "img" => {
                    if let Some(alt) = attr_value(&tag, "alt").filter(|s| !s.is_empty()) {
                        flush_space(&mut out, &mut pending_space);
                        out.push_str(&format!("\n\n![{alt}]\n\n"));
                    } else if let Some(src) = attr_value(&tag, "src").filter(|s| !s.is_empty()) {
                        out.push_str(&format!("\n\n![illustration]({src})\n\n"));
                    }
                    pending_space = false;
                }
                "hr" => {
                    out.push_str("\n\n---\n\n");
                    pending_space = false;
                }
                _ => {}
            }
            continue;
        }
        if in_tag {
            tag.push(c);
            continue;
        }
        match c {
            '&' => {
                // Keep entities as-is for now; decode a few common ones.
                pending_space = false;
                out.push('&');
            }
            '\n' | '\r' | '\t' => pending_space = true,
            ' ' => pending_space = true,
            _ => {
                flush_space(&mut out, &mut pending_space);
                out.push(c);
            }
        }
    }
    decode_basic_entities(&collapse_blank_lines(&out))
}

fn attr_value(tag_inner: &str, name: &str) -> Option<String> {
    let lower = tag_inner.to_ascii_lowercase();
    let key = format!("{name}=");
    let idx = lower.find(&key)?;
    let rest = &tag_inner[idx + key.len()..];
    let rest = rest.trim_start();
    let bytes = rest.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    if bytes[0] == b'"' || bytes[0] == b'\'' {
        let q = bytes[0] as char;
        let end = rest[1..].find(q)?;
        return Some(rest[1..1 + end].to_string());
    }
    let end = rest
        .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
        .unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

fn decode_basic_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .replace("&mdash;", "—")
        .replace("&ndash;", "–")
        .replace("&rsquo;", "'")
        .replace("&lsquo;", "'")
        .replace("&rdquo;", "\"")
        .replace("&ldquo;", "\"")
}

fn collapse_blank_lines(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut blank = 0usize;
    for line in s.lines() {
        if line.trim().is_empty() {
            blank += 1;
            if blank <= 2 {
                out.push('\n');
            }
        } else {
            blank = 0;
            out.push_str(line.trim_end());
            out.push('\n');
        }
    }
    out
}

fn markdown_to_safe_html(
    md: &str,
    pack_root: &Path,
    article_dir: Option<&Path>,
) -> Result<String, String> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    let parser = Parser::new_ext(md, options);
    let mut html_buf = String::new();
    html::push_html(&mut html_buf, parser);
    Ok(rewrite_local_images(
        &sanitize_html(&html_buf),
        pack_root,
        article_dir,
    ))
}

fn sanitize_html(input: &str) -> String {
    // Strip script/iframe/object and inline event handlers from generated or pack HTML.
    // IMPORTANT: copy text as UTF-8 chars — never `bytes[i] as char` (that turns curly
    // quotes into mojibake like â□□).
    let lower = input.to_ascii_lowercase();
    if !lower.contains('<') {
        return escape_text(input);
    }
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input.as_bytes()[i] == b'<' {
            let rest = &input[i..];
            let end = rest.find('>').map(|n| i + n).unwrap_or(input.len().saturating_sub(1));
            let end = end.min(input.len().saturating_sub(1));
            let tag = &input[i..=end];
            let tag_l = tag.to_ascii_lowercase();
            let blocked = tag_l.starts_with("<script")
                || tag_l.starts_with("</script")
                || tag_l.starts_with("<iframe")
                || tag_l.starts_with("</iframe")
                || tag_l.starts_with("<object")
                || tag_l.starts_with("<embed")
                || tag_l.starts_with("<link")
                || tag_l.starts_with("<meta")
                || tag_l.starts_with("<style")
                || tag_l.starts_with("</style");
            if !blocked {
                let cleaned = strip_event_handlers(tag);
                out.push_str(&cleaned);
            }
            i = end + 1;
        } else {
            let ch = input[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

fn strip_event_handlers(tag: &str) -> String {
    // Drop on*="..." / on*='...' attributes.
    let mut out = String::new();
    let mut chars = tag.chars().peekable();
    while let Some(c) = chars.next() {
        if c == ' ' || c == '\t' || c == '\n' {
            out.push(c);
            // Peek ahead for on...
            let mut look = String::new();
            while let Some(&n) = chars.peek() {
                if n.is_ascii_alphanumeric() || n == '_' || n == '-' {
                    look.push(n);
                    chars.next();
                } else {
                    break;
                }
            }
            if look.len() >= 3 && look.to_ascii_lowercase().starts_with("on") {
                // skip = and value
                while let Some(&n) = chars.peek() {
                    if n == '=' {
                        chars.next();
                        break;
                    }
                    if n.is_whitespace() {
                        chars.next();
                        continue;
                    }
                    break;
                }
                if let Some(&q) = chars.peek() {
                    if q == '"' || q == '\'' {
                        let quote = chars.next().unwrap();
                        for n in chars.by_ref() {
                            if n == quote {
                                break;
                            }
                        }
                    } else {
                        for n in chars.by_ref() {
                            if n.is_whitespace() || n == '>' {
                                if n == '>' {
                                    out.push('>');
                                }
                                break;
                            }
                        }
                    }
                }
                continue;
            }
            out.push_str(&look);
            continue;
        }
        out.push(c);
    }
    out
}

fn escape_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Per-image ceiling when falling back to data-URL inlining (tests / tiny packs).
const MAX_INLINE_IMAGE_BYTES: usize = 96 * 1024;

fn rewrite_local_images(html: &str, pack_root: &Path, article_dir: Option<&Path>) -> String {
    // Prefer `metis://…/guides/packs/…` so large HTML stays light and images still load
    // inside the HQ WebView (same outcome as html_view's local_file base URI).
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(idx) = rest.find("src=\"") {
        out.push_str(&rest[..idx]);
        out.push_str("src=\"");
        rest = &rest[idx + 5..];
        let Some(end) = rest.find('"') else {
            out.push_str(rest);
            return out;
        };
        let src = &rest[..end];
        rest = &rest[end + 1..];
        if src.starts_with("data:")
            || src.starts_with("metis://")
            || src.starts_with("http://metis.localhost/")
        {
            out.push_str(src);
            out.push('"');
            continue;
        }
        if src.starts_with("http://") || src.starts_with("https://") {
            // Block remote images in the offline reader.
            out.push('"');
            continue;
        }
        if let Some(path) = resolve_article_asset(pack_root, article_dir, src) {
            if let Some(url) = pack_asset_metis_url(pack_root, &path) {
                out.push_str(&url);
            } else if let Ok(data_url) = file_to_data_url(&path) {
                out.push_str(&data_url);
            }
        }
        out.push('"');
    }
    out.push_str(rest);
    out
}

fn pack_asset_metis_url(pack_root: &Path, path: &Path) -> Option<String> {
    let pack_id = pack_root.file_name()?.to_str()?;
    let rel = path.strip_prefix(pack_root).ok()?;
    let rel = rel.to_str()?.replace('\\', "/");
    if rel.is_empty() || rel.contains("..") {
        return None;
    }
    Some(crate::brand::metis_asset_url(&format!(
        "guides/packs/{pack_id}/{rel}"
    )))
}

/// Read a file under an installed pack for the `metis://guides/packs/…` protocol.
pub fn read_pack_asset(pack_id: &str, relative: &str) -> Option<(Vec<u8>, &'static str)> {
    let path = resolve_pack_path(&pack_dir(pack_id), relative).ok()?;
    if !path.is_file() {
        return None;
    }
    let bytes = fs::read(&path).ok()?;
    // Hard cap so a mistaken request cannot pull multi-GB junk into the WebView.
    if bytes.len() > 8 * 1024 * 1024 {
        return None;
    }
    let mime = match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "md" | "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    };
    Some((bytes, mime))
}

fn resolve_article_asset(
    pack_root: &Path,
    article_dir: Option<&Path>,
    src: &str,
) -> Option<PathBuf> {
    let rel = src.trim_start_matches(['/', '\\']);
    if rel.is_empty() || rel.contains("..") {
        return None;
    }
    if let Some(dir) = article_dir {
        let candidate = dir.join(rel);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    resolve_pack_path(pack_root, rel).ok().filter(|p| p.is_file())
}

fn file_to_data_url(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_INLINE_IMAGE_BYTES {
        return Err("Image too large to inline.".into());
    }
    let mime = match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    };
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok(format!("data:{mime};base64,{b64}"))
}

// ---------- Export ----------

pub fn export_article_md(pack_id: &str, article_id: &str) -> Result<PathBuf, String> {
    let loaded = load_article(pack_id, article_id)?;
    let suggested = format!("{}.md", sanitize_id(&loaded.article.id));
    let dest = rfd::FileDialog::new()
        .set_title("Export article as Markdown")
        .set_file_name(&suggested)
        .add_filter("Markdown", &["md"])
        .save_file()
        .ok_or_else(|| "Export cancelled.".to_string())?;
    fs::write(&dest, &loaded.body_markdown).map_err(|e| e.to_string())?;
    Ok(dest)
}

pub fn export_article_pdf(pack_id: &str, article_id: &str) -> Result<PathBuf, String> {
    let loaded = load_article(pack_id, article_id)?;
    let suggested = format!("{}.pdf", sanitize_id(&loaded.article.id));
    let dest = rfd::FileDialog::new()
        .set_title("Export article as PDF")
        .set_file_name(&suggested)
        .add_filter("PDF", &["pdf"])
        .save_file()
        .ok_or_else(|| "Export cancelled.".to_string())?;
    write_simple_pdf(
        &dest,
        &loaded.article.title,
        &loaded.pack_name,
        &loaded.body_markdown,
    )?;
    Ok(dest)
}

/// Minimal multi-page text PDF (Helvetica, WinAnsi-ish ASCII fallback).
fn write_simple_pdf(path: &Path, title: &str, pack: &str, body: &str) -> Result<(), String> {
    let lines = wrap_pdf_lines(&format!("{title}\n{pack}\n\n{body}"), 88);
    let mut pages: Vec<Vec<String>> = Vec::new();
    for chunk in lines.chunks(52) {
        pages.push(chunk.to_vec());
    }
    if pages.is_empty() {
        pages.push(vec![String::new()]);
    }

    let mut content_objs: Vec<String> = Vec::new();
    for page_lines in &pages {
        let mut stream = String::from("BT /F1 11 Tf 50 780 Td 14 TL\n");
        for (i, line) in page_lines.iter().enumerate() {
            if i > 0 {
                stream.push_str("T*\n");
            }
            stream.push_str(&format!("({}) Tj\n", pdf_escape(line)));
        }
        stream.push_str("ET");
        content_objs.push(stream);
    }

    let mut pdf = String::from("%PDF-1.4\n");
    let mut offsets: Vec<usize> = Vec::new();
    let obj = |pdf: &mut String, offsets: &mut Vec<usize>, body: &str| {
        offsets.push(pdf.len());
        let n = offsets.len();
        pdf.push_str(&format!("{n} 0 obj\n{body}\nendobj\n"));
    };

    obj(
        &mut pdf,
        &mut offsets,
        "<< /Type /Catalog /Pages 2 0 R >>",
    );

    let kids: String = (0..pages.len())
        .map(|i| format!("{} 0 R", 3 + i * 2))
        .collect::<Vec<_>>()
        .join(" ");
    obj(
        &mut pdf,
        &mut offsets,
        &format!(
            "<< /Type /Pages /Kids [{kids}] /Count {} >>",
            pages.len()
        ),
    );

    let font_obj_num = 3 + pages.len() * 2;
    for (i, stream) in content_objs.iter().enumerate() {
        let page_num = 3 + i * 2;
        let content_num = page_num + 1;
        obj(
            &mut pdf,
            &mut offsets,
            &format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
                 /Contents {content_num} 0 R /Resources << /Font << /F1 {font_obj_num} 0 R >> >> >>"
            ),
        );
        obj(
            &mut pdf,
            &mut offsets,
            &format!(
                "<< /Length {} >>\nstream\n{}\nendstream",
                stream.len(),
                stream
            ),
        );
    }
    obj(
        &mut pdf,
        &mut offsets,
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    );

    let xref_at = pdf.len();
    pdf.push_str(&format!("xref\n0 {}\n", offsets.len() + 1));
    pdf.push_str("0000000000 65535 f \n");
    for off in &offsets {
        pdf.push_str(&format!("{off:010} 00000 n \n"));
    }
    pdf.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
        offsets.len() + 1
    ));
    fs::write(path, pdf).map_err(|e| e.to_string())
}

fn pdf_escape(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\\' => "\\\\".to_string(),
            '(' => "\\(".to_string(),
            ')' => "\\)".to_string(),
            c if c.is_ascii() && !c.is_control() => c.to_string(),
            _ => "?".to_string(),
        })
        .collect()
}

fn wrap_pdf_lines(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for para in text.split('\n') {
        if para.is_empty() {
            lines.push(String::new());
            continue;
        }
        let mut cur = String::new();
        for word in para.split_whitespace() {
            if cur.is_empty() {
                cur.push_str(word);
            } else if cur.len() + 1 + word.len() <= width {
                cur.push(' ');
                cur.push_str(word);
            } else {
                lines.push(cur);
                cur = word.to_string();
            }
        }
        if !cur.is_empty() {
            lines.push(cur);
        }
    }
    lines
}

pub fn open_attachment(pack_id: &str, relative: &str) -> Result<(), String> {
    let path = resolve_pack_path(&pack_dir(pack_id), relative)?;
    open_path(&path)
}

fn open_path(path: &Path) -> Result<(), String> {
    let path_s = path.to_string_lossy().to_string();
    #[cfg(target_os = "windows")]
    {
        let mut cmd = std::process::Command::new("cmd");
        cmd.args(["/C", "start", "", &path_s]);
        return if cmd.spawn().is_ok() {
            Ok(())
        } else {
            Err("Could not open file.".into())
        };
    }
    #[cfg(target_os = "macos")]
    {
        let mut cmd = std::process::Command::new("open");
        cmd.arg(&path_s);
        return if cmd.spawn().is_ok() {
            Ok(())
        } else {
            Err("Could not open file.".into())
        };
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let mut xdg = std::process::Command::new("xdg-open");
        xdg.arg(&path_s);
        if xdg.spawn().is_ok() {
            return Ok(());
        }
        let mut gio = std::process::Command::new("gio");
        gio.args(["open", &path_s]);
        if gio.spawn().is_ok() {
            return Ok(());
        }
        Err("Could not open file (tried xdg-open, gio).".into())
    }
}

pub fn spawn_install(pack_id: String) {
    std::thread::spawn(move || {
        if let Err(err) = install_pack(&pack_id) {
            hq_log!(warn, "guides install {pack_id}: {err}");
        }
    });
}

pub fn spawn_sync_catalog() {
    std::thread::spawn(|| {
        if let Err(err) = sync_catalog() {
            hq_log!(warn, "guides catalog sync: {err}");
            set_job(job_fail("", err));
        }
    });
}

/// Cap highlighted marks so huge Gutenberg texts stay scrollable in the WebView.
const FIND_MAX_MARKS: usize = 120;
/// Ignore 1-char queries (too many hits, useless).
pub const FIND_MIN_NEEDLE_CHARS: usize = 2;

/// Highlight case-insensitive matches in already-rendered article HTML (no markdown re-parse).
/// Empty / too-short needle returns the input HTML unchanged with 0 hits.
pub fn highlight_article_html(html: &str, needle: &str) -> (String, usize) {
    let needle = normalize_find_needle(needle);
    if needle.chars().count() < FIND_MIN_NEEDLE_CHARS {
        return (html.to_string(), 0);
    }
    highlight_in_html_text(html, &needle, 0)
}

fn normalize_find_needle(raw: &str) -> String {
    raw.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

/// Highlight matches only outside HTML tags. `current` gets `guides-find-current`.
/// Marks are capped at [`FIND_MAX_MARKS`]; returned hit count is the marked count.
fn highlight_in_html_text(html: &str, needle_lower: &str, current: usize) -> (String, usize) {
    if needle_lower.is_empty() {
        return (html.to_string(), 0);
    }
    let needle_bytes = needle_lower.as_bytes();
    let nlen = needle_bytes.len();
    if nlen == 0 {
        return (html.to_string(), 0);
    }
    let bytes = html.as_bytes();
    let mut out = String::with_capacity(html.len().saturating_add(html.len() / 8));
    let mut hits = 0usize;
    let mut in_tag = false;
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'<' {
            in_tag = true;
            out.push('<');
            i += 1;
            continue;
        }
        if b == b'>' && in_tag {
            in_tag = false;
            out.push('>');
            i += 1;
            continue;
        }
        if in_tag {
            let ch = html[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }
        if hits >= FIND_MAX_MARKS {
            // Cap marks so scrolling stays usable; append the rest unchanged.
            out.push_str(&html[i..]);
            break;
        }
        if i + nlen <= bytes.len() {
            let window = &bytes[i..i + nlen];
            let matches = window
                .iter()
                .zip(needle_bytes.iter())
                .all(|(a, n)| a.to_ascii_lowercase() == *n);
            if matches && html.is_char_boundary(i) && html.is_char_boundary(i + nlen) {
                if hits == current {
                    out.push_str(
                        "<mark id=\"gfind-current\" class=\"guides-find-hit guides-find-current\">",
                    );
                } else {
                    out.push_str("<mark class=\"guides-find-hit\">");
                }
                out.push_str(&html[i..i + nlen]);
                out.push_str("</mark>");
                hits += 1;
                i += nlen;
                continue;
            }
        }
        let ch = html[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    (out, hits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn with_temp_data<F: FnOnce()>(f: F) {
        let _guard = TEST_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!(
            "metis-guides-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        // Override via env used by data_dir when set.
        std::env::set_var("METIS_DATA_DIR", &dir);
        // Clear leftover job state.
        set_job(JobStatus::default());
        f();
        let _ = fs::remove_dir_all(&dir);
        std::env::remove_var("METIS_DATA_DIR");
    }

    #[test]
    fn bundled_install_search_remove() {
        with_temp_data(|| {
            let catalog = load_catalog();
            assert!(catalog.packs.iter().any(|p| p.id == "hq-basics"));
            install_pack("hq-basics").expect("install");
            let hits = search("water", None, None).expect("search");
            assert!(
                hits.iter().any(|h| h.article_id == "water"),
                "expected water hit, got {hits:?}"
            );
            let empty = search("", Some("hq-basics"), None).expect("list");
            assert!(
                empty.len() >= 6,
                "expected expanded hq-basics articles, got {}",
                empty.len()
            );
            let by_cat = search("", None, Some("water")).expect("category");
            assert!(
                by_cat.iter().any(|h| h.article_id == "water"),
                "expected water under category filter, got {by_cat:?}"
            );
            let article = load_article("hq-basics", "water").expect("load");
            assert!(article.body_html.contains("drinking") || article.body_markdown.contains("water"));
            remove_pack("hq-basics").expect("remove");
            let after = search("water", None, None).expect("search after");
            assert!(after.iter().all(|h| h.pack_id != "hq-basics"));
        });
    }

    #[test]
    fn fts_query_sanitizes() {
        let q = fts_query_from_user("first aid!");
        assert!(q.contains("first"));
        assert!(q.contains("aid"));
    }

    #[test]
    fn sanitize_strips_script() {
        let dirty = "<p>Hi</p><script>alert(1)</script><img src=\"x\" onerror=\"evil()\">";
        let clean = sanitize_html(dirty);
        assert!(!clean.to_ascii_lowercase().contains("<script"));
        assert!(!clean.to_ascii_lowercase().contains("onerror"));
    }

    #[test]
    fn remote_files_synthesize_and_search() {
        with_temp_data(|| {
            let entry = CatalogPack {
                id: "test-remote".into(),
                name: "Test Remote Pack".into(),
                version: "1.0.0".into(),
                source: "Test".into(),
                license: "Public domain".into(),
                description: "Fixture pack for remote-file install.".into(),
                format: "hq-native".into(),
                download_url: None,
                sha256: None,
                size_bytes: None,
                files: vec![CatalogRemoteFile {
                    id: "note".into(),
                    title: "Boil water note".into(),
                    url: "https://example.invalid/note.txt".into(),
                    sha256: "deadbeef".into(),
                    size_bytes: None,
                    path: "articles/note.txt".into(),
                }],
                bundled: None,
                tags: vec!["water".into(), "test".into()],
                categories: vec!["water".into()],
                installed_version: None,
                status: "available".into(),
            };
            assert!(entry.is_installable());

            let dest = pack_dir(&entry.id);
            fs::create_dir_all(dest.join("articles")).unwrap();
            fs::write(
                dest.join("articles/note.txt"),
                "Boil clear water for one minute before drinking.\n",
            )
            .unwrap();
            write_synthesized_manifest(
                &entry,
                &dest,
                vec![ArticleMeta {
                    id: "note".into(),
                    title: "Boil water note".into(),
                    path: "articles/note.txt".into(),
                    tags: entry.tags.clone(),
                    summary: entry.description.clone(),
                }],
                vec![],
            )
            .expect("manifest");
            let manifest = load_manifest_file(&dest.join("manifest.json")).unwrap();
            index_pack(&manifest, &dest).expect("index");

            let mut catalog = load_catalog();
            catalog.packs.push(entry.clone());
            save_catalog(&catalog).unwrap();
            if let Some(row) = catalog.packs.iter_mut().find(|p| p.id == entry.id) {
                row.installed_version = Some(entry.version.clone());
                row.status = "installed".into();
            }
            save_catalog(&catalog).unwrap();

            let listed = search("", Some("test-remote"), None).expect("list");
            assert!(
                listed.iter().any(|h| h.article_id == "note"),
                "expected note in browse list, got {listed:?}"
            );
            assert!(
                listed.iter().all(|h| h.article_id != "about"),
                "about should be hidden when pack has real articles, got {listed:?}"
            );
            let about = manifest.articles.iter().find(|a| a.id == "about");
            assert_eq!(
                about.map(|a| a.title.as_str()),
                Some("About"),
                "multi-article packs label about as About"
            );

            let hits = search("boil", Some("test-remote"), None).expect("search");
            assert!(
                hits.iter().any(|h| h.article_id == "note"),
                "expected boil hit on note, got {hits:?}"
            );
            let by_cat = search("", None, Some("water")).expect("cat");
            assert!(
                by_cat.iter().any(|h| h.pack_id == "test-remote"),
                "expected test-remote under water category, got {by_cat:?}"
            );
            assert!(
                hits.iter().all(|h| h.article_id != "about"),
                "about should stay out of search when siblings exist, got {hits:?}"
            );
            let about = load_article("test-remote", "about").expect("about");
            assert!(about.body_markdown.contains("Test Remote Pack"));
            remove_pack("test-remote").expect("remove");
        });
    }

    #[test]
    fn merge_bundled_seed_adds_missing_packs() {
        let seed = parse_seed();
        let mut local = GuidesCatalog {
            version: 1,
            updated_at: 0,
            remote_url: seed.remote_url.clone(),
            packs: seed
                .packs
                .iter()
                .filter(|p| p.id == "hq-basics" || p.id == "fema-are-you-ready")
                .cloned()
                .collect(),
        };
        assert!(merge_bundled_seed(&mut local));
        assert_eq!(local.packs.len(), seed.packs.len());
        assert!(local.packs.iter().any(|p| p.id == "us-fm-survival"));
        assert!(!local.packs.iter().any(|p| p.id == "pd-shelters-shacks"));
        // Second pass is a no-op.
        assert!(!merge_bundled_seed(&mut local));
    }

    #[test]
    fn merge_bundled_seed_retires_dropped_packs() {
        let mut local = GuidesCatalog {
            version: 1,
            updated_at: 0,
            remote_url: String::new(),
            packs: vec![CatalogPack {
                id: "pd-shelters-shacks".into(),
                name: "gone".into(),
                version: "1".into(),
                source: String::new(),
                license: String::new(),
                description: String::new(),
                format: "hq-native".into(),
                download_url: None,
                sha256: None,
                size_bytes: None,
                files: vec![],
                bundled: None,
                tags: vec![],
                categories: vec!["shelter".into()],
                installed_version: None,
                status: "available".into(),
            }],
        };
        assert!(merge_bundled_seed(&mut local));
        assert!(!local.packs.iter().any(|p| p.id == "pd-shelters-shacks"));
    }

    #[test]
    fn catalog_seed_lists_remote_packs() {
        let seed: GuidesCatalog = serde_json::from_str(SEED_CATALOG).unwrap();
        assert!(seed.packs.iter().any(|p| p.id == "fema-are-you-ready"));
        assert!(seed.packs.iter().any(|p| p.id == "ready-water-outage"));
        assert!(seed.packs.iter().any(|p| p.id == "ready-hazard-sheets"));
        assert!(seed.packs.iter().any(|p| p.id == "pd-woodcraft-camping"));
        assert!(seed.packs.iter().any(|p| p.id == "pd-knots-ropework"));
        for id in [
            "us-fm-survival",
            "us-fm-first-aid",
            "us-fm-land-nav",
            "us-fm-hygiene",
            "usda-home-canning",
            "pd-nuclear-war-survival",
        ] {
            assert!(
                seed.packs.iter().any(|p| p.id == id),
                "missing first-wave manual pack `{id}`"
            );
        }
        assert!(!seed.packs.iter().any(|p| p.id == "pd-shelters-shacks"));
        assert!(!seed.packs.iter().any(|p| p.id == "offline-survival-stub"));
        assert!(
            seed.packs.iter().all(|p| !p.categories.is_empty()),
            "every seed pack should declare categories"
        );
        let fema = seed
            .packs
            .iter()
            .find(|p| p.id == "fema-are-you-ready")
            .unwrap();
        assert!(fema.is_installable());
        assert!(!fema.files.is_empty());
        for pack in &seed.packs {
            for cat in &pack.categories {
                assert!(
                    is_known_guide_category(cat),
                    "pack {} has unknown category `{cat}`",
                    pack.id
                );
            }
            for file in &pack.files {
                assert_eq!(
                    file.sha256.len(),
                    64,
                    "pack {} file {} sha256 must be 64 hex chars",
                    pack.id,
                    file.id
                );
                assert!(
                    file.sha256.chars().all(|c| c.is_ascii_hexdigit()),
                    "pack {} file {} sha256 must be hex",
                    pack.id,
                    file.id
                );
                assert!(
                    file.url.starts_with("https://"),
                    "pack {} file {} must use https URL",
                    pack.id,
                    file.id
                );
                let lower = file.url.to_ascii_lowercase();
                assert!(
                    !lower.contains("trueprepper.com")
                        && !lower.contains("ardbark.com")
                        && !lower.contains("collapsesurvivalsite.com")
                        && !lower.contains("wp-content"),
                    "pack {} file {} must not use aggregator/wp-content mirrors",
                    pack.id,
                    file.id
                );
            }
        }
    }

    #[test]
    fn guide_categories_normalize() {
        let raw = vec![
            "water".into(),
            "nope".into(),
            "medical".into(),
            "water".into(),
        ];
        assert_eq!(
            normalize_guide_categories(&raw),
            vec!["water".to_string(), "medical".to_string()]
        );
        assert_eq!(guide_category_label("knots"), Some("Knots"));
        assert!(!is_known_guide_category("gas-mask"));
    }

    #[test]
    fn find_highlights_text_not_tags() {
        let html = "<p>Boil water before drinking water.</p>";
        let (out, hits) = highlight_in_html_text(html, "water", 0);
        assert_eq!(hits, 2);
        assert!(out.contains("guides-find-current"));
        assert!(out.contains("<mark"));
        assert!(out.starts_with("<p>"));
        let (none, zero) = highlight_article_html(html, "");
        assert_eq!(zero, 0);
        assert_eq!(none, html);
        let (short, zero2) = highlight_article_html(html, "w");
        assert_eq!(zero2, 0);
        assert_eq!(short, html);
    }

    #[test]
    fn user_agent_omits_metis() {
        assert!(
            !USER_AGENT.to_ascii_lowercase().contains("metis"),
            "guides UA must not contain Metis (CDN 403)"
        );
    }

    #[test]
    fn plain_text_becomes_paragraphs() {
        let raw = "TITLE LINE\n\nFirst paragraph\nstill first.\n\nSecond paragraph.\n";
        let md = plain_text_to_markdown(raw);
        assert!(md.contains("## TITLE LINE"));
        assert!(md.contains("First paragraph still first."));
        assert!(md.contains("Second paragraph."));
        let html = markdown_to_safe_html(&md, Path::new("."), None).unwrap();
        assert!(html.contains("<h2>"));
        assert!(html.contains("<p>"));
    }

    #[test]
    fn rewrite_anchors_marks_jumps_and_pgmarks() {
        let raw = concat!(
            r##"<a id="Page_12">[Pg 12]</a>"##,
            r##" text "##,
            r##"<a href="#Page_12" class="pginternal">12</a>"##,
            r##" and "##,
            r##"<a href="#Chapter_II">Chapter II</a>"##,
        );
        let out = rewrite_guide_anchors(raw);
        assert!(
            out.contains("id=\"Page_12\"")
                && out.contains("data-guides-jump=\"Page_12\"")
                && out.contains("guides-pgmark"),
            "pgmark must keep id and be a jump control, got:\n{out}"
        );
        assert!(out.contains("button type=\"button\" class=\"guides-jump\""), "got:\n{out}");
        assert!(out.contains("data-guides-jump=\"Chapter_II\""), "got:\n{out}");
        assert!(!out.contains("<a id=\"Page_12\">"), "marker should not stay an <a>");
        assert!(!out.contains("href=\"#Page_12\""), "jumps must not keep href (opens OS browser)");
    }

    #[test]
    fn strip_pg_chrome_survives_utf8_emdash() {
        // Regression: byte-walking with str slices panicked inside '—' during index.
        let raw = concat!(
            r#"<div id="pg-header"><div><p>Public domain — Project Gutenberg</p></div></div>"#,
            r#"<h1>Woodcraft</h1><p>Body — still here.</p>"#,
            r#"<div id="pg-footer"><div><p>End — footer</p></div></div>"#,
        );
        let out = strip_pg_chrome(raw);
        assert!(!out.contains("pg-header"));
        assert!(!out.contains("Project Gutenberg"));
        assert!(out.contains("Body — still here."), "got:\n{out}");
        assert!(!out.contains("pg-footer"));
    }

    #[test]
    fn html_article_keeps_images_and_exports_markdown() {
        let raw = r#"<!DOCTYPE html><html><body>
<div id="pg-header"><p>Gutenberg chrome</p></div>
<h1>Woodcraft</h1>
<p>Hello camp.</p>
<img src="images/illp19.png" alt="Indian Camp">
<div id="pg-footer"><p>Footer</p></div>
</body></html>"#;
        let dir = std::env::temp_dir().join(format!(
            "metis-html-article-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("images")).unwrap();
        // 1x1 PNG
        let png = base64::engine::general_purpose::STANDARD
            .decode(
                "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==",
            )
            .unwrap();
        fs::write(dir.join("images/illp19.png"), png).unwrap();
        let html = html_article_to_safe_html(raw, &dir, Some(&dir)).unwrap();
        let pack_name = dir.file_name().and_then(|s| s.to_str()).unwrap();
        assert!(
            html.contains(&format!("guides/packs/{pack_name}/images/illp19.png"))
                || html.contains("data:image/png;base64,"),
            "expected metis:// pack image or tiny data URL, got:\n{html}"
        );
        assert!(!html.to_ascii_lowercase().contains("gutenberg chrome"));
        assert!(html.contains("<h1>"));
        let md = html_to_export_markdown(raw);
        assert!(md.contains("# Woodcraft"), "got:\n{md}");
        assert!(md.contains("Hello camp"));
        assert!(md.contains("Indian Camp"), "got:\n{md}");
        let _ = fs::remove_dir_all(&dir);
    }


    #[test]
    fn woodcraft_html_stays_under_webview_budget() {
        // Optional soak against the operator-installed Gutenberg pack (not in CI).
        let article = PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
            .join(".MetisMeshHQ/guides/packs/pd-woodcraft-camping/articles/woodcraft.html");
        if !article.is_file() {
            eprintln!("skip: woodcraft pack not installed at {}", article.display());
            return;
        }
        let raw = fs::read_to_string(&article).expect("read woodcraft");
        let article_dir = article.parent().unwrap();
        let pack_root = article_dir.parent().unwrap();
        let t0 = std::time::Instant::now();
        let html = html_article_to_safe_html(&raw, pack_root, Some(article_dir)).expect("safe html");
        let ms = t0.elapsed().as_millis();
        eprintln!(
            "woodcraft safe-html {} ms · {} bytes (raw {})",
            ms,
            html.len(),
            raw.len()
        );
        assert!(ms < 5_000, "html_article_to_safe_html took {ms} ms");
        assert!(html.len() > 10_000);
        assert!(
            html.len() < 400_000,
            "HTML too large for WebView: {}",
            html.len()
        );
        assert!(
            html.contains("guides/packs/pd-woodcraft-camping/"),
            "expected metis:// pack image URLs, got snippet:\n{}",
            &html[..html.len().min(400)]
        );
        assert!(
            html.contains("button type=\"button\" class=\"guides-jump\""),
            "TOC jumps must be buttons, not <a href>"
        );
        assert!(
            !html.contains("href=\"#Page_"),
            "hash hrefs must be rewritten away so wry cannot open the OS browser"
        );
    }

    #[test]
    fn sanitize_preserves_utf8_quotes() {
        let html = "<p>provided to you \u{2018}AS-IS\u{2019}, WITH NO</p>";
        let out = sanitize_html(html);
        assert!(out.contains('\u{2018}') || out.contains('\''));
        assert!(!out.contains('â'), "must not Latin-1-decode UTF-8 bytes: {out}");
        // After plaintext normalize, curly quotes become ASCII in .txt path:
        let md = plain_text_to_markdown("provided to you \u{2018}AS-IS\u{2019}, WITH NO");
        assert!(md.contains("'AS-IS'"));
        assert!(!md.contains('\u{2018}'));
    }

    #[test]
    fn index_block_becomes_table() {
        let raw = "paddles 93\n      \"Sairy Gamp\" 91\n      seats 93\n\nRecipes\n      baked beans 69\n      bread 63, 64\n      coffee 64-66\n";
        let md = plain_text_to_markdown(raw);
        assert!(
            md.contains("| Topic | Pages |"),
            "expected table header, got:\n{md}"
        );
        assert!(md.contains("| paddles | 93 |"), "got:\n{md}");
        assert!(md.contains("| baked beans | 69 |"), "got:\n{md}");
        let html = markdown_to_safe_html(&md, Path::new("."), None).unwrap();
        assert!(html.contains("<table>"), "got:\n{html}");
    }

    #[test]
    fn gutenberg_mirror_candidates() {
        let c = download_url_candidates(
            "https://www.gutenberg.org/cache/epub/34607/pg34607-images.html",
        );
        assert_eq!(c[0], "https://www.gutenberg.org/cache/epub/34607/pg34607-images.html");
        assert!(
            c.iter().any(|u| u.contains("gutenberg.pglaf.org")),
            "expected pglaf mirror, got {c:?}"
        );
    }
}
