//! Community LoRa radio presets (MeshCore App API + offline cache).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};

const API_URL: &str = "https://api.meshcore.nz/api/v1/config";
const USER_AGENT: &str = "MetisMeshHQ/0.1";
const CACHE_NAME: &str = "meshcore_presets.json";
const CACHE_MAX_AGE_SECS: u64 = 86_400;

static PENDING_PATH_HASH_MODE: Mutex<Option<u8>> = Mutex::new(None);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RadioPreset {
    pub name: String,
    pub freq_mhz: f64,
    pub bw_khz: f64,
    pub sf: u8,
    pub cr: u8,
    #[serde(default)]
    pub recommended: bool,
    /// Byte count from API `network_settings.path_hash_size` (e.g. 2 or 3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_hash_size: Option<u8>,
}

impl RadioPreset {
    pub fn summary(&self) -> String {
        let mut s = format!(
            "{:.3} MHz / SF{} / BW{} / CR{}",
            self.freq_mhz, self.sf, trim_float(self.bw_khz), self.cr
        );
        if let Some(size) = self.path_hash_size {
            s.push_str(&format!(" / {size}B path"));
        }
        s
    }

    pub fn matches(&self, freq_mhz: f64, bw_khz: f64, sf: u8, cr: u8) -> bool {
        (self.freq_mhz - freq_mhz).abs() < 0.001
            && (self.bw_khz - bw_khz).abs() < 0.05
            && self.sf == sf
            && self.cr == cr
    }

    /// Companion `path_hash_mode` is 0/1/2 for 1/2/3-byte hashes.
    pub fn path_hash_mode(&self) -> Option<u8> {
        self.path_hash_size.map(|size| size.saturating_sub(1).min(2))
    }
}

fn trim_float(value: f64) -> String {
    if (value - value.round()).abs() < 0.05 {
        format!("{}", value.round() as i64)
    } else {
        format!("{value}")
    }
}

pub fn presets_cache_path() -> PathBuf {
    crate::data_dir().join(CACHE_NAME)
}

fn bundled_presets() -> Vec<RadioPreset> {
    serde_json::from_str(include_str!("../assets/radio_presets.json")).unwrap_or_else(|err| {
        eprintln!("radio presets (bundled): {err}");
        Vec::new()
    })
}

fn load_presets_file(path: &Path) -> Option<Vec<RadioPreset>> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text)
        .ok()
        .filter(|v: &Vec<RadioPreset>| !v.is_empty())
}

fn cache_is_fresh(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    let Ok(modified) = meta.modified() else {
        return false;
    };
    let Ok(age) = std::time::SystemTime::now().duration_since(modified) else {
        return false;
    };
    age <= Duration::from_secs(CACHE_MAX_AGE_SECS)
}

/// Prefer on-disk cache when present; otherwise the shipped snapshot.
pub fn community_presets() -> Vec<RadioPreset> {
    let cache = presets_cache_path();
    if let Some(presets) = load_presets_file(&cache) {
        return presets;
    }
    bundled_presets()
}

pub fn preset_label_for(freq_mhz: f64, bw_khz: f64, sf: u8, cr: u8) -> String {
    community_presets()
        .into_iter()
        .find(|preset| preset.matches(freq_mhz, bw_khz, sf, cr))
        .map(|preset| preset.name)
        .unwrap_or_else(|| "Custom".into())
}

pub fn loc_policy_label(policy: u8) -> &'static str {
    match policy {
        0 => "None (no location in advert)",
        1 => "Share (GPS / sensor)",
        2 => "Prefs (stored lat/lon)",
        _ => "Unknown",
    }
}

pub fn queue_path_hash_mode(mode: u8) {
    if let Ok(mut guard) = PENDING_PATH_HASH_MODE.lock() {
        *guard = Some(mode.min(2));
    }
}

pub fn take_pending_path_hash_mode() -> Option<u8> {
    PENDING_PATH_HASH_MODE
        .lock()
        .ok()
        .and_then(|mut guard| guard.take())
}

#[derive(Debug, Deserialize)]
struct ApiRoot {
    config: ApiConfig,
}

#[derive(Debug, Deserialize)]
struct ApiConfig {
    suggested_radio_settings: ApiSuggested,
}

#[derive(Debug, Deserialize)]
struct ApiSuggested {
    entries: Vec<ApiEntry>,
}

