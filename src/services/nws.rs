//! NOAA / NWS active alerts (weather + IPAWS-style emergencies including Amber).

use serde::Deserialize;

use crate::signup::{fit_mesh, mesh_frames};

/// Child Abduction / Amber and related non-weather emergency SAME codes.
pub const AMBER_CODES: &str = "CAE";
/// Broader civil / public-safety emergencies (not routine weather advisories).
pub const EMERG_CODES: &str =
    "CAE,BLU,CDW,CEM,EVI,FRW,HMW,LAE,LEW,NUW,EQW,SPW,RHW,TOE";

#[derive(Debug, Clone)]
pub struct NwsAlert {
    pub id: String,
    pub event: String,
    pub severity: String,
    pub area: String,
    pub ends: Option<String>,
    pub office: String,
    pub url: String,
    pub event_code: String,
    /// Full CAP description (vehicle / person / location narrative for Amber).
    pub description: String,
    pub instruction: String,
    pub headline: String,
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
    headline: Option<String>,
    description: Option<String>,
    instruction: Option<String>,
    #[serde(rename = "eventCode", default)]
    event_code: Option<Vec<EventCodeEntry>>,
}

#[derive(Debug, Deserialize)]
struct EventCodeEntry {
    value: Option<String>,
}

/// Blocking HTTP for active weather alerts at lat/lon.
pub fn fetch_active_alerts(lat: f64, lon: f64) -> Result<Vec<NwsAlert>, String> {
    let url = format!(
        "https://api.weather.gov/alerts/active?point={lat:.4},{lon:.4}&status=actual"
    );
    fetch_alerts_url(&url)
}

/// Amber / civil emergencies for a US state (e.g. `CA`), via NWS CAP codes.
pub fn fetch_emergency_alerts(state: &str, codes: &str) -> Result<Vec<NwsAlert>, String> {
    let state = state.trim().to_ascii_uppercase();
    if state.len() != 2 || !state.chars().all(|c| c.is_ascii_alphabetic()) {
        return Err("Emergency alerts need a 2-letter US state (Settings → Services).".into());
    }
    let url = format!(
        "https://api.weather.gov/alerts/active?area={state}&status=actual&code={codes}"
    );
    fetch_alerts_url(&url)
}

fn fetch_alerts_url(url: &str) -> Result<Vec<NwsAlert>, String> {
    let body = crate::services::http_get(url)
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
        let code = props
            .event_code
            .as_ref()
            .and_then(|codes| {
                codes.iter().find_map(|c| {
                    c.value
                        .as_deref()
                        .filter(|v| v.len() == 3 && v.chars().all(|ch| ch.is_ascii_alphabetic()))
                        .map(|v| v.to_ascii_uppercase())
                })
            })
            .unwrap_or_default();
        let url = alert_url_from_id(&id);
        out.push(NwsAlert {
            id,
            event: props.event.unwrap_or_else(|| "Alert".into()),
            severity: props.severity.unwrap_or_else(|| "Unknown".into()),
            area: props.area_desc.unwrap_or_default(),
            ends,
            office,
            url,
            event_code: code,
            description: collapse_ws(&props.description.unwrap_or_default()),
            instruction: collapse_ws(&props.instruction.unwrap_or_default()),
            headline: collapse_ws(&props.headline.unwrap_or_default()),
        });
    }
    Ok(out)
}

fn collapse_ws(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn is_amber_event(alert: &NwsAlert) -> bool {
    alert.event_code.eq_ignore_ascii_case("CAE")
        || alert.event.to_ascii_lowercase().contains("child abduction")
        || alert.event.to_ascii_lowercase().contains("amber")
}

fn alert_url_from_id(id: &str) -> String {
    // api.weather.gov alert ids are often full URNs; prefer the HTTPS form when present.
    if id.starts_with("http://") || id.starts_with("https://") {
        return id.to_string();
    }
    format!("https://api.weather.gov/alerts/{id}")
}

fn alert_prefix(alert: &NwsAlert) -> &'static str {
    if is_amber_event(alert) {
        "AMBER"
    } else if !alert.event_code.is_empty()
        && EMERG_CODES
            .split(',')
            .any(|c| c.eq_ignore_ascii_case(&alert.event_code))
    {
        "ALERT"
    } else {
        "NWS"
    }
}

