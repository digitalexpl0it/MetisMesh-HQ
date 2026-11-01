//! USGS FDSN earthquake feed for a bounding box.

use serde::Deserialize;

use crate::signup::fit_mesh;

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

pub fn format_quake(event: &QuakeEvent) -> String {
    let when = if event.time_ms > 0 {
        let secs = event.time_ms / 1000;
        chrono::DateTime::from_timestamp(secs, 0)
            .map(|dt| dt.format("%H:%MZ").to_string())
            .unwrap_or_else(|| "?".into())
    } else {
        "?".into()
    };
    let place = compact_place(&event.place, 42);
    fit_mesh(&format!(
        "USGS M{mag:.1} · {place} · {when} · d{depth:.0}km",
        mag = event.magnitude,
        depth = event.depth_km.abs(),
    ))
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
        let line = format_quake(&event);
        assert!(line.len() <= 160);
        assert!(line.starts_with("USGS M3.2"));
        assert!(line.contains("Borrego"));
    }
}
