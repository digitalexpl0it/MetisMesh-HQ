//! HQ-owned info services (weather, NWS alerts, earthquakes).
//!
//! Runs beside the radio task — not a second MeshCore companion. Mesh replies
//! are enrolled-only, rate-limited, and capped at 160 bytes.

mod ecowitt;
pub mod ha_mqtt;
pub mod local_api;
pub mod map_upload;
mod mqtt_client;
mod mqtt_weather;
mod nws;
mod poller;
mod quake;
mod station;
mod weather;

pub use ecowitt::{bump_ecowitt_config, ecowitt_pull_status_line, ecowitt_push_status_line};
pub use ha_mqtt::{
    bump_ha_mqtt_config, clamp_ha_mqtt_interval, clear_ha_mqtt_password,
    ha_mqtt_password_present, ha_mqtt_status_line, save_ha_mqtt_password,
};
pub use local_api::{
    bump_local_api_config, clamp_rate_limit as clamp_local_api_rate_limit,
    clear_token as clear_local_api_token, ensure_token as ensure_local_api_token,
    generate_token as generate_local_api_token, is_loopback_bind,
    load_token as load_local_api_token, local_api_status_line,
    normalize_prefix as normalize_local_api_prefix, save_token as save_local_api_token,
    token_file_present as local_api_token_present,
};
pub use map_upload::{
    authorize_map_upload, map_upload_status_line, revoke_map_upload_auth,
};
pub use mqtt_weather::{
    bump_mqtt_weather_config, clear_mqtt_weather_password, default_json_template,
    mqtt_weather_status_line, password_file_present, save_mqtt_weather_password,
};
pub use station::{
    age_label, any_station_source_enabled, current_station_reading, outdoor_temp_display,
    station_is_fresh, station_label, station_stale_secs, try_station_wx_line, StationReading,
};
pub use nws::{
    fetch_active_alerts, fetch_emergency_alerts, format_nws_alert, format_nws_detail,
    is_amber_event, shorten_url, AMBER_CODES, EMERG_CODES,
};
pub use poller::spawn_poller;
pub use quake::{fetch_quakes, format_quake};
pub use weather::{fetch_weather_detail, fetch_weather_line, short_wx_label, WeatherQuery};

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::db::Db;
use crate::signup::{fit_mesh, Enrollment};

const BOT_GAP: Duration = Duration::from_secs(45);

/// Shared outbound UA. Do **not** put "Metis" in this string — NWS (Akamai) and
/// USGS (CloudFront) both 403 User-Agents that contain that substring.
pub const HTTP_USER_AGENT: &str = "LocalHQ/0.1 (desktop emergency console)";

pub fn http_get(url: &str) -> Result<ureq::Response, String> {
    ureq::get(url)
        .set("User-Agent", HTTP_USER_AGENT)
        .set("Accept", "application/geo+json, application/json;q=0.9, */*;q=0.8")
        .timeout(Duration::from_secs(15))
        .call()
        .map_err(|e| e.to_string())
}

fn last_bot() -> &'static Mutex<HashMap<String, Instant>> {
    static LAST_BOT: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    LAST_BOT.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BotCommand {
    /// Optional ZIP / city after the command (`#weather 92345`).
    /// `detail` is `#weather+` / `#wx more` — multi-frame reply.
    Wx {
        place: Option<String>,
        detail: bool,
    },
    Quake,
    Amber {
        detail: bool,
    },
    /// Broader civil / Amber / evacuation-style emergencies.
    Alert {
        detail: bool,
    },
    Help,
}

/// Channel commands on METIS-INFO.
/// Leading `#` preferred; bare keywords still parse for stubborn clients.
/// Trailing `+` or a first arg of `more`/`ext`/`detail`/`full` requests extended frames.
pub fn parse_bot_command(text: &str) -> Option<BotCommand> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let bare = trimmed.strip_prefix('#').unwrap_or(trimmed).trim();
    let mut parts = bare.split_whitespace();
    let raw_head = parts.next()?.to_ascii_lowercase();
    let (head, plus) = if let Some(h) = raw_head.strip_suffix('+') {
        (h.to_string(), true)
    } else {
        (raw_head, false)
    };
    let mut rest_parts: Vec<&str> = parts.collect();
    let mut detail = plus;
    if let Some(first) = rest_parts.first().copied() {
        let f = first.to_ascii_lowercase();
        if matches!(f.as_str(), "more" | "ext" | "detail" | "full" | "+") {
            detail = true;
            rest_parts.remove(0);
        }
    }
    let rest = rest_parts.join(" ");
    let place = {
        let t = rest.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.chars().take(48).collect())
        }
    };
    match head.as_str() {
        "wx" | "weather" => Some(BotCommand::Wx { place, detail }),
        "quake" | "earthquake" | "eq" => Some(BotCommand::Quake),
        "amber" | "amberalert" => Some(BotCommand::Amber { detail }),
        "alert" | "alerts" | "emerg" | "emergency" => Some(BotCommand::Alert { detail }),
        "help" | "cmds" | "commands" => Some(BotCommand::Help),
        _ => None,
    }
}

