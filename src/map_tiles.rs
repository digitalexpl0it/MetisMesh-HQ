//! Multi-region offline PMTiles downloads (Protomaps extracts).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::Datelike;
use serde::{Deserialize, Serialize};

use crate::db::Db;
use crate::pmtiles_path;

const REGIONS_KEY: &str = "map_regions";
const ACTIVE_KEY: &str = "map_active_region";
const MAX_ZOOM: u8 = 12;
/// Avoid z0–zN world tiles in extracts: a single low zoom tile spans far past
/// the placed square and makes Streets look “shifted” when zoomed out.
const MIN_ZOOM_FLOOR: u8 = 4;
const MIN_ZOOM_CEIL: u8 = 9;

/// Pick extract/source minzoom so tiles are about the pack size (or smaller).
pub fn min_zoom_for_bbox(bbox: [f64; 4]) -> u8 {
    let [w, s, e, n] = bbox;
    let span = (e - w).abs().max((n - s).abs()).max(0.25);
    // Tile width at zoom z ≈ 360 / 2^z degrees. Want ~2 tiles across the pack.
    let z = (360.0 / (span / 2.0)).log2().floor() as i32;
    z.clamp(MIN_ZOOM_FLOOR as i32, MIN_ZOOM_CEIL as i32) as u8
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TileRegion {
    pub id: String,
    pub name: String,
    /// west, south, east, north — tile extract always uses this axis-aligned box
    pub bbox: [f64; 4],
    pub path: String,
    /// Placement shape used for the on-map outline (`rectangle` / `circle` / `triangle`).
    #[serde(default = "default_region_shape")]
    pub shape: String,
    #[serde(default)]
    pub center_lng: f64,
    #[serde(default)]
    pub center_lat: f64,
    #[serde(default)]
    pub radius_miles: f64,
    #[serde(default)]
    pub source_build: String,
    #[serde(default)]
    pub updated_at: i64,
    #[serde(default)]
    pub bytes: u64,
    #[serde(default)]
    pub status: String,
}

fn default_region_shape() -> String {
    "rectangle".into()
}

/// Active pack display (axis-aligned square tile extract).
#[derive(Debug, Clone)]
pub struct ActiveCoverage {
    pub bbox: [f64; 4],
}

#[derive(Debug, Clone, Default)]
pub struct DownloadStatus {
    pub region_id: String,
    pub phase: String,
    pub detail: String,
    pub running: bool,
}

static DOWNLOAD: OnceLock<Mutex<DownloadStatus>> = OnceLock::new();

fn download_lock() -> &'static Mutex<DownloadStatus> {
    DOWNLOAD.get_or_init(|| Mutex::new(DownloadStatus::default()))
}

