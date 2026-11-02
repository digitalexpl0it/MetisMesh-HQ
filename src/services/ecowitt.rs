//! Local Ecowitt / Fine Offset ingest (D1d).
//!
//! - Push: custom-server HTTP listener (form-urlencoded)
//! - Pull: poll `http://{host}/get_livedata_info`
//!
//! No cloud. Ambient Weather cloud APIs are intentionally unsupported.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::db::Db;
use crate::services::station::{
    age_label, publish_station_reading, StationReading, StationSource,
};

const DEFAULT_PUSH_PORT: u16 = 4199;
const DEFAULT_PUSH_PATH: &str = "/data/report";
const DEFAULT_PULL_INTERVAL: i64 = 30;

#[derive(Debug, Clone, Default)]
struct PushStatus {
    listening: bool,
    last_rx: Option<Instant>,
    last_error: String,
}

#[derive(Debug, Clone, Default)]
struct PullStatus {
    last_ok: Option<Instant>,
    last_error: String,
}

fn push_status() -> &'static Mutex<PushStatus> {
    static S: OnceLock<Mutex<PushStatus>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(PushStatus::default()))
}

fn pull_status() -> &'static Mutex<PullStatus> {
    static S: OnceLock<Mutex<PullStatus>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(PullStatus::default()))
}

fn push_gen() -> &'static Mutex<u64> {
    static G: OnceLock<Mutex<u64>> = OnceLock::new();
    G.get_or_init(|| Mutex::new(0))
}

pub fn bump_ecowitt_config() {
    if let Ok(mut g) = push_gen().lock() {
        *g = g.wrapping_add(1);
    }
}

fn setting(db: &Db, key: &str) -> String {
    db.setting(key)
        .ok()
        .flatten()
        .unwrap_or_default()
        .trim()
        .to_string()
}

#[derive(Debug, Clone)]
pub struct EcowittPushConfig {
    pub enabled: bool,
    pub port: u16,
    pub path: String,
    pub passkey: String,
}

#[derive(Debug, Clone)]
pub struct EcowittPullConfig {
    pub enabled: bool,
    pub host: String,
    pub interval_secs: u64,
}

pub fn load_ecowitt_push_config(db: &Db) -> EcowittPushConfig {
    let path_raw = setting(db, "svc_weather_ecowitt_push_path");
    let path = if path_raw.is_empty() {
        DEFAULT_PUSH_PATH.to_string()
    } else if path_raw.starts_with('/') {
        path_raw
    } else {
        format!("/{path_raw}")
    };
    EcowittPushConfig {
        enabled: crate::services::setting_bool(db, "svc_weather_ecowitt_push_enabled", false),
        port: setting(db, "svc_weather_ecowitt_push_port")
            .parse()
            .unwrap_or(DEFAULT_PUSH_PORT),
        path,
        passkey: setting(db, "svc_weather_ecowitt_passkey"),
    }
}

pub fn load_ecowitt_pull_config(db: &Db) -> EcowittPullConfig {
    EcowittPullConfig {
        enabled: crate::services::setting_bool(db, "svc_weather_ecowitt_pull_enabled", false),
        host: setting(db, "svc_weather_ecowitt_pull_host"),
        interval_secs: crate::services::setting_i64(
            db,
            "svc_weather_ecowitt_pull_interval_secs",
            DEFAULT_PULL_INTERVAL,
        )
        .clamp(5, 300) as u64,
    }
}

pub fn ecowitt_push_status_line(db: &Db) -> String {
    let cfg = load_ecowitt_push_config(db);
    if !cfg.enabled {
        return "Ecowitt push off".into();
    }
    let st = push_status().lock().ok();
    let listening = st.as_ref().map(|s| s.listening).unwrap_or(false);
    let err = st
        .as_ref()
        .map(|s| s.last_error.clone())
        .unwrap_or_default();
    let last = st.as_ref().and_then(|s| s.last_rx);
    if let Some(t) = last {
        let age = age_label(t.elapsed().as_secs() as i64);
        if listening {
            format!("Listening :{} · last {age}", cfg.port)
        } else {
            format!("Last RX {age} · restarting")
        }
    } else if listening {
        format!("Listening :{}{} · waiting", cfg.port, cfg.path)
    } else if !err.is_empty() {
        format!("Push error · {err}")
    } else {
        format!("Push starting on :{}", cfg.port)
    }
}

