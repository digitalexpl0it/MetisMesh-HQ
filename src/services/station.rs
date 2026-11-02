//! Shared local weather-station cache (MQTT + Ecowitt push/pull).
//!
//! Bare `#weather` and the Weather page both read from here. Open-Meteo stays
//! for ZIP/place lookups only.

use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use crate::db::Db;
use crate::signup::fit_mesh;

pub const DEFAULT_STALE_SECS: i64 = 3600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StationSource {
    Mqtt,
    EcowittPush,
    EcowittPull,
}

impl StationSource {
    pub fn as_label(self) -> &'static str {
        match self {
            Self::Mqtt => "MQTT",
            Self::EcowittPush => "Ecowitt push",
            Self::EcowittPull => "Ecowitt pull",
        }
    }
}

#[derive(Debug, Clone)]
pub struct StationReading {
    pub received_at: Instant,
    pub source: StationSource,
    /// Outdoor temp °F when known.
    pub outdoor_temp_f: Option<f64>,
    /// Outdoor temp °C when known.
    pub outdoor_temp_c: Option<f64>,
    pub humidity_pct: Option<f64>,
    pub feels_like_f: Option<f64>,
    pub wind_mph: Option<f64>,
    pub wind_gust_mph: Option<f64>,
    pub wind_dir_deg: Option<f64>,
    pub pressure_inhg: Option<f64>,
    pub pressure_hpa: Option<f64>,
    pub rain_day_in: Option<f64>,
    pub rain_rate_in: Option<f64>,
    pub uv_index: Option<f64>,
    pub solar_wm2: Option<f64>,
    pub indoor_temp_f: Option<f64>,
    pub indoor_humidity_pct: Option<f64>,
    pub device: String,
    /// Pre-formatted mesh body (MQTT template/passthrough). When set, used for air.
    pub mesh_body: Option<String>,
    /// True when `mesh_body` is already a full `WX …` line.
    pub mesh_full_line: bool,
}

impl Default for StationSource {
    fn default() -> Self {
        Self::Mqtt
    }
}

impl Default for StationReading {
    fn default() -> Self {
        Self {
            received_at: Instant::now(),
            source: StationSource::Mqtt,
            outdoor_temp_f: None,
            outdoor_temp_c: None,
            humidity_pct: None,
            feels_like_f: None,
            wind_mph: None,
            wind_gust_mph: None,
            wind_dir_deg: None,
            pressure_inhg: None,
            pressure_hpa: None,
            rain_day_in: None,
            rain_rate_in: None,
            uv_index: None,
            solar_wm2: None,
            indoor_temp_f: None,
            indoor_humidity_pct: None,
            device: String::new(),
            mesh_body: None,
            mesh_full_line: false,
        }
    }
}

fn cache() -> &'static Mutex<Option<StationReading>> {
    static CACHE: OnceLock<Mutex<Option<StationReading>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// Shared stale window. Prefers `svc_weather_station_stale_secs`, else MQTT key.
pub fn station_stale_secs(db: &Db) -> i64 {
    if let Some(raw) = db
        .setting("svc_weather_station_stale_secs")
        .ok()
        .flatten()
        .filter(|s| !s.trim().is_empty())
    {
        return raw
            .parse::<i64>()
            .unwrap_or(DEFAULT_STALE_SECS)
            .clamp(60, 7 * 24 * 3600);
    }
    crate::services::setting_i64(db, "svc_weather_mqtt_stale_secs", DEFAULT_STALE_SECS)
        .clamp(60, 7 * 24 * 3600)
}

pub fn station_label(db: &Db) -> String {
    let label = db
        .setting("svc_weather_mqtt_label")
        .ok()
        .flatten()
        .unwrap_or_default();
    let label = label.trim();
    if label.is_empty() {
        "Yard".into()
    } else {
        label.to_string()
    }
}

pub fn any_station_source_enabled(db: &Db) -> bool {
    crate::services::setting_bool(db, "svc_weather_mqtt_enabled", false)
        || crate::services::setting_bool(db, "svc_weather_ecowitt_push_enabled", false)
        || crate::services::setting_bool(db, "svc_weather_ecowitt_pull_enabled", false)
}

/// Publish a reading. Newer wall-clock Instant always replaces the previous.
pub fn publish_station_reading(reading: StationReading) {
    if let Ok(mut guard) = cache().lock() {
        *guard = Some(reading);
    }
}

#[cfg(test)]
pub fn clear_station_cache() {
    if let Ok(mut g) = cache().lock() {
        *g = None;
    }
}

pub fn current_station_reading() -> Option<StationReading> {
    cache().lock().ok().and_then(|g| g.clone())
}

pub fn station_is_fresh(db: &Db) -> bool {
    if !any_station_source_enabled(db) {
        return false;
    }
    let Some(snap) = current_station_reading() else {
        return false;
    };
    snap.received_at.elapsed().as_secs() as i64 <= station_stale_secs(db)
}