pub fn help_line() -> String {
    fit_mesh(
        "cmds: #weather[+] [zip] #quake #amber[+] #alert[+] #help · + = more detail",
    )
}

/// Two-letter state for Amber/emergency CAP queries.
pub fn emerg_state(db: &Db) -> String {
    db.setting("svc_emerg_state")
        .ok()
        .flatten()
        .map(|s| s.trim().to_ascii_uppercase())
        .filter(|s| s.len() == 2 && s.chars().all(|c| c.is_ascii_alphabetic()))
        .unwrap_or_else(|| "CA".into())
}

/// Slot for the bot-home channel (default METIS-INFO), or None if missing.
pub fn metis_info_slot(db: &Db) -> Option<u8> {
    let info = db.info_channel_name().ok()?;
    db.list_channels()
        .ok()?
        .into_iter()
        .find(|c| c.name.eq_ignore_ascii_case(&info))
        .map(|c| c.slot.clamp(0, 255) as u8)
}

pub fn is_metis_info_slot(db: &Db, slot: u8) -> bool {
    metis_info_slot(db) == Some(slot)
}

pub fn bot_rate_ok(pubkey: &str) -> bool {
    let mut map = last_bot().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(last) = map.get(pubkey) {
        if last.elapsed() < BOT_GAP {
            return false;
        }
    }
    map.insert(pubkey.to_string(), Instant::now());
    true
}

pub fn bot_rate_wait_line() -> String {
    fit_mesh("Rate limited. Try again in about a minute.")
}

pub fn setting_bool(db: &Db, key: &str, default: bool) -> bool {
    match db.setting(key).ok().flatten().as_deref() {
        Some("1") | Some("true") | Some("yes") | Some("on") => true,
        Some("0") | Some("false") | Some("no") | Some("off") => false,
        Some(_) => default,
        None => default,
    }
}

pub fn set_setting_bool(db: &Db, key: &str, value: bool) {
    let _ = db.set_setting(key, if value { "1" } else { "0" });
}