pub fn ecowitt_pull_status_line(db: &Db) -> String {
    let cfg = load_ecowitt_pull_config(db);
    if !cfg.enabled {
        return "Ecowitt pull off".into();
    }
    if cfg.host.is_empty() {
        return "Pull: set gateway IP".into();
    }
    let st = pull_status().lock().ok();
    let err = st
        .as_ref()
        .map(|s| s.last_error.clone())
        .unwrap_or_default();
    let last = st.as_ref().and_then(|s| s.last_ok);
    if let Some(t) = last {
        format!("Pull OK · {}", age_label(t.elapsed().as_secs() as i64))
    } else if !err.is_empty() {
        format!("Pull error · {err}")
    } else {
        format!("Pulling {}", cfg.host)
    }
}

fn parse_f64(raw: &str) -> Option<f64> {
    let s = raw.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("none") {
        return None;
    }
    let cleaned: String = s
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-' || *c == '+')
        .collect();
    cleaned.parse().ok()
}

fn form_map(body: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for pair in body.split('&') {
        let mut it = pair.splitn(2, '=');
        let Some(k) = it.next() else { continue };
        let v = it.next().unwrap_or("");
        let key = urlencoding_decode(k);
        let val = urlencoding_decode(v);
        if !key.is_empty() {
            map.insert(key.to_ascii_lowercase(), val);
        }
    }
    map
}

fn urlencoding_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let h = |c: u8| -> Option<u8> {
                    match c {
                        b'0'..=b'9' => Some(c - b'0'),
                        b'a'..=b'f' => Some(c - b'a' + 10),
                        b'A'..=b'F' => Some(c - b'A' + 10),
                        _ => None,
                    }
                };
                if let (Some(a), Some(b)) = (h(bytes[i + 1]), h(bytes[i + 2])) {
                    out.push((a << 4) | b);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Parse Ecowitt customized upload form body into a station reading.
pub fn reading_from_ecowitt_form(body: &str) -> Result<StationReading, String> {
    let map = form_map(body);
    if map.is_empty() {
        return Err("empty_form".into());
    }
    let tempf = map.get("tempf").and_then(|s| parse_f64(s));
    if tempf.is_none() && map.get("humidity").and_then(|s| parse_f64(s)).is_none() {
        // Allow indoor-only? Prefer outdoor for mesh — require tempf or humidity.
        if map.get("tempinf").and_then(|s| parse_f64(s)).is_none() {
            return Err("missing_obs".into());
        }
    }
    let inhg = map
        .get("baromrelin")
        .or_else(|| map.get("baromabsin"))
        .and_then(|s| parse_f64(s));
    let mut reading = StationReading {
        received_at: Instant::now(),
        source: StationSource::EcowittPush,
        outdoor_temp_f: tempf,
        outdoor_temp_c: tempf.map(|f| (f - 32.0) * 5.0 / 9.0),
        humidity_pct: map.get("humidity").and_then(|s| parse_f64(s)),
        wind_mph: map.get("windspeedmph").and_then(|s| parse_f64(s)),
        wind_gust_mph: map.get("windgustmph").and_then(|s| parse_f64(s)),
        wind_dir_deg: map.get("winddir").and_then(|s| parse_f64(s)),
        pressure_inhg: inhg,
        pressure_hpa: inhg.map(|v| v * 33.8639),
        rain_day_in: map.get("dailyrainin").and_then(|s| parse_f64(s)),
        rain_rate_in: map
            .get("rainratein")
            .or_else(|| map.get("hourlyrainin"))
            .and_then(|s| parse_f64(s)),
        uv_index: map.get("uv").and_then(|s| parse_f64(s)),
        solar_wm2: map.get("solarradiation").and_then(|s| parse_f64(s)),
        indoor_temp_f: map.get("tempinf").and_then(|s| parse_f64(s)),
        indoor_humidity_pct: map.get("humidityin").and_then(|s| parse_f64(s)),
        device: map
            .get("model")
            .or_else(|| map.get("stationtype"))
            .cloned()
            .unwrap_or_default(),
        ..StationReading::default()
    };
    if reading.outdoor_temp_f.is_none() && reading.indoor_temp_f.is_some() {
        // Fall back so mesh still has a temp when outdoor sensor is quiet.
        reading.outdoor_temp_f = reading.indoor_temp_f;
        reading.outdoor_temp_c = reading.outdoor_temp_f.map(|f| (f - 32.0) * 5.0 / 9.0);
        if reading.humidity_pct.is_none() {
            reading.humidity_pct = reading.indoor_humidity_pct;
        }
    }
    Ok(reading)
}

fn item_id_num(id: &str) -> Option<u32> {
    let s = id.trim();
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        return u32::from_str_radix(hex, 16).ok();
    }
    s.parse().ok()
}

