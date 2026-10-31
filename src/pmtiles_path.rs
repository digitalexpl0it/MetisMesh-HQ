//! Shared offline PMTiles path for the desktop custom protocol + Map style.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

static PMTILES_PATH: Mutex<Option<PathBuf>> = Mutex::new(None);

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

/// URL the MapLibre PMTiles protocol should fetch (custom `metis` scheme).
pub fn basemap_protocol_url() -> Option<&'static str> {
    // Host `local` so the path is `/pmtiles/basemap` (not host=`pmtiles`, path=`/basemap`).
    current_pmtiles_path().map(|_| "metis://local/pmtiles/basemap")
}