pub fn setting_f64(db: &Db, key: &str, default: f64) -> f64 {
    db.setting(key)
        .ok()
        .flatten()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

pub fn setting_i64(db: &Db, key: &str, default: i64) -> i64 {
    db.setting(key)
        .ok()
        .flatten()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Resolve coords for weather: node fix → HQ site settings → companion advert.
pub fn resolve_weather_coords(
    db: &Db,
    node_lat: Option<f64>,
    node_lon: Option<f64>,
    hq_lat: Option<f64>,
    hq_lon: Option<f64>,
) -> Option<(f64, f64, &'static str)> {
    if let (Some(lat), Some(lon)) = (node_lat, node_lon) {
        if lat.abs() > 0.0001 || lon.abs() > 0.0001 {
            return Some((lat, lon, "node"));
        }
    }
    let site_lat = setting_f64(db, "hq_site_lat", f64::NAN);
    let site_lon = setting_f64(db, "hq_site_lon", f64::NAN);
    if site_lat.is_finite() && site_lon.is_finite() {
        return Some((site_lat, site_lon, "HQ site"));
    }
    if let (Some(lat), Some(lon)) = (hq_lat, hq_lon) {
        if lat.abs() > 0.0001 || lon.abs() > 0.0001 {
            return Some((lat, lon, "HQ"));
        }
    }
    None
}

pub fn weather_unit_f(db: &Db) -> bool {
    match db.setting("svc_weather_unit").ok().flatten().as_deref() {
        Some("C") | Some("c") | Some("celsius") => false,
        _ => true,
    }
}

/// NWS / quake / amber / emergency relays always use the bot-home channel.
pub fn alerts_channel_slot(db: &Db) -> u8 {
    metis_info_slot(db).unwrap_or_else(|| setting_i64(db, "svc_alerts_slot", 1).clamp(0, 255) as u8)
}

pub fn quake_bbox(db: &Db) -> (f64, f64, f64, f64) {
    // Default: California (meshcore-bot defaults).
    (
        setting_f64(db, "svc_quake_min_lat", 32.5),
        setting_f64(db, "svc_quake_max_lat", 42.0),
        setting_f64(db, "svc_quake_min_lon", -124.5),
        setting_f64(db, "svc_quake_max_lon", -114.0),
    )
}

pub fn quake_min_magnitude(db: &Db) -> f64 {
    setting_f64(db, "svc_quake_min_mag", 3.0)
}

/// ZIP / postal list from Settings (`svc_weather_zips`), comma or newline separated.
pub fn weather_zip_list(db: &Db) -> Vec<String> {
    let raw = db
        .setting("svc_weather_zips")
        .ok()
        .flatten()
        .unwrap_or_default();
    let mut out = Vec::new();
    for part in raw.split(|c: char| c == ',' || c == ';' || c == '\n' || c == ' ') {
        let zip: String = part.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        if zip.len() >= 3 && zip.len() <= 10 && !out.iter().any(|z: &String| z == &zip) {
            out.push(zip);
        }
    }
    out
}

pub fn weather_interval_secs(db: &Db) -> i64 {
    let mins = setting_i64(db, "svc_weather_interval_mins", 60).clamp(15, 24 * 60);
    mins * 60
}

/// Cache geocoded ZIP centers so hourly posts do not hammer Nominatim.
pub fn cached_zip_coords(db: &Db, zip: &str) -> Option<(f64, f64, String)> {
    let raw = db.setting("svc_weather_zip_coords").ok().flatten()?;
    let map: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&raw).ok()?;
    let entry = map.get(zip)?;
    let lat = entry.get("lat")?.as_f64()?;
    let lon = entry.get("lon")?.as_f64()?;
    let label = entry
        .get("label")
        .and_then(|v| v.as_str())
        .unwrap_or(zip)
        .to_string();
    Some((lat, lon, label))
}

pub fn remember_zip_coords(db: &Db, zip: &str, lat: f64, lon: f64, label: &str) {
    let mut map: serde_json::Map<String, serde_json::Value> = db
        .setting("svc_weather_zip_coords")
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    map.insert(
        zip.to_string(),
        serde_json::json!({ "lat": lat, "lon": lon, "label": label }),
    );
    if let Ok(raw) = serde_json::to_string(&map) {
        let _ = db.set_setting("svc_weather_zip_coords", &raw);
    }
}

pub fn is_enrolled(enrollment: Enrollment) -> bool {
    matches!(enrollment, Enrollment::Enrolled | Enrollment::Hq)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bot_commands() {
        assert_eq!(
            parse_bot_command("#wx"),
            Some(BotCommand::Wx {
                place: None,
                detail: false
            })
        );
        assert_eq!(
            parse_bot_command("#weather 92345"),
            Some(BotCommand::Wx {
                place: Some("92345".into()),
                detail: false
            })
        );
        assert_eq!(
            parse_bot_command("#weather+ 92345"),
            Some(BotCommand::Wx {
                place: Some("92345".into()),
                detail: true
            })
        );
        assert_eq!(
            parse_bot_command("#wx more Hesperia"),
            Some(BotCommand::Wx {
                place: Some("Hesperia".into()),
                detail: true
            })
        );
        assert_eq!(
            parse_bot_command("weather Hesperia, CA"),
            Some(BotCommand::Wx {
                place: Some("Hesperia, CA".into()),
                detail: false
            })
        );
        assert_eq!(parse_bot_command("#quake"), Some(BotCommand::Quake));
        assert_eq!(
            parse_bot_command("#amber"),
            Some(BotCommand::Amber { detail: false })
        );
        assert_eq!(
            parse_bot_command("#amber+"),
            Some(BotCommand::Amber { detail: true })
        );
        assert_eq!(
            parse_bot_command("#alert more"),
            Some(BotCommand::Alert { detail: true })
        );
        assert_eq!(parse_bot_command("help"), Some(BotCommand::Help));
        assert_eq!(parse_bot_command("signup"), None);
        assert_eq!(parse_bot_command("hello"), None);
    }

    #[test]
    fn help_fits_mesh() {
        assert!(help_line().len() <= 160);
    }
}