pub fn download_status() -> DownloadStatus {
    download_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

fn set_download(status: DownloadStatus) {
    if let Ok(mut g) = download_lock().lock() {
        *g = status;
    }
}

pub fn tiles_dir() -> PathBuf {
    let root = if let Some(path) = std::env::var_os("METIS_DATA_DIR") {
        PathBuf::from(path)
    } else {
        PathBuf::from("data")
    };
    let dir = root.join("map-tiles");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn bin_dir() -> PathBuf {
    let dir = tiles_dir().join("bin");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn region_file_path(id: &str) -> PathBuf {
    tiles_dir().join(format!("{id}.pmtiles"))
}

/// Resolve a region's PMTiles file for the `metis://` protocol (no DB required).
pub fn region_pmtiles_path(id: &str) -> Option<PathBuf> {
    let path = region_file_path(id);
    path.is_file().then_some(path)
}

fn refresh_region_file_meta(region: &mut TileRegion) {
    let path = if region.path.is_empty() {
        region_file_path(&region.id)
    } else {
        PathBuf::from(&region.path)
    };
    if path.is_file() {
        region.path = path.display().to_string();
        region.bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if region.status != "downloading" && region.status != "error" {
            region.status = "ready".into();
        }
    } else if region.status != "downloading" {
        region.status = "missing".into();
        region.bytes = 0;
    }
}

fn preset_ids() -> Vec<&'static str> {
    vec!["socal", "norcal", "pnw"]
}

pub fn list_regions(db: &Db) -> Vec<TileRegion> {
    let mut regions: Vec<TileRegion> = db
        .setting(REGIONS_KEY)
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    for region in &mut regions {
        refresh_region_file_meta(region);
    }
    // Drop unused legacy presets — downloads now happen via Map ZIP + radius.
    let before = regions.len();
    regions.retain(|r| {
        let is_preset = preset_ids().iter().any(|id| *id == r.id);
        if !is_preset {
            return true;
        }
        Path::new(&r.path).is_file() || r.bytes > 0 || r.status == "ready" || r.status == "downloading"
    });
    if regions.len() != before {
        let _ = save_regions(db, &regions);
    } else {
        let _ = save_regions(db, &regions);
    }
    regions
}

/// Packs that are installed or in-progress (for Settings management UI).
pub fn list_installed_regions(db: &Db) -> Vec<TileRegion> {
    list_regions(db)
        .into_iter()
        .filter(|r| {
            Path::new(&r.path).is_file()
                || r.bytes > 0
                || r.status == "ready"
                || r.status == "downloading"
                || r.status == "error"
        })
        .collect()
}

pub fn save_regions(db: &Db, regions: &[TileRegion]) -> Result<(), String> {
    let raw = serde_json::to_string(regions).map_err(|e| e.to_string())?;
    db.set_setting(REGIONS_KEY, &raw).map_err(|e| e.to_string())
}

pub fn active_region_id(db: &Db) -> Option<String> {
    db.setting(ACTIVE_KEY).ok().flatten().filter(|s| !s.is_empty())
}

/// Active pack bbox `[west, south, east, north]` when a ready region is selected.
pub fn active_coverage_bbox(db: &Db) -> Option<[f64; 4]> {
    active_coverage(db).map(|c| c.bbox)
}

pub fn active_coverage(db: &Db) -> Option<ActiveCoverage> {
    let id = active_region_id(db)?;
    let region = list_regions(db).into_iter().find(|r| r.id == id)?;
    let [w, s, e, n] = region.bbox;
    if !(Path::new(&region.path).is_file() && w < e && s < n) {
        return None;
    }
    Some(ActiveCoverage {
        bbox: region.bbox,
    })
}

pub fn set_active_region(db: &Db, id: &str) -> Result<(), String> {
    db.set_setting(ACTIVE_KEY, id).map_err(|e| e.to_string())?;
    sync_active_path(db);
    Ok(())
}

pub fn sync_active_path(db: &Db) {
    let regions = list_regions(db);
    let active = active_region_id(db);
    let path = active
        .as_ref()
        .and_then(|id| regions.iter().find(|r| &r.id == id))
        .filter(|r| Path::new(&r.path).is_file())
        .map(|r| PathBuf::from(&r.path))
        .or_else(|| {
            // Legacy single-path setting still wins if no active region file.
            db.setting("pmtiles_path")
                .ok()
                .flatten()
                .filter(|p| !p.trim().is_empty() && Path::new(p).is_file())
                .map(PathBuf::from)
        });
    pmtiles_path::set_pmtiles_path(path.as_ref());
    if let Some(id) = active {
        pmtiles_path::set_active_region_id(Some(id));
    } else {
        pmtiles_path::set_active_region_id(None);
    }
}

pub fn upsert_region(db: &Db, mut region: TileRegion) -> Result<(), String> {
    if region.id.trim().is_empty() {
        return Err("Region id is required.".into());
    }
    if region.name.trim().is_empty() {
        return Err("Region name is required.".into());
    }
    let [w, s, e, n] = region.bbox;
    if !(w < e && s < n) {
        return Err("Invalid bbox (need west < east and south < north).".into());
    }
    region.id = region.id.trim().to_ascii_lowercase().replace(' ', "-");
    if region.path.is_empty() {
        region.path = region_file_path(&region.id).display().to_string();
    }
    refresh_region_file_meta(&mut region);
    let mut regions = list_regions(db);
    if let Some(existing) = regions.iter_mut().find(|r| r.id == region.id) {
        *existing = region;
    } else {
        regions.push(region);
    }
    save_regions(db, &regions)
}

pub fn delete_region(db: &Db, id: &str) -> Result<(), String> {
    let mut regions = list_regions(db);
    let Some(idx) = regions.iter().position(|r| r.id == id) else {
        return Err("Region not found.".into());
    };
    let removed = regions.remove(idx);
    if Path::new(&removed.path).is_file() {
        let _ = std::fs::remove_file(&removed.path);
    }
    save_regions(db, &regions)?;
    if active_region_id(db).as_deref() == Some(id) {
        let _ = db.set_setting(ACTIVE_KEY, "");
    }
    sync_active_path(db);
    Ok(())
}

fn region_from_center(id: &str, name: &str, lat: f64, lon: f64, radius_miles: f64) -> TileRegion {
    let bbox = bbox_from_center_radius(lat, lon, radius_miles);
    TileRegion {
        id: id.into(),
        name: name.into(),
        bbox,
        path: region_file_path(id).display().to_string(),
        shape: "rectangle".into(),
        center_lng: lon,
        center_lat: lat,
        radius_miles,
        source_build: String::new(),
        updated_at: 0,
        bytes: 0,
        status: "missing".into(),
    }
}

pub fn bbox_from_center_radius(lat: f64, lon: f64, radius_miles: f64) -> [f64; 4] {
    let radius = radius_miles.max(5.0).min(300.0);
    let d_lat = radius / 69.0;
    let cos_lat = lat.to_radians().cos().abs().max(0.2);
    let d_lon = radius / (69.0 * cos_lat);
    [lon - d_lon, lat - d_lat, lon + d_lon, lat + d_lat]
}

fn percent_encode_query(value: &str) -> String {
    let mut out = String::with_capacity(value.len() * 3);
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn looks_like_us_zip(query: &str) -> bool {
    let digits: String = query.chars().filter(|c| c.is_ascii_digit()).collect();
    let compact: String = query
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    digits.len() == 5
        && compact.len() == 5
        && query
            .chars()
            .all(|c| c.is_ascii_digit() || c.is_ascii_whitespace() || c == '-')
}

fn nominatim_first(url: &str, not_found: &str) -> Result<(f64, f64, String), String> {
    let body = ureq::get(url)
        .set(
            "User-Agent",
            "MetisMeshHQ/0.1 (offline map download; local desktop)",
        )
        .timeout(std::time::Duration::from_secs(20))
        .call()
        .map_err(|e| format!("Place lookup failed: {e}"))?
        .into_string()
        .map_err(|e| e.to_string())?;
    let rows: Vec<serde_json::Value> =
        serde_json::from_str(&body).map_err(|e| format!("Place lookup parse error: {e}"))?;
    let row = rows.first().ok_or_else(|| not_found.to_string())?;
    let lat: f64 = row
        .get("lat")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| "Place lookup missing latitude.".to_string())?;
    let lon: f64 = row
        .get("lon")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| "Place lookup missing longitude.".to_string())?;
    let label = row
        .get("display_name")
        .and_then(|v| v.as_str())
        .unwrap_or("Map area")
        .to_string();
    Ok((lat, lon, label))
}

fn geocode_us_zip(zip: &str) -> Result<(f64, f64, String), String> {
    let zip5: String = zip.chars().filter(|c| c.is_ascii_digit()).take(5).collect();
    let url = format!(
        "https://nominatim.openstreetmap.org/search?postalcode={zip5}&countrycodes=us&format=json&limit=1"
    );
    nominatim_first(&url, &format!("No location found for US ZIP {zip5}."))
}

/// Worldwide place lookup via OpenStreetMap Nominatim.
/// Accepts city / region / postal code / free text. US 5-digit ZIP uses the
/// postalcode endpoint for a tighter match.
pub fn geocode_place(query: &str) -> Result<(f64, f64, String), String> {
    let query = query.trim();
    if query.is_empty() {
        return Err("Enter a city, postal code, or place name.".into());
    }
    if looks_like_us_zip(query) {
        return geocode_us_zip(query);
    }
    let url = format!(
        "https://nominatim.openstreetmap.org/search?q={}&format=json&limit=1",
        percent_encode_query(query)
    );
    nominatim_first(
        &url,
        &format!("No location found for “{query}”. Try a city and country, e.g. Toronto, Canada."),
    )
}

fn slug_id(label: &str) -> String {
    let mut out = String::new();
    for ch in label.chars().flat_map(|c| c.to_lowercase()) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else if (ch == ' ' || ch == '-' || ch == ',') && !out.ends_with('-') {
            out.push('-');
        }
        if out.len() >= 40 {
            break;
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "place".into()
    } else {
        out
    }
}

/// Download (or replace) a pack for an exact bbox after the operator names it.
/// `replace_id` updates an existing pack in place; otherwise a new id is created
/// from the name slug.
pub fn start_named_bbox_download(
    db: std::sync::Arc<Db>,
    name: String,
    bbox: [f64; 4],
    replace_id: Option<String>,
    shape_param: String,
    center_lng: f64,
    center_lat: f64,
    radius_miles: f64,
) {
    let status = download_status();
    if status.running {
        return;
    }
    let [w, s, e, n] = bbox;
    if !(w < e && s < n) {
        set_download(DownloadStatus {
            region_id: String::new(),
            phase: "error".into(),
            detail: "Invalid coverage box.".into(),
            running: false,
        });
        return;
    }
    let name = name.trim().to_string();
    if name.is_empty() {
        set_download(DownloadStatus {
            region_id: String::new(),
            phase: "error".into(),
            detail: "Enter a name for this offline region.".into(),
            running: false,
        });
        return;
    }
    set_download(DownloadStatus {
        region_id: replace_id.clone().unwrap_or_else(|| "area".into()),
        phase: "starting".into(),
        detail: format!("Preparing “{name}”…"),
        running: true,
    });
    std::thread::Builder::new()
        .name("metis-pmtiles-named".into())
        .spawn(move || {
            let result = (|| -> Result<String, String> {
                let radius = if radius_miles > 0.0 {
                    radius_miles.max(5.0).min(300.0)
                } else {
                    radius_miles_from_bbox(bbox)
                };
                let shape = "rectangle".to_string();
                let _ = shape_param; // packs are always a square extract
                let id = if let Some(existing) = replace_id.as_ref() {
                    let mut regions = list_regions(&db);
                    let Some(region) = regions.iter_mut().find(|r| &r.id == existing) else {
                        return Err("Pack to update was not found.".into());
                    };
                    region.name = name.clone();
                    region.bbox = bbox;
                    region.shape = shape;
                    region.center_lng = center_lng;
                    region.center_lat = center_lat;
                    region.radius_miles = radius;
                    region.status = "downloading".into();
                    save_regions(&db, &regions)?;
                    let _ = set_active_region(&db, existing);
                    existing.clone()
                } else {
                    let base = slug_id(&name);
                    let mut id = if base.is_empty() {
                        format!("area-{:.0}", radius)
                    } else {
                        base
                    };
                    // Avoid clobbering another pack with the same slug.
                    let existing_ids: std::collections::HashSet<String> =
                        list_regions(&db).into_iter().map(|r| r.id).collect();
                    if existing_ids.contains(&id) {
                        let mut n = 2u32;
                        while existing_ids.contains(&format!("{id}-{n}")) {
                            n += 1;
                        }
                        id = format!("{id}-{n}");
                    }
                    let region = TileRegion {
                        id: id.clone(),
                        name: name.clone(),
                        bbox,
                        path: region_file_path(&id).display().to_string(),
                        shape,
                        center_lng,
                        center_lat,
                        radius_miles: radius,
                        source_build: String::new(),
                        updated_at: 0,
                        bytes: 0,
                        status: "downloading".into(),
                    };
                    upsert_region(&db, region)?;
                    let _ = set_active_region(&db, &id);
                    id
                };
                Ok(id)
            })();
            match result {
                Ok(id) => {
                    set_download(DownloadStatus {
                        region_id: id.clone(),
                        phase: "starting".into(),
                        detail: format!("Downloading tiles for “{name}”…"),
                        running: true,
                    });
                    match run_download(&db, &id) {
                        Ok(()) => set_download(DownloadStatus {
                            region_id: id,
                            phase: "done".into(),
                            detail: format!(
                                "“{name}” ready. Streets / Terrain / Topo are available inside the region."
                            ),
                            running: false,
                        }),
                        Err(err) => set_download(DownloadStatus {
                            region_id: id,
                            phase: "error".into(),
                            detail: err,
                            running: false,
                        }),
                    }
                    sync_active_path(&db);
                }
                Err(err) => set_download(DownloadStatus {
                    region_id: String::new(),
                    phase: "error".into(),
                    detail: err,
                    running: false,
                }),
            }
        })
        .ok();
}

fn radius_miles_from_bbox(bbox: [f64; 4]) -> f64 {
    let [w, s, e, n] = bbox;
    let lat = (s + n) / 2.0;
    let half_ns = ((n - s) / 2.0) * 69.0;
    let cos_lat = lat.to_radians().cos().abs().max(0.2);
    let half_ew = ((e - w) / 2.0) * 69.0 * cos_lat;
    half_ns.max(half_ew).max(5.0).min(300.0)
}

/// Create/update a region from ZIP + radius (or map/HQ center) and start download.
pub fn start_area_download(
    db: std::sync::Arc<Db>,
    zip: String,
    radius_miles: f64,
    fallback_center: Option<(f64, f64)>,
) {
    let status = download_status();
    if status.running {
        return;
    }
    set_download(DownloadStatus {
        region_id: "area".into(),
        phase: "geocode".into(),
        detail: "Looking up area…".into(),
        running: true,
    });
    std::thread::Builder::new()
        .name("metis-pmtiles-area".into())
        .spawn(move || {
            let result = (|| -> Result<String, String> {
                let radius = radius_miles.max(5.0).min(300.0);
                let (lat, lon, name, id) = if !zip.trim().is_empty() {
                    let (lat, lon, label) = geocode_place(&zip)?;
                    let short = label.split(',').next().unwrap_or(&label).trim();
                    let id = if looks_like_us_zip(&zip) {
                        let zip5: String =
                            zip.chars().filter(|c| c.is_ascii_digit()).take(5).collect();
                        format!("zip-{zip5}")
                    } else {
                        format!("place-{}", slug_id(short))
                    };
                    (
                        lat,
                        lon,
                        format!("{short} · {radius:.0} mi"),
                        id,
                    )
                } else if let Some((lat, lon)) = fallback_center {
                    (
                        lat,
                        lon,
                        format!("Map area · {radius:.0} mi"),
                        format!(
                            "area-{:.2}-{:.2}-{:.0}",
                            lat, lon, radius
                        )
                        .replace('.', "p"),
                    )
                } else if let Some((_, lat, lon)) = db.hq_position().ok().flatten() {
                    (
                        lat,
                        lon,
                        format!("Around HQ · {radius:.0} mi"),
                        "around-hq".into(),
                    )
                } else {
                    return Err(
                        "Enter a city or postal code, or set HQ coordinates / pan the map first."
                            .into(),
                    );
                };
                let region = region_from_center(&id, &name, lat, lon, radius);
                upsert_region(&db, region)?;
                let _ = set_active_region(&db, &id);
                Ok(id)
            })();
            match result {
                Ok(id) => {
                    // Continue with the normal extract path (clears/sets status).
                    set_download(DownloadStatus {
                        region_id: id.clone(),
                        phase: "starting".into(),
                        detail: "Starting download…".into(),
                        running: true,
                    });
                    match run_download(&db, &id) {
                        Ok(()) => set_download(DownloadStatus {
                            region_id: id,
                            phase: "done".into(),
                            detail: "Download complete. Streets / Terrain / Topo are ready."
                                .into(),
                            running: false,
                        }),
                        Err(err) => set_download(DownloadStatus {
                            region_id: id,
                            phase: "error".into(),
                            detail: err,
                            running: false,
                        }),
                    }
                    sync_active_path(&db);
                }
                Err(err) => set_download(DownloadStatus {
                    region_id: String::new(),
                    phase: "error".into(),
                    detail: err,
                    running: false,
                }),
            }
        })
        .ok();
}

fn resolve_latest_build_url() -> Result<String, String> {
    // Official index used by maps.protomaps.com/builds (SPA HTML has no URLs).
    if let Ok(url) = resolve_build_from_metadata() {
        return Ok(url);
    }
    // Fallback: probe dated planet URLs for the last two weeks.
    resolve_build_by_date_probe()
}

fn resolve_build_from_metadata() -> Result<String, String> {
    let body = ureq::get("https://build-metadata.protomaps.dev/builds.json")
        .set("User-Agent", "metis-mesh-hq")
        .timeout(std::time::Duration::from_secs(20))
        .call()
        .map_err(|e| format!("Could not reach Protomaps build metadata: {e}"))?
        .into_string()
        .map_err(|e| e.to_string())?;
    let builds: Vec<serde_json::Value> =
        serde_json::from_str(&body).map_err(|e| format!("Invalid builds.json: {e}"))?;
    let mut keys: Vec<String> = builds
        .iter()
        .filter_map(|b| b.get("key").and_then(|k| k.as_str()).map(str::to_string))
        .filter(|k| k.ends_with(".pmtiles") && k.len() >= 16)
        .collect();
    keys.sort_by(|a, b| b.cmp(a));
    let key = keys
        .first()
        .cloned()
        .ok_or_else(|| "Protomaps builds.json listed no archives.".to_string())?;
    Ok(format!("https://build.protomaps.com/{key}"))
}

fn resolve_build_by_date_probe() -> Result<String, String> {
    let now = chrono_ymd_utc_today().ok_or_else(|| {
        "Could not determine today's date for Protomaps build probe.".to_string()
    })?;
    let mut y = now.0;
    let mut m = now.1;
    let mut d = now.2;
    for _ in 0..16 {
        let stamp = format!("{y:04}{m:02}{d:02}");
        let url = format!("https://build.protomaps.com/{stamp}.pmtiles");
        let ok = ureq::head(&url)
            .set("User-Agent", "metis-mesh-hq")
            .timeout(std::time::Duration::from_secs(12))
            .call()
            .map(|r| r.status() >= 200 && r.status() < 300)
            .unwrap_or(false);
        if ok {
            return Ok(url);
        }
        // Step back one calendar day.
        if d > 1 {
            d -= 1;
        } else if m > 1 {
            m -= 1;
            d = days_in_month(y, m);
        } else {
            y -= 1;
            m = 12;
            d = 31;
        }
    }
    Err("Could not find a recent Protomaps daily build. Check your network and try again.".into())
}

fn chrono_ymd_utc_today() -> Option<(i32, u32, u32)> {
    let now = chrono::Utc::now().date_naive();
    Some((now.year(), now.month(), now.day()))
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
            if leap {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

fn ensure_pmtiles_cli() -> Result<PathBuf, String> {
    if let Ok(path) = which_pmtiles() {
        return Ok(path);
    }
    let dest = bin_dir().join(pmtiles_bin_name());
    if dest.is_file() {
        return Ok(dest);
    }
    download_pmtiles_cli(&dest)?;
    Ok(dest)
}

fn which_pmtiles() -> Result<PathBuf, String> {
    let output = Command::new("sh")
        .arg("-c")
        .arg("command -v pmtiles")
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err("pmtiles not on PATH".into());
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if path.is_empty() {
        return Err("pmtiles not on PATH".into());
    }
    Ok(PathBuf::from(path))
}

fn pmtiles_bin_name() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "pmtiles.exe"
    }
    #[cfg(not(target_os = "windows"))]
    {
        "pmtiles"
    }
}

fn download_pmtiles_cli(dest: &Path) -> Result<(), String> {
    let (os, arch) = pmtiles_release_triplet()?;
    set_download(DownloadStatus {
        region_id: String::new(),
        phase: "cli".into(),
        detail: format!("Downloading pmtiles CLI ({os}/{arch})…"),
        running: true,
    });
    let api = ureq::get("https://api.github.com/repos/protomaps/go-pmtiles/releases/latest")
        .set("User-Agent", "metis-mesh-hq")
        .timeout(std::time::Duration::from_secs(30))
        .call()
        .map_err(|e| format!("GitHub release lookup failed: {e}"))?
        .into_string()
        .map_err(|e| e.to_string())?;
    let needle = format!("_{os}_{arch}.tar.gz");
    let url = api
        .split("\"browser_download_url\"")
        .skip(1)
        .filter_map(|chunk| {
            let start = chunk.find("https://")?;
            let rest = &chunk[start..];
            let end = rest.find('"')?;
            Some(rest[..end].to_string())
        })
        .find(|u| u.contains(&needle))
        .ok_or_else(|| format!("No pmtiles CLI asset for {os}/{arch}"))?;
    let tmp = bin_dir().join("go-pmtiles.tgz");
    {
        let resp = ureq::get(&url)
            .set("User-Agent", "metis-mesh-hq")
            .timeout(std::time::Duration::from_secs(120))
            .call()
            .map_err(|e| format!("CLI download failed: {e}"))?;
        let mut reader = resp.into_reader();
        let mut file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        std::io::copy(&mut reader, &mut file).map_err(|e| e.to_string())?;
    }
    // Extract with tar.
    let status = Command::new("tar")
        .args(["-xzf", tmp.to_str().unwrap_or(""), "-C"])
        .arg(bin_dir())
        .status()
        .map_err(|e| format!("tar extract failed: {e}"))?;
    let _ = std::fs::remove_file(&tmp);
    if !status.success() {
        return Err("Failed to extract pmtiles CLI archive.".into());
    }
    // Binary may be named go-pmtiles or pmtiles depending on release.
    let candidates = [
        bin_dir().join("pmtiles"),
        bin_dir().join("go-pmtiles"),
        bin_dir().join(pmtiles_bin_name()),
    ];
    let found = candidates.into_iter().find(|p| p.is_file());
    let Some(found) = found else {
        return Err("pmtiles binary missing after extract.".into());
    };
    if found != dest {
        let _ = std::fs::rename(&found, dest);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dest, std::fs::Permissions::from_mode(0o755));
    }
    Ok(())
}