pub fn outdoor_temp_display(reading: &StationReading, fahrenheit: bool) -> Option<String> {
    if fahrenheit {
        reading
            .outdoor_temp_f
            .or_else(|| reading.outdoor_temp_c.map(|c| c * 9.0 / 5.0 + 32.0))
            .map(|t| format!("{t:.0}F"))
    } else {
        reading
            .outdoor_temp_c
            .or_else(|| reading.outdoor_temp_f.map(|f| (f - 32.0) * 5.0 / 9.0))
            .map(|t| format!("{t:.0}C"))
    }
}

fn compact_mesh_body(reading: &StationReading, fahrenheit: bool) -> Option<String> {
    let temp = outdoor_temp_display(reading, fahrenheit)?;
    let mut parts = vec![temp];
    if let Some(h) = reading.humidity_pct {
        parts.push(format!("{h:.0}%"));
    }
    if let Some(spd) = reading.wind_mph {
        let wind = if fahrenheit {
            format!("{spd:.0}mph")
        } else {
            format!("{:.0}kmh", spd * 1.60934)
        };
        if let Some(dir) = reading.wind_dir_deg {
            parts.push(format!("{}{wind}", compass(dir)));
        } else {
            parts.push(wind);
        }
    }
    Some(parts.join(" "))
}

fn compass(deg: f64) -> &'static str {
    const DIRS: [&str; 8] = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
    let idx = ((deg.rem_euclid(360.0) + 22.5) / 45.0) as usize % 8;
    DIRS[idx]
}

/// Compact `WX {label}: …` when any station source is enabled and the cache is fresh.
pub fn try_station_wx_line(db: &Db, fahrenheit: bool) -> Option<String> {
    if !any_station_source_enabled(db) {
        return None;
    }
    let snap = current_station_reading()?;
    if snap.received_at.elapsed().as_secs() as i64 > station_stale_secs(db) {
        return None;
    }
    let label = station_label(db);
    if let Some(body) = snap.mesh_body.as_ref().filter(|s| !s.trim().is_empty()) {
        let line = if snap.mesh_full_line {
            body.clone()
        } else {
            format!("WX {label}: {body}")
        };
        return Some(fit_mesh(&line));
    }
    let body = compact_mesh_body(&snap, fahrenheit)?;
    Some(fit_mesh(&format!("WX {label}: {body}")))
}

pub fn age_label(age_secs: i64) -> String {
    if age_secs < 60 {
        format!("{age_secs}s ago")
    } else if age_secs < 3600 {
        format!("{}m ago", age_secs / 60)
    } else {
        format!("{}h ago", age_secs / 3600)
    }
}

#[cfg(test)]
pub fn inject_mesh_only_for_tests(body: &str, full_line: bool, age_secs: u64) {
    let received_at = Instant::now()
        .checked_sub(std::time::Duration::from_secs(age_secs))
        .unwrap_or_else(Instant::now);
    publish_station_reading(StationReading {
        received_at,
        source: StationSource::Mqtt,
        mesh_body: Some(body.to_string()),
        mesh_full_line: full_line,
        ..StationReading::default()
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn fresh_typed_reading_formats_wx() {
        clear_station_cache();
        let db = Db::from_conn(Connection::open_in_memory().unwrap()).unwrap();
        db.ensure_defaults().unwrap();
        let _ = db.set_setting("svc_weather_ecowitt_push_enabled", "1");
        let _ = db.set_setting("svc_weather_mqtt_label", "Yard");
        let _ = db.set_setting("svc_weather_station_stale_secs", "3600");
        publish_station_reading(StationReading {
            received_at: Instant::now(),
            source: StationSource::EcowittPush,
            outdoor_temp_f: Some(68.0),
            humidity_pct: Some(41.0),
            wind_mph: Some(5.0),
            wind_dir_deg: Some(180.0),
            ..StationReading::default()
        });
        assert!(station_is_fresh(&db));
        let line = try_station_wx_line(&db, true).expect("line");
        assert!(line.starts_with("WX Yard:"), "{line}");
        assert!(line.contains("68F"), "{line}");
        assert!(line.len() <= 160, "{line}");
        clear_station_cache();
    }

    #[test]
    fn stale_returns_none() {
        clear_station_cache();
        let db = Db::from_conn(Connection::open_in_memory().unwrap()).unwrap();
        db.ensure_defaults().unwrap();
        let _ = db.set_setting("svc_weather_mqtt_enabled", "1");
        let _ = db.set_setting("svc_weather_station_stale_secs", "3600");
        inject_mesh_only_for_tests("68F 41%", false, 7200);
        assert!(!station_is_fresh(&db));
        assert!(try_station_wx_line(&db, true).is_none());
        clear_station_cache();
    }
}
