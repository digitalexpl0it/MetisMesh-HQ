//! USGS FDSN earthquake feed for a bounding box.

use serde::Deserialize;

use crate::db::Db;
use crate::signup::fit_mesh;
use crate::timefmt;

#[derive(Debug, Clone)]
pub struct QuakeEvent {
    pub id: String,
    pub magnitude: f64,
    pub place: String,
    pub time_ms: i64,
    pub depth_km: f64,
    pub lat: f64,
    pub lon: f64,
    pub url: String,
}

#[derive(Debug, Deserialize)]
struct UsgsResponse {
    features: Vec<UsgsFeature>,
}

#[derive(Debug, Deserialize)]
struct UsgsFeature {
    id: Option<String>,
    properties: Option<UsgsProps>,
    geometry: Option<UsgsGeometry>,
}

#[derive(Debug, Deserialize)]
struct UsgsProps {
    mag: Option<f64>,
    place: Option<String>,
    time: Option<i64>,
    url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UsgsGeometry {
    coordinates: Option<Vec<f64>>,
}

/// Blocking HTTP. `window_minutes` caps how far back to look.
pub fn fetch_quakes(
    min_lat: f64,
    max_lat: f64,
    min_lon: f64,
    max_lon: f64,
    min_magnitude: f64,
    window_minutes: u64,
) -> Result<Vec<QuakeEvent>, String> {
    let start = chrono::Utc::now() - chrono::Duration::minutes(window_minutes as i64);
    let start_iso = start.format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let url = format!(
        "https://earthquake.usgs.gov/fdsnws/event/1/query?format=geojson\
         &starttime={start_iso}\
         &minmagnitude={min_magnitude}\
         &minlatitude={min_lat}&maxlatitude={max_lat}\
         &minlongitude={min_lon}&maxlongitude={max_lon}\
         &orderby=time"
    );
    let body = crate::services::http_get(&url)
        .map_err(|e| format!("USGS fetch failed: {e}"))?
        .into_string()
        .map_err(|e| e.to_string())?;
    let parsed: UsgsResponse =
        serde_json::from_str(&body).map_err(|e| format!("USGS parse error: {e}"))?;
    let mut out = Vec::new();
    for feat in parsed.features {
        let props = feat.properties.unwrap_or(UsgsProps {
            mag: None,
            place: None,
            time: None,
            url: None,
        });
        let coords = feat
            .geometry
            .and_then(|g| g.coordinates)
            .unwrap_or_default();
        let lon = coords.first().copied().unwrap_or(0.0);
        let lat = coords.get(1).copied().unwrap_or(0.0);
        let depth = coords.get(2).copied().unwrap_or(0.0);
        let id = feat
            .id
            .filter(|s| !s.is_empty())
            .or_else(|| {
                props.time.map(|t| format!("usgs-{t}-{}", props.mag.unwrap_or(0.0)))
            })
            .unwrap_or_else(|| format!("usgs-{}", out.len()));
        out.push(QuakeEvent {
            id,
            magnitude: props.mag.unwrap_or(0.0),
            place: props.place.unwrap_or_else(|| "unknown".into()),
            time_ms: props.time.unwrap_or(0),
            depth_km: depth,
            lat,
            lon,
            url: props.url.unwrap_or_default(),
        });
    }
    Ok(out)
}

pub fn format_quake(db: &Db, event: &QuakeEvent) -> String {
    let when = if event.time_ms > 0 {
        // Local TZ + Settings date/time prefs (same as Messages UI).
        timefmt::format_stamp(db, event.time_ms / 1000)
    } else {
        "?".into()
    };
    // Leave room for the formatted stamp (can be longer than HH:MMZ).
    let place_budget = 42usize.saturating_sub(when.len().saturating_sub(6));
    let place = compact_place(&event.place, place_budget.max(24));
    // USGS depth is always km; follow weather unit (°F → miles).
    let depth = depth_label(event.depth_km.abs(), crate::services::weather_unit_f(db));
    fit_mesh(&format!(
        "USGS M{mag:.1} · {place} · {when} · {depth}",
        mag = event.magnitude,
    ))
}

fn depth_label(depth_km: f64, imperial: bool) -> String {
    if imperial {
        format!("d{:.0}mi", depth_km * 0.621_371)
    } else {
        format!("d{depth_km:.0}km")
    }
}

fn compact_place(place: &str, max_chars: usize) -> String {
    let trimmed = place.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    trimmed.chars().take(max_chars.saturating_sub(1)).collect::<String>() + "…"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_labeled_quake() {
        let db = Db::from_conn(rusqlite::Connection::open_in_memory().unwrap()).unwrap();
        db.ensure_defaults().unwrap();
        let _ = db.set_setting("time_format", "12h");
        let _ = db.set_setting("date_format", "md_short");
        let _ = db.set_setting("svc_weather_unit", "F");
        let event = QuakeEvent {
            id: "ci123".into(),
            magnitude: 3.2,
            place: "12km NW of Borrego Springs, CA".into(),
            time_ms: 1_730_000_000_000,
            depth_km: 12.0,
            lat: 33.28,
            lon: -116.42,
            url: String::new(),
        };
        let line = format_quake(&db, &event);
        assert!(line.len() <= 160, "{line}");
        assert!(line.starts_with("USGS M3.2"));
        assert!(line.contains("Borrego"));
        assert!(!line.contains('Z'), "{line}");
        assert!(
            line.contains("AM") || line.contains("PM"),
            "{line}"
        );
        assert!(line.contains("d7mi"), "{line}"); // 12km ≈ 7mi
        assert!(!line.contains("d12km"), "{line}");
    }

    #[test]
    fn depth_metric_when_celsius() {
        assert_eq!(depth_label(20.0, false), "d20km");
        assert_eq!(depth_label(20.0, true), "d12mi");
    }
}