fn pmtiles_release_triplet() -> Result<(&'static str, &'static str), String> {
    let os = match std::env::consts::OS {
        "linux" => "Linux",
        "macos" => "Darwin",
        "windows" => "Windows",
        other => return Err(format!("Unsupported OS for pmtiles CLI: {other}")),
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "arm64",
        other => return Err(format!("Unsupported arch for pmtiles CLI: {other}")),
    };
    Ok((os, arch))
}

pub fn start_download(db: std::sync::Arc<Db>, region_id: String) {
    let status = download_status();
    if status.running {
        return;
    }
    set_download(DownloadStatus {
        region_id: region_id.clone(),
        phase: "starting".into(),
        detail: "Starting download…".into(),
        running: true,
    });
    std::thread::Builder::new()
        .name("metis-pmtiles".into())
        .spawn(move || {
            let result = run_download(&db, &region_id);
            match result {
                Ok(()) => set_download(DownloadStatus {
                    region_id: region_id.clone(),
                    phase: "done".into(),
                    detail: "Download complete.".into(),
                    running: false,
                }),
                Err(err) => {
                    let mut regions = list_regions(&db);
                    if let Some(r) = regions.iter_mut().find(|r| r.id == region_id) {
                        r.status = "error".into();
                        let _ = save_regions(&db, &regions);
                    }
                    set_download(DownloadStatus {
                        region_id,
                        phase: "error".into(),
                        detail: err,
                        running: false,
                    });
                }
            }
            sync_active_path(&db);
        })
        .ok();
}