pub fn format_nws_alert(alert: &NwsAlert, short_url: Option<&str>) -> String {
    let prefix = alert_prefix(alert);
    let sev = severity_tag(&alert.severity);
    let event = compact(&alert.event, 22);
    let until = alert
        .ends
        .as_deref()
        .and_then(short_until)
        .unwrap_or_else(|| "ongoing".into());
    let area = first_area(&alert.area, 22);
    let link = short_url
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_default();
    if link.is_empty() {
        fit_mesh(&format!(
            "{prefix} {sev} {event} · {area} til {until} · {office}",
            office = compact(&alert.office, 12)
        ))
    } else {
        fit_mesh(&format!(
            "{prefix} {sev} {event} · {area} til {until} · {link}"
        ))
    }
}

/// Compact summary plus description / instruction frames (vehicle, persons, what to do).
///
/// Caps at 6 frames total so a reconnect or curious operator cannot flood the channel.
pub fn format_nws_detail(alert: &NwsAlert, short_url: Option<&str>) -> Vec<String> {
    let prefix = alert_prefix(alert);
    let mut frames = vec![format_nws_alert(alert, short_url)];

    if !alert.headline.is_empty()
        && !alert
            .event
            .eq_ignore_ascii_case(alert.headline.split('.').next().unwrap_or(""))
    {
        frames.extend(mesh_frames(&format!("{prefix}+"), &alert.headline, 1));
    }

    if !alert.description.is_empty() {
        frames.extend(mesh_frames(&format!("{prefix}+"), &alert.description, 4));
    }

    if !alert.instruction.is_empty() {
        frames.extend(mesh_frames(
            &format!("{prefix}+"),
            &format!("DO: {}", alert.instruction),
            2,
        ));
    }

    if frames.len() == 1 {
        frames.push(fit_mesh(&format!(
            "{prefix}+: no extra detail in CAP — see agency feed"
        )));
    }

    frames.truncate(6);
    frames
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
            event_code: String::new(),
            description: String::new(),
            instruction: String::new(),
            headline: String::new(),
        };
        let line = format_nws_alert(&alert, None);
        assert!(line.len() <= 160);
        assert!(line.contains("NWS MOD"));
        assert!(line.contains("21:00"));
    }

    #[test]
    fn amber_detail_chunks_vehicle_text() {
        let alert = NwsAlert {
            id: "urn:oid:amber".into(),
            event: "Child Abduction Emergency".into(),
            severity: "Severe".into(),
            area: "San Bernardino County".into(),
            ends: Some("2026-11-01T21:00:00-07:00".into()),
            office: "CA Highway Patrol".into(),
            url: String::new(),
            event_code: "CAE".into(),
            headline: "AMBER Alert in San Bernardino County".into(),
            description: "Suspect vehicle: blue 2018 Honda Civic CA plate 7ABC123. \
                 Missing child: Jane Doe, age 7, brown hair, red jacket. \
                 Last seen near I-15 and Main St Hesperia around 10:15 AM. \
                 Suspect: adult male, approx 35, dark hoodie."
                .into(),
            instruction: "If located call 911. Do not approach.".into(),
        };
        let frames = format_nws_detail(&alert, None);
        assert!(frames.len() >= 3, "{frames:?}");
        assert!(frames.iter().all(|f| f.len() <= 160), "{frames:?}");
        assert!(frames[0].starts_with("AMBER "));
        let joined = frames.join(" ");
        assert!(joined.contains("Honda") || joined.contains("Civic") || joined.contains("7ABC"));
        assert!(joined.contains("911") || joined.contains("DO:"));
    }
}
