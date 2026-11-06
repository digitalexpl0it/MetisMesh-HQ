//! Shared offline PMTiles path for the desktop custom protocol + Map style.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

static PMTILES_PATH: Mutex<Option<PathBuf>> = Mutex::new(None);
static ACTIVE_REGION: Mutex<Option<String>> = Mutex::new(None);

pub fn set_pmtiles_path(path: Option<impl AsRef<Path>>) {
    let next = path.and_then(|p| {
        let path = p.as_ref();
        if path.as_os_str().is_empty() {
            None
        } else {
            Some(path.to_path_buf())
        }
    });
    if let Ok(mut guard) = PMTILES_PATH.lock() {
        *guard = next;
    }
}

pub fn current_pmtiles_path() -> Option<PathBuf> {
    PMTILES_PATH
        .lock()
        .ok()
        .and_then(|guard| guard.clone())
        .filter(|path| path.is_file())
}

pub fn set_active_region_id(id: Option<String>) {
    if let Ok(mut guard) = ACTIVE_REGION.lock() {
        *guard = id.filter(|s| !s.is_empty());
    }
}

pub fn active_region_id() -> Option<String> {
    ACTIVE_REGION.lock().ok().and_then(|g| g.clone())
}

/// URL the MapLibre PMTiles protocol should fetch (custom `metis` scheme).
/// `v=` is the file mtime so a re-download of the same region id busts the JS cache.
/// On Windows this is `http://metis.localhost/…` — WebView2 will not fetch `metis://`.
pub fn basemap_protocol_url() -> Option<String> {
    let path = current_pmtiles_path()?;
    let id = active_region_id().unwrap_or_else(|| "basemap".into());
    let v = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Some(crate::brand::metis_asset_url(&format!("pmtiles/{id}?v={v}")))
}