fn run_download(db: &Db, region_id: &str) -> Result<(), String> {
    let mut regions = list_regions(db);
    let Some(region) = regions.iter_mut().find(|r| r.id == region_id) else {
        return Err("Region not found.".into());
    };
    region.status = "downloading".into();
    let bbox = region.bbox;
    let out = if region.path.is_empty() {
        region_file_path(region_id)
    } else {
        PathBuf::from(&region.path)
    };
    let _ = save_regions(db, &regions);

    set_download(DownloadStatus {
        region_id: region_id.into(),
        phase: "cli".into(),
        detail: "Preparing pmtiles CLI…".into(),
        running: true,
    });
    let cli = ensure_pmtiles_cli()?;

    set_download(DownloadStatus {
        region_id: region_id.into(),
        phase: "resolve".into(),
        detail: "Resolving latest Protomaps build…".into(),
        running: true,
    });
    let source = resolve_latest_build_url()?;

    set_download(DownloadStatus {
        region_id: region_id.into(),
        phase: "extract".into(),
        detail: format!(
            "Extracting {}… this can take several minutes.",
            region_id
        ),
        running: true,
    });

    if let Some(parent) = out.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tmp = out.with_extension("pmtiles.partial");
    // Single --bbox=… token: leading '-' otherwise looks like another flag to clap.
    let bbox_flag = format!(
        "--bbox={},{},{},{}",
        bbox[0], bbox[1], bbox[2], bbox[3]
    );
    let min_zoom = min_zoom_for_bbox(bbox);
    let minzoom_flag = format!("--minzoom={min_zoom}");
    let maxzoom_flag = format!("--maxzoom={MAX_ZOOM}");
    let output = Command::new(&cli)
        .args([
            "extract",
            &source,
            tmp.to_str().unwrap_or(""),
            &bbox_flag,
            &minzoom_flag,
            &maxzoom_flag,
        ])
        .output()
        .map_err(|e| format!("pmtiles extract failed to start: {e}"))?;
    if !output.status.success() {
        let _ = std::fs::remove_file(&tmp);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = stderr.trim();
        let detail = if detail.is_empty() {
            stdout.trim()
        } else {
            detail
        };
        if detail.is_empty() {
            return Err("pmtiles extract failed (check network / bbox).".into());
        }
        return Err(format!("pmtiles extract failed: {detail}"));
    }
    std::fs::rename(&tmp, &out).map_err(|e| format!("Could not finalize archive: {e}"))?;

    let mut regions = list_regions(db);
    if let Some(region) = regions.iter_mut().find(|r| r.id == region_id) {
        region.path = out.display().to_string();
        region.source_build = source;
        region.updated_at = unix_now();
        region.status = "ready".into();
        refresh_region_file_meta(region);
    }
    save_regions(db, &regions)?;
    if active_region_id(db).is_none() {
        let _ = set_active_region(db, region_id);
    } else {
        sync_active_path(db);
    }
    Ok(())
}
