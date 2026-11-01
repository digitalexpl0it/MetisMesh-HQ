//! NOAA / NWS active alerts near a point (US only).

use serde::Deserialize;

use crate::signup::fit_mesh;

#[derive(Debug, Clone)]
pub struct NwsAlert {
    pub id: String,
    pub event: String,
    pub severity: String,
    pub area: String,
    pub ends: Option<String>,
    pub office: String,
    pub url: String,
}

#[derive(Debug, Deserialize)]
struct AlertsResponse {
    features: Vec<AlertFeature>,
}

#[derive(Debug, Deserialize)]
struct AlertFeature {
    id: Option<String>,
    properties: Option<AlertProps>,
}

#[derive(Debug, Deserialize)]
struct AlertProps {
    id: Option<String>,
    event: Option<String>,
    severity: Option<String>,
    #[serde(rename = "areaDesc")]
    area_desc: Option<String>,
    ends: Option<String>,
    expires: Option<String>,
    #[serde(rename = "senderName")]
    sender_name: Option<String>,
}

/// Blocking HTTP for active alerts at lat/lon.
pub fn fetch_active_alerts(lat: f64, lon: f64) -> Result<Vec<NwsAlert>, String> {
    let url = format!(
        "https://api.weather.gov/alerts/active?point={lat:.4},{lon:.4}&status=actual"
    );
    let body = crate::services::http_get(&url)
        .map_err(|e| format!("NWS fetch failed: {e}"))?
        .into_string()
        .map_err(|e| e.to_string())?;
    let parsed: AlertsResponse =
        serde_json::from_str(&body).map_err(|e| format!("NWS parse error: {e}"))?;
    let mut out = Vec::new();
    for feat in parsed.features {
        let props = match feat.properties {
            Some(p) => p,
            None => continue,
        };
        let id = props
            .id
            .or(feat.id)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("nws-{}", out.len()));
        let ends = props.ends.or(props.expires);
        let office = props
            .sender_name
            .unwrap_or_default()
            .replace("NWS ", "")
            .chars()
            .take(24)
            .collect::<String>();
        let url = alert_url_from_id(&id);
        out.push(NwsAlert {
            id,
            event: props.event.unwrap_or_else(|| "Alert".into()),
            severity: props.severity.unwrap_or_else(|| "Unknown".into()),
            area: props.area_desc.unwrap_or_default(),
            ends,
            office,
            url,
        });
    }
    Ok(out)
}

fn alert_url_from_id(id: &str) -> String {
    // api.weather.gov alert ids are often full URNs; prefer the HTTPS form when present.
    if id.starts_with("http://") || id.starts_with("https://") {
        return id.to_string();
    }
    format!("https://api.weather.gov/alerts/{id}")
}

pub fn format_nws_alert(alert: &NwsAlert, short_url: Option<&str>) -> String {
    let sev = severity_tag(&alert.severity);
    let event = compact(&alert.event, 18);
    let until = alert
        .ends
        .as_deref()
        .and_then(short_until)
        .unwrap_or_else(|| "ongoing".into());
    let area = first_area(&alert.area, 20);
    let link = short_url
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_default();
    if link.is_empty() {
        fit_mesh(&format!(
            "NWS {sev} {event} near {area} til {until} · {office}",
            office = compact(&alert.office, 12)
        ))
    } else {
        fit_mesh(&format!(
            "NWS {sev} {event} near {area} til {until} · {link}"
        ))
    }
}

fn severity_tag(severity: &str) -> &'static str {
    match severity.to_ascii_lowercase().as_str() {
        "extreme" => "EXT",
        "severe" => "SEV",
        "moderate" => "MOD",
        "minor" => "MIN",
        _ => "ALT",
    }
}

fn first_area(area: &str, max_chars: usize) -> String {
    let first = area.split(';').next().unwrap_or(area).trim();
    compact(first, max_chars)
}

fn compact(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return "area".into();
    }
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    trimmed.chars().take(max_chars.saturating_sub(1)).collect::<String>() + "…"
}

fn short_until(iso: &str) -> Option<String> {
    // "2026-11-01T21:00:00-07:00" → "21:00"
    let t = iso.split('T').nth(1)?;
    let hhmm = t.get(0..5)?;
    Some(hhmm.to_string())
}

/// Best-effort is.gd short link; returns None on failure (never blocks alert).
pub fn shorten_url(long_url: &str) -> Option<String> {
    if long_url.is_empty() {
        return None;
    }
    let encoded = urlencode(long_url);
    let api = format!("https://is.gd/create.php?format=simple&url={encoded}");
    let body = crate::services::http_get(&api)
        .ok()?
        .into_string()
        .ok()?;
    let trimmed = body.trim();
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        Some(trimmed.to_string())
    } else {
        None
    }
}

fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len() * 3);
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_nws_without_link() {
        let alert = NwsAlert {
            id: "urn:oid:1".into(),
            event: "Wind Advisory".into(),
            severity: "Moderate".into(),
            area: "Los Angeles County; Orange County".into(),
            ends: Some("2026-11-01T21:00:00-07:00".into()),
            office: "NWS Los Angeles CA".into(),
            url: String::new(),
        };
        let line = format_nws_alert(&alert, None);
        assert!(line.len() <= 160);
        assert!(line.contains("NWS MOD"));
        assert!(line.contains("21:00"));
    }
}