fn apply_livedata_item(reading: &mut StationReading, id: &str, val: &str, unit: Option<&str>) {
    let Some(code) = item_id_num(id) else {
        return;
    };
    let Some(n) = parse_f64(val) else {
        return;
    };
    let unit = unit.unwrap_or("").to_ascii_lowercase();
    let as_f = |c: f64| c * 9.0 / 5.0 + 32.0;
    let as_c = |f: f64| (f - 32.0) * 5.0 / 9.0;
    match code {
        0x01 => {
            // indoor temp — API docs say °C but unit field may be F
            if unit.contains('f') {
                reading.indoor_temp_f = Some(n);
            } else {
                reading.indoor_temp_f = Some(as_f(n));
            }
        }
        0x02 => {
            if unit.contains('f') {
                reading.outdoor_temp_f = Some(n);
                reading.outdoor_temp_c = Some(as_c(n));
            } else {
                reading.outdoor_temp_c = Some(n);
                reading.outdoor_temp_f = Some(as_f(n));
            }
        }
        0x03 | 0x04 | 0x05 => {
            // dew / chill / heat — treat as feels-like when outdoor
            let f = if unit.contains('f') { n } else { as_f(n) };
            reading.feels_like_f = Some(f);
        }
        0x06 => reading.indoor_humidity_pct = Some(n),
        0x07 => reading.humidity_pct = Some(n),
        0x08 | 0x09 => {
            if unit.contains("inhg") || unit.contains("in") {
                reading.pressure_inhg = Some(n);
                reading.pressure_hpa = Some(n * 33.8639);
            } else {
                reading.pressure_hpa = Some(n);
                reading.pressure_inhg = Some(n / 33.8639);
            }
        }
        0x0A => reading.wind_dir_deg = Some(n),
        0x0B => {
            // m/s or mph depending on unit
            let mph = if unit.contains("mph") {
                n
            } else if unit.contains("km") {
                n / 1.60934
            } else {
                n * 2.23694 // m/s
            };
            reading.wind_mph = Some(mph);
        }
        0x0C => {
            let mph = if unit.contains("mph") {
                n
            } else if unit.contains("km") {
                n / 1.60934
            } else {
                n * 2.23694
            };
            reading.wind_gust_mph = Some(mph);
        }
        0x0E => {
            let inches = if unit.contains("mm") {
                n / 25.4
            } else {
                n
            };
            reading.rain_rate_in = Some(inches);
        }
        0x10 => {
            let inches = if unit.contains("mm") {
                n / 25.4
            } else {
                n
            };
            reading.rain_day_in = Some(inches);
        }
        0x15 => reading.solar_wm2 = Some(n),
        0x16 | 0x17 => reading.uv_index = Some(n),
        _ => {}
    }
}

/// Map Ecowitt `get_livedata_info` JSON into a station reading.
pub fn reading_from_livedata_json(raw: &str) -> Result<StationReading, String> {
    let value: Value = serde_json::from_str(raw).map_err(|e| format!("json:{e}"))?;
    let mut reading = StationReading {
        received_at: Instant::now(),
        source: StationSource::EcowittPull,
        ..StationReading::default()
    };

    if let Some(list) = value.get("common_list").and_then(|v| v.as_array()) {
        for item in list {
            let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let val = item
                .get("val")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| {
                    item.get("val")
                        .and_then(|v| v.as_f64())
                        .map(|n| n.to_string())
                })
                .unwrap_or_default();
            let unit = item.get("unit").and_then(|v| v.as_str());
            apply_livedata_item(&mut reading, id, &val, unit);
        }
    }

    if let Some(wh25) = value
        .get("wh25")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
    {
        if reading.indoor_temp_f.is_none() {
            if let Some(t) = wh25.get("intemp").and_then(|v| v.as_str()).and_then(parse_f64) {
                let unit = wh25.get("unit").and_then(|v| v.as_str()).unwrap_or("F");
                reading.indoor_temp_f = if unit.eq_ignore_ascii_case("C") {
                    Some(t * 9.0 / 5.0 + 32.0)
                } else {
                    Some(t)
                };
            }
        }
        if reading.indoor_humidity_pct.is_none() {
            reading.indoor_humidity_pct = wh25
                .get("inhumi")
                .and_then(|v| v.as_str())
                .and_then(parse_f64);
        }
        if reading.pressure_inhg.is_none() {
            if let Some(rel) = wh25.get("rel").and_then(|v| v.as_str()).and_then(parse_f64) {
                reading.pressure_inhg = Some(rel);
                reading.pressure_hpa = Some(rel * 33.8639);
            }
        }
    }

    if let Some(rain) = value.get("rain").and_then(|v| v.as_array()) {
        for item in rain {
            let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let val = item.get("val").and_then(|v| v.as_str()).unwrap_or("");
            apply_livedata_item(&mut reading, id, val, Some("in"));
        }
    }

    if reading.outdoor_temp_f.is_none()
        && reading.outdoor_temp_c.is_none()
        && reading.humidity_pct.is_none()
        && reading.indoor_temp_f.is_none()
    {
        return Err("empty_livedata".into());
    }
    Ok(reading)
}

