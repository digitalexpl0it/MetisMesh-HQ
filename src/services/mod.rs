//! HQ-owned info services (weather, NWS alerts, earthquakes).
//!
//! Runs beside the radio task — not a second MeshCore companion. Mesh replies
//! are enrolled-only, rate-limited, and capped at 160 bytes.

mod nws;
mod poller;
mod quake;
mod weather;

pub use nws::{fetch_active_alerts, format_nws_alert, shorten_url};
pub use poller::spawn_poller;
pub use quake::{fetch_quakes, format_quake};
pub use weather::{fetch_weather_line, WeatherQuery};

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
    Wx { place: Option<String> },
    Quake,
    Help,
}

/// Channel commands on METIS-INFO: `#weather [zip]`, `#quake`, `#help`.
/// Leading `#` preferred; bare keywords still parse for stubborn clients.
pub fn parse_bot_command(text: &str) -> Option<BotCommand> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let bare = trimmed.strip_prefix('#').unwrap_or(trimmed).trim();
    let mut parts = bare.split_whitespace();
    let head = parts.next()?.to_ascii_lowercase();
    let rest = parts.collect::<Vec<_>>().join(" ");
    let place = {
        let t = rest.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.chars().take(48).collect())
        }
    };
    match head.as_str() {
        "wx" | "weather" => Some(BotCommand::Wx { place }),
        "quake" | "earthquake" | "eq" => Some(BotCommand::Quake),
        "help" | "cmds" | "commands" => Some(BotCommand::Help),
        _ => None,
    }
}

pub fn help_line() -> String {
    fit_mesh("METIS-INFO cmds: #weather [zip] #quake #help · enrolled · Open-Meteo/USGS")
}

/// Slot for METIS-INFO, or None if the channel row is missing.
pub fn metis_info_slot(db: &Db) -> Option<u8> {
    db.list_channels()
        .ok()?
        .into_iter()
        .find(|c| c.name.eq_ignore_ascii_case(crate::secrets::CHANNEL_NAME))
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

pub fn alerts_channel_slot(db: &Db) -> u8 {
    let named = db
        .setting("svc_alerts_channel")
        .ok()
        .flatten()
        .unwrap_or_default();
    if !named.is_empty() {
        if let Ok(channels) = db.list_channels() {
            if let Some(ch) = channels
                .iter()
                .find(|c| c.name.eq_ignore_ascii_case(named.trim()))
            {
                return ch.slot.clamp(0, 255) as u8;
            }
        }
    }
    // Prefer METIS-INFO when present.
    if let Ok(channels) = db.list_channels() {
        if let Some(ch) = channels
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case("METIS-INFO"))
        {
            return ch.slot.clamp(0, 255) as u8;
        }
    }
    setting_i64(db, "svc_alerts_slot", 1).clamp(0, 255) as u8
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
            Some(BotCommand::Wx { place: None })
        );
        assert_eq!(
            parse_bot_command("#weather 92345"),
            Some(BotCommand::Wx {
                place: Some("92345".into())
            })
        );
        assert_eq!(
            parse_bot_command("weather Hesperia, CA"),
            Some(BotCommand::Wx {
                place: Some("Hesperia, CA".into())
            })
        );
        assert_eq!(parse_bot_command("#quake"), Some(BotCommand::Quake));
        assert_eq!(parse_bot_command("help"), Some(BotCommand::Help));
        assert_eq!(parse_bot_command("signup"), None);
        assert_eq!(parse_bot_command("hello"), None);
    }

    #[test]
    fn help_fits_mesh() {
        assert!(help_line().len() <= 160);
    }
}