#[derive(Debug, Deserialize)]
struct ApiEntry {
    title: String,
    #[serde(deserialize_with = "de_f64_flexible")]
    frequency: f64,
    #[serde(deserialize_with = "de_f64_flexible")]
    bandwidth: f64,
    #[serde(deserialize_with = "de_u8_flexible")]
    spreading_factor: u8,
    #[serde(deserialize_with = "de_u8_flexible")]
    coding_rate: u8,
    #[serde(default)]
    network_settings: Option<ApiNetworkSettings>,
}

#[derive(Debug, Deserialize)]
struct ApiNetworkSettings {
    #[serde(default)]
    path_hash_size: Option<u8>,
}

fn de_f64_flexible<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<f64, D::Error> {
    use serde::de::{self, Visitor};
    struct V;
    impl<'de> Visitor<'de> for V {
        type Value = f64;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("number or numeric string")
        }
        fn visit_f64<E: de::Error>(self, v: f64) -> Result<f64, E> {
            Ok(v)
        }
        fn visit_i64<E: de::Error>(self, v: i64) -> Result<f64, E> {
            Ok(v as f64)
        }
        fn visit_u64<E: de::Error>(self, v: u64) -> Result<f64, E> {
            Ok(v as f64)
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<f64, E> {
            v.parse().map_err(E::custom)
        }
    }
    deserializer.deserialize_any(V)
}

fn de_u8_flexible<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u8, D::Error> {
    use serde::de::{self, Visitor};
    struct V;
    impl<'de> Visitor<'de> for V {
        type Value = u8;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("integer or numeric string")
        }
        fn visit_u64<E: de::Error>(self, v: u64) -> Result<u8, E> {
            u8::try_from(v).map_err(E::custom)
        }
        fn visit_i64<E: de::Error>(self, v: i64) -> Result<u8, E> {
            u8::try_from(v).map_err(E::custom)
        }
        fn visit_f64<E: de::Error>(self, v: f64) -> Result<u8, E> {
            if (0.0..=255.0).contains(&v) && (v - v.round()).abs() < f64::EPSILON {
                Ok(v as u8)
            } else {
                Err(E::custom("not a u8"))
            }
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<u8, E> {
            v.parse().map_err(E::custom)
        }
    }
    deserializer.deserialize_any(V)
}

fn api_entries_to_presets(entries: Vec<ApiEntry>) -> Vec<RadioPreset> {
    entries
        .into_iter()
        .map(|entry| {
            let recommended = matches!(
                entry.title.as_str(),
                "USA" | "USA - Southern California" | "Canada"
            );
            RadioPreset {
                name: entry.title,
                freq_mhz: entry.frequency,
                bw_khz: entry.bandwidth,
                sf: entry.spreading_factor,
                cr: entry.coding_rate,
                recommended,
                path_hash_size: entry.network_settings.and_then(|n| n.path_hash_size),
            }
        })
        .collect()
}

/// Fetch MeshCore community presets and write `data/meshcore_presets.json`.
pub fn fetch_and_cache_presets() -> Result<Vec<RadioPreset>, String> {
    let body = ureq::get(API_URL)
        .set("User-Agent", USER_AGENT)
        .timeout(Duration::from_secs(12))
        .call()
        .map_err(|err| err.to_string())?
        .into_string()
        .map_err(|err| err.to_string())?;
    let root: ApiRoot = serde_json::from_str(&body).map_err(|err| err.to_string())?;
    let presets = api_entries_to_presets(root.config.suggested_radio_settings.entries);
    if presets.is_empty() {
        return Err("API returned no radio presets".into());
    }
    let path = presets_cache_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let json = serde_json::to_string_pretty(&presets).map_err(|err| err.to_string())?;
    fs::write(&path, json).map_err(|err| err.to_string())?;
    Ok(presets)
}

/// Refresh cache when missing or stale. Safe on a background thread.
pub fn refresh_presets_cache_if_needed() -> Result<bool, String> {
    let path = presets_cache_path();
    if path.is_file() && cache_is_fresh(&path) {
        return Ok(false);
    }
    fetch_and_cache_presets()?;
    Ok(true)
}

pub const BW_OPTIONS_KHZ: &[f64] = &[62.5, 125.0, 250.0];
pub const SF_OPTIONS: &[u8] = &[5, 6, 7, 8, 9, 10, 11, 12];
pub const CR_OPTIONS: &[u8] = &[5, 6, 7, 8];