fn passkey_ok(cfg: &EcowittPushConfig, map: &HashMap<String, String>) -> bool {
    if cfg.passkey.is_empty() {
        return true;
    }
    map.get("passkey")
        .map(|p| p == &cfg.passkey)
        .unwrap_or(false)
}

fn http_response(code: u16, body: &str) -> Vec<u8> {
    let reason = match code {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        _ => "Error",
    };
    format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

async fn handle_client(mut stream: TcpStream, cfg: EcowittPushConfig) {
    let mut buf = vec![0u8; 64 * 1024];
    let n = match tokio::time::timeout(Duration::from_secs(10), stream.read(&mut buf)).await {
        Ok(Ok(n)) => n,
        _ => return,
    };
    if n == 0 {
        return;
    }
    let raw = String::from_utf8_lossy(&buf[..n]);
    let Some(header_end) = raw.find("\r\n\r\n") else {
        let _ = stream.write_all(&http_response(400, "bad request")).await;
        return;
    };
    let head = &raw[..header_end];
    let body_start = header_end + 4;
    let mut body = raw[body_start..].to_string();

    let first = head.lines().next().unwrap_or("");
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path_q = parts.next().unwrap_or("");
    let path = path_q.split('?').next().unwrap_or(path_q);
    let query = path_q.split('?').nth(1).unwrap_or("");

    let want = cfg.path.trim_end_matches('/');
    let got = path.trim_end_matches('/');
    // Ecowitt app may omit leading slash or add extras
    let path_ok = got.eq_ignore_ascii_case(want)
        || got.eq_ignore_ascii_case(want.trim_start_matches('/'))
        || format!("/{got}").eq_ignore_ascii_case(want);

    if !path_ok {
        let _ = stream.write_all(&http_response(404, "not found")).await;
        return;
    }

    if method.eq_ignore_ascii_case("GET") && body.trim().is_empty() && !query.is_empty() {
        body = query.to_string();
    }

    // Content-Length may mean more body pending — for typical Ecowitt sizes first read is enough.
    let map = form_map(&body);
    if !passkey_ok(&cfg, &map) {
        let _ = stream.write_all(&http_response(401, "bad passkey")).await;
        return;
    }

    match reading_from_ecowitt_form(&body) {
        Ok(mut reading) => {
            reading.source = StationSource::EcowittPush;
            publish_station_reading(reading);
            if let Ok(mut s) = push_status().lock() {
                s.last_rx = Some(Instant::now());
                s.last_error.clear();
            }
            let _ = stream.write_all(&http_response(200, "ok")).await;
        }
        Err(err) => {
            hq_log!(debug, "ecowitt push: {err}");
            let _ = stream.write_all(&http_response(400, &err)).await;
        }
    }
}

async fn push_session(db: &Db, cfg: &EcowittPushConfig) -> Result<(), String> {
    let addr = format!("0.0.0.0:{}", cfg.port);
    let listener = TcpListener::bind(&addr)
        .await
        .map_err(|e| format!("bind {addr}: {e}"))?;
    if let Ok(mut s) = push_status().lock() {
        s.listening = true;
        s.last_error.clear();
    }
    hq_log!(
        info,
        "ecowitt push: listening {} path={}",
        addr,
        cfg.path
    );

    let start_gen = push_gen().lock().map(|g| *g).unwrap_or(0);
    loop {
        let gen_now = push_gen().lock().map(|g| *g).unwrap_or(0);
        if gen_now != start_gen {
            return Ok(());
        }
        let live = load_ecowitt_push_config(db);
        if !live.enabled || live.port != cfg.port || live.path != cfg.path {
            return Ok(());
        }

        match tokio::time::timeout(Duration::from_secs(2), listener.accept()).await {
            Ok(Ok((stream, _))) => {
                let cfg_c = live.clone();
                tokio::spawn(async move {
                    handle_client(stream, cfg_c).await;
                });
            }
            Ok(Err(err)) => return Err(err.to_string()),
            Err(_) => {}
        }
    }
}

/// Long-lived Ecowitt custom-server listener.
pub async fn ecowitt_push_loop(db: std::sync::Arc<Db>) {
    loop {
        let cfg = load_ecowitt_push_config(&db);
        if !cfg.enabled {
            if let Ok(mut s) = push_status().lock() {
                s.listening = false;
            }
            tokio::time::sleep(Duration::from_secs(3)).await;
            continue;
        }
        match push_session(&db, &cfg).await {
            Ok(()) => {}
            Err(err) => {
                if let Ok(mut s) = push_status().lock() {
                    s.listening = false;
                    s.last_error = err.clone();
                }
                hq_log!(warn, "ecowitt push: {err}");
            }
        }
        if let Ok(mut s) = push_status().lock() {
            s.listening = false;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

fn pull_once(cfg: &EcowittPullConfig) -> Result<(), String> {
    let host = cfg.host.trim().trim_end_matches('/');
    let url = if host.contains("://") {
        format!("{host}/get_livedata_info?")
    } else {
        format!("http://{host}/get_livedata_info?")
    };
    let body = crate::services::http_get(&url)
        .map_err(|e| e.to_string())?
        .into_string()
        .map_err(|e| e.to_string())?;
    let mut reading = reading_from_livedata_json(&body)?;
    reading.source = StationSource::EcowittPull;
    reading.device = host.to_string();
    publish_station_reading(reading);
    if let Ok(mut s) = pull_status().lock() {
        s.last_ok = Some(Instant::now());
        s.last_error.clear();
    }
    Ok(())
}

/// Long-lived poll of gateway `get_livedata_info`.
pub async fn ecowitt_pull_loop(db: std::sync::Arc<Db>) {
    loop {
        let cfg = load_ecowitt_pull_config(&db);
        if !cfg.enabled || cfg.host.is_empty() {
            tokio::time::sleep(Duration::from_secs(3)).await;
            continue;
        }
        let interval = Duration::from_secs(cfg.interval_secs);
        let result = tokio::task::spawn_blocking({
            let cfg = cfg.clone();
            move || pull_once(&cfg)
        })
        .await;
        match result {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                if let Ok(mut s) = pull_status().lock() {
                    s.last_error = err.clone();
                }
                hq_log!(debug, "ecowitt pull: {err}");
            }
            Err(err) => {
                hq_log!(warn, "ecowitt pull join: {err}");
            }
        }
        tokio::time::sleep(interval).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ecowitt_push_form() {
        let body = "PASSKEY=ABC&stationtype=GW2000A_V2.2.3&tempf=76.28&humidity=34&winddir=224&windspeedmph=2.5&baromrelin=30.41&dailyrainin=0.12&uv=3&tempinf=77.00&humidityin=40&model=GW2000A";
        let r = reading_from_ecowitt_form(body).unwrap();
        assert!((r.outdoor_temp_f.unwrap() - 76.28).abs() < 0.01);
        assert_eq!(r.humidity_pct, Some(34.0));
        assert_eq!(r.wind_mph, Some(2.5));
        assert_eq!(r.device, "GW2000A");
        assert!(r.pressure_hpa.is_some());
    }

    #[test]
    fn parses_livedata_common_list() {
        let raw = r#"{
            "common_list":[
                {"id":"0x02","val":"79.2","unit":"F"},
                {"id":"0x07","val":"65%"},
                {"id":"0x0B","val":"1.2","unit":"m/s"},
                {"id":"0x0A","val":"180"},
                {"id":"0x09","val":"1015.2","unit":"hpa"}
            ],
            "wh25":[{"intemp":"72.0","unit":"F","inhumi":"50%","rel":"30.01 inHg"}]
        }"#;
        let r = reading_from_livedata_json(raw).unwrap();
        assert!((r.outdoor_temp_f.unwrap() - 79.2).abs() < 0.01);
        assert_eq!(r.humidity_pct, Some(65.0));
        assert!(r.wind_mph.unwrap() > 2.0);
        assert_eq!(r.indoor_temp_f, Some(72.0));
    }

    #[test]
    fn reject_empty_form() {
        assert!(reading_from_ecowitt_form("").is_err());
    }
}
