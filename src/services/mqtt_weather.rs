//! Local MQTT weather station cache (D1c).
//!
//! Subscribes to one topic; bare `#weather` prefers a fresh reading, else Open-Meteo.
//! Ecowitt GW3002 and similar stations work when bridged to MQTT (HA / WeeWX / etc.).
//!
//! Uses a small MQTT 3.1.1 subscriber over Tokio (no extra MQTT crate) so LAN
//! Mosquitto works offline. Optional TLS via tokio-rustls when enabled.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::db::Db;
use crate::services::station::{
    publish_station_reading, station_stale_secs, StationReading, StationSource,
};

const PASS_FILE: &str = "mqtt-weather.pass";
const DEFAULT_PORT: u16 = 1883;

/// Placeholders allowed in `json_template` (meshcore-bot–aligned).
const JSON_TEMPLATE_KEYS: &[&str] = &[
    "time",
    "temperature_f",
    "temperature_c",
    "humidity",
    "device",
    "dewpoint_f",
    "heat_index_f",
    "wind_chill_f",
    "apparent_temperature_f",
    "wet_bulb_temperature_f",
    "pressure_hpa",
    "humidity_percent",
    "absolute_humidity_g_m3",
    "altitude_ft",
    "rain_10min_in",
    "rain_1hour_in",
    "rain_24hour_in",
    "rain_today_in",
    "wind_current_kmh",
    "wind_avg_10min_kmh",
    "wind_avg_1hour_kmh",
    "wind_avg_24hour_kmh",
    "wind_avg_today_kmh",
    "wind_peak_kmh",
    "wind_peak_10min_kmh",
    "wind_peak_1hour_kmh",
    "wind_peak_24hour_kmh",
    "wind_peak_today_kmh",
    "wind_direction_deg",
    "battery_voltage_v",
    "uptime_s",
    "last_update",
    "sensor_name",
    "connection_status",
    "error",
];

#[derive(Debug, Clone)]
pub struct MqttWeatherConfig {
    pub enabled: bool,
    pub broker: String,
    pub port: u16,
    pub tls: bool,
    pub username: String,
    pub password: String,
    pub topic: String,
    pub mode: OutputMode,
    pub template: String,
    pub stale_secs: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    JsonTemplate,
    Passthrough,
}

impl OutputMode {
    pub fn parse(raw: &str) -> Self {
        if raw.eq_ignore_ascii_case("passthrough") {
            Self::Passthrough
        } else {
            Self::JsonTemplate
        }
    }
}

#[derive(Debug, Clone, Default)]
struct RuntimeStatus {
    connected: bool,
    last_error: String,
    last_rx: Option<Instant>,
}

fn status() -> &'static Mutex<RuntimeStatus> {
    static STATUS: OnceLock<Mutex<RuntimeStatus>> = OnceLock::new();
    STATUS.get_or_init(|| Mutex::new(RuntimeStatus::default()))
}

fn config_gen() -> &'static Mutex<u64> {
    static GEN: OnceLock<Mutex<u64>> = OnceLock::new();
    GEN.get_or_init(|| Mutex::new(0))
}

/// Bump so the subscriber reloads Settings on the next reconnect tick.
pub fn bump_mqtt_weather_config() {
    if let Ok(mut g) = config_gen().lock() {
        *g = g.wrapping_add(1);
    }
}

pub fn mqtt_weather_pass_path(data_dir: &Path) -> PathBuf {
    data_dir.join(PASS_FILE)
}

pub fn save_mqtt_weather_password(data_dir: &Path, password: &str) -> Result<(), String> {
    fs::create_dir_all(data_dir).map_err(|e| e.to_string())?;
    let path = mqtt_weather_pass_path(data_dir);
    let password = password.trim();
    if password.is_empty() {
        let _ = fs::remove_file(&path);
        bump_mqtt_weather_config();
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut opts = fs::OpenOptions::new();
        opts.create(true).write(true).truncate(true).mode(0o600);
        let mut file = opts.open(&path).map_err(|e| e.to_string())?;
        file.write_all(password.as_bytes())
            .map_err(|e| e.to_string())?;
    }
    #[cfg(not(unix))]
    {
        fs::write(&path, password).map_err(|e| e.to_string())?;
    }
    bump_mqtt_weather_config();
    Ok(())
}

pub fn load_mqtt_weather_password(data_dir: &Path) -> String {
    fs::read_to_string(mqtt_weather_pass_path(data_dir))
        .ok()
        .map(|s| s.trim_end_matches(['\r', '\n']).to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_default()
}

pub fn clear_mqtt_weather_password(data_dir: &Path) {
    let _ = fs::remove_file(mqtt_weather_pass_path(data_dir));
    bump_mqtt_weather_config();
}

pub fn password_file_present(data_dir: &Path) -> bool {
    mqtt_weather_pass_path(data_dir).is_file()
}

fn setting(db: &Db, key: &str) -> String {
    db.setting(key)
        .ok()
        .flatten()
        .unwrap_or_default()
        .trim()
        .to_string()
}

pub fn default_json_template(fahrenheit: bool) -> String {
    if fahrenheit {
        "{temperature_f}F {humidity}%".into()
    } else {
        "{temperature_c}C {humidity}%".into()
    }
}

pub fn load_mqtt_weather_config(db: &Db, data_dir: &Path) -> MqttWeatherConfig {
    let enabled = crate::services::setting_bool(db, "svc_weather_mqtt_enabled", false);
    let broker = setting(db, "svc_weather_mqtt_broker");
    let port = setting(db, "svc_weather_mqtt_port")
        .parse::<u16>()
        .unwrap_or(DEFAULT_PORT);
    let tls = crate::services::setting_bool(db, "svc_weather_mqtt_tls", false);
    let username = setting(db, "svc_weather_mqtt_username");
    let password = load_mqtt_weather_password(data_dir);
    let topic = setting(db, "svc_weather_mqtt_topic");
    let mode = OutputMode::parse(&setting(db, "svc_weather_mqtt_mode"));
    let fahrenheit = crate::services::weather_unit_f(db);
    let template_raw = setting(db, "svc_weather_mqtt_template");
    let template = if template_raw.is_empty() {
        default_json_template(fahrenheit)
    } else {
        template_raw
    };
    let stale_secs = station_stale_secs(db);
    MqttWeatherConfig {
        enabled,
        broker,
        port,
        tls,
        username,
        password,
        topic,
        mode,
        template,
        stale_secs,
    }
}

fn set_status(connected: bool, err: impl Into<String>) {
    if let Ok(mut s) = status().lock() {
        s.connected = connected;
        let e = err.into();
        if !e.is_empty() {
            s.last_error = e;
        } else if connected {
            s.last_error.clear();
        }
    }
}

fn mark_rx() {
    if let Ok(mut s) = status().lock() {
        s.last_rx = Some(Instant::now());
        s.connected = true;
        s.last_error.clear();
    }
}

/// UI status: Connected · 2m ago / Stale · … / Disconnected · …
pub fn mqtt_weather_status_line(db: &Db) -> String {
    let cfg = load_mqtt_weather_config(db, &crate::data_dir());
    if !cfg.enabled {
        return "MQTT station off".into();
    }
    if cfg.broker.is_empty() || cfg.topic.is_empty() {
        return "MQTT: set broker and topic".into();
    }
    let st = status().lock().ok();
    let connected = st.as_ref().map(|s| s.connected).unwrap_or(false);
    let err = st
        .as_ref()
        .map(|s| s.last_error.clone())
        .unwrap_or_default();
    let last_rx = st.as_ref().and_then(|s| s.last_rx);
    let from_cache = crate::services::station::current_station_reading()
        .filter(|r| r.source == StationSource::Mqtt);
    if from_cache.is_some() || last_rx.is_some() {
        let age = from_cache
            .as_ref()
            .map(|s| s.received_at.elapsed().as_secs() as i64)
            .or_else(|| last_rx.map(|t| t.elapsed().as_secs() as i64))
            .unwrap_or(0);
        let age_label = crate::services::station::age_label(age);
        let mqtt_fresh = from_cache
            .as_ref()
            .is_some_and(|s| s.received_at.elapsed().as_secs() as i64 <= cfg.stale_secs);
        if from_cache.is_some() && !mqtt_fresh {
            format!("Stale · last {age_label}")
        } else if connected {
            format!("Connected · {age_label}")
        } else {
            format!("Last reading {age_label} · reconnecting")
        }
    } else if connected {
        "Connected · waiting for data".into()
    } else if !err.is_empty() {
        format!("Disconnected · {err}")
    } else {
        "Disconnected".into()
    }
}

fn sanitize_text(raw: &str, max: usize) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.chars().count() <= max {
        trimmed.to_string()
    } else {
        trimmed.chars().take(max).collect()
    }
}

fn lookup_json_value<'a>(map: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a Value> {
    if let Some(v) = map.get(key) {
        return Some(v);
    }
    let lower = key.to_ascii_lowercase();
    for (k, v) in map {
        if k.eq_ignore_ascii_case(&lower) {
            return Some(v);
        }
    }
    let aliases: &[(&str, &[&str])] = &[
        ("temperature_f", &["temp_f", "tempf", "temperature"]),
        ("temperature_c", &["temp_c", "tempc"]),
        ("humidity", &["humidity_percent", "rh", "relative_humidity"]),
        ("humidity_percent", &["humidity", "rh"]),
        ("pressure_hpa", &["pressure", "baromabsin", "baromrelin"]),
        ("wind_current_kmh", &["wind_speed", "windspeed", "windspeedkmh"]),
        ("wind_direction_deg", &["wind_dir", "winddir"]),
        ("time", &["datetime", "last_update", "timestamp"]),
        ("device", &["sensor_name", "name", "station"]),
    ];
    for (canon, alts) in aliases {
        if key.eq_ignore_ascii_case(canon) {
            for a in *alts {
                if let Some(v) = map.get(*a) {
                    return Some(v);
                }
                for (k, v) in map {
                    if k.eq_ignore_ascii_case(a) {
                        return Some(v);
                    }
                }
            }
        }
    }
    None
}

fn value_as_display(v: &Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::Bool(b) => Some(b.to_string()),
        Value::Number(n) => {
            if let Some(f) = n.as_f64() {
                if f.fract().abs() < 0.05 {
                    Some(format!("{f:.0}"))
                } else {
                    Some(format!("{f:.1}"))
                }
            } else {
                Some(n.to_string())
            }
        }
        Value::String(s) => {
            let s = s.trim();
            if s.is_empty() {
                None
            } else {
                Some(s.to_string())
            }
        }
        Value::Array(_) | Value::Object(_) => None,
    }
}

/// Format a raw MQTT payload into cache body.
pub fn format_mqtt_payload(
    payload: &[u8],
    mode: OutputMode,
    template: &str,
    fahrenheit: bool,
) -> Result<(String, bool), String> {
    let raw = std::str::from_utf8(payload).map_err(|_| "utf8".to_string())?;
    match mode {
        OutputMode::Passthrough => {
            let text = sanitize_text(raw, 500);
            if text.is_empty() {
                return Err("empty".into());
            }
            let full = text.to_ascii_uppercase().starts_with("WX ");
            Ok((text, full))
        }
        OutputMode::JsonTemplate => {
            let value: Value = serde_json::from_str(raw).map_err(|e| format!("json:{e}"))?;
            let obj = value
                .as_object()
                .ok_or_else(|| "json_object".to_string())?;
            let tmpl = if template.trim().is_empty() {
                default_json_template(fahrenheit)
            } else {
                template.to_string()
            };
            let mut out = String::new();
            let mut rest = tmpl.as_str();
            while let Some(start) = rest.find('{') {
                out.push_str(&rest[..start]);
                rest = &rest[start + 1..];
                let Some(end) = rest.find('}') else {
                    out.push('{');
                    out.push_str(rest);
                    rest = "";
                    break;
                };
                let key_spec = &rest[..end];
                rest = &rest[end + 1..];
                let key = key_spec.split(':').next().unwrap_or(key_spec).trim();
                if !JSON_TEMPLATE_KEYS
                    .iter()
                    .any(|k| k.eq_ignore_ascii_case(key))
                    && !key.is_empty()
                {
                    continue;
                }
                if let Some(v) = lookup_json_value(obj, key) {
                    if let Some(disp) = value_as_display(v) {
                        out.push_str(&disp);
                    }
                }
            }
            out.push_str(rest);
            let body = sanitize_text(&out, 140);
            if body.is_empty() {
                return Err("empty_template".into());
            }
            let needs_temp = tmpl.contains("temperature_");
            if needs_temp {
                let has = lookup_json_value(obj, "temperature_f")
                    .or_else(|| lookup_json_value(obj, "temperature_c"))
                    .and_then(value_as_display)
                    .is_some();
                if !has {
                    return Err("missing_temp".into());
                }
            }
            Ok((body, false))
        }
    }
}

fn reading_from_json_obj(obj: &serde_json::Map<String, Value>) -> StationReading {
    let mut reading = StationReading {
        received_at: Instant::now(),
        source: StationSource::Mqtt,
        ..StationReading::default()
    };
    let num = |key: &str| {
        lookup_json_value(obj, key).and_then(|v| match v {
            Value::Number(n) => n.as_f64(),
            Value::String(s) => s.trim().parse().ok(),
            _ => None,
        })
    };
    reading.outdoor_temp_f = num("temperature_f");
    reading.outdoor_temp_c = num("temperature_c");
    reading.humidity_pct = num("humidity").or_else(|| num("humidity_percent"));
    reading.pressure_hpa = num("pressure_hpa");
    reading.wind_dir_deg = num("wind_direction_deg");
    if let Some(kmh) = num("wind_current_kmh") {
        reading.wind_mph = Some(kmh / 1.60934);
    }
    if let Some(v) = lookup_json_value(obj, "device").and_then(value_as_display) {
        reading.device = v;
    }
    reading
}

fn ingest_payload(payload: &[u8], cfg: &MqttWeatherConfig, fahrenheit: bool) {
    match format_mqtt_payload(payload, cfg.mode, &cfg.template, fahrenheit) {
        Ok((body, full_line)) => {
            let mut reading = if cfg.mode == OutputMode::JsonTemplate {
                if let Ok(Value::Object(obj)) = serde_json::from_slice::<Value>(payload) {
                    reading_from_json_obj(&obj)
                } else {
                    StationReading {
                        received_at: Instant::now(),
                        source: StationSource::Mqtt,
                        ..StationReading::default()
                    }
                }
            } else {
                StationReading {
                    received_at: Instant::now(),
                    source: StationSource::Mqtt,
                    ..StationReading::default()
                }
            };
            reading.mesh_body = Some(body);
            reading.mesh_full_line = full_line;
            reading.source = StationSource::Mqtt;
            reading.received_at = Instant::now();
            publish_station_reading(reading);
            mark_rx();
        }
        Err(err) => {
            hq_log!(debug, "mqtt weather payload: {err}");
        }
    }
}

// --- Minimal MQTT 3.1.1 subscriber -------------------------------------------------

fn encode_remaining_length(mut len: usize) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let mut byte = (len % 128) as u8;
        len /= 128;
        if len > 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if len == 0 {
            break;
        }
    }
    out
}

fn mqtt_str(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(2 + b.len());
    out.extend_from_slice(&(b.len() as u16).to_be_bytes());
    out.extend_from_slice(b);
    out
}

fn build_connect(client_id: &str, username: &str, password: &str) -> Vec<u8> {
    let mut flags = 0u8;
    let mut payload = mqtt_str(client_id);
    if !username.is_empty() {
        flags |= 0x80;
        payload.extend(mqtt_str(username));
        if !password.is_empty() {
            flags |= 0x40;
            payload.extend(mqtt_str(password));
        }
    }
    let mut vh = Vec::new();
    vh.extend(mqtt_str("MQTT"));
    vh.push(4); // protocol level 3.1.1
    vh.push(flags);
    vh.extend_from_slice(&30u16.to_be_bytes()); // keep alive
    let mut packet = Vec::new();
    packet.push(0x10); // CONNECT
    let rem = vh.len() + payload.len();
    packet.extend(encode_remaining_length(rem));
    packet.extend(vh);
    packet.extend(payload);
    packet
}

fn build_subscribe(packet_id: u16, topic: &str) -> Vec<u8> {
    let mut rem_payload = Vec::new();
    rem_payload.extend_from_slice(&packet_id.to_be_bytes());
    rem_payload.extend(mqtt_str(topic));
    rem_payload.push(0); // QoS 0
    let mut packet = Vec::new();
    packet.push(0x82); // SUBSCRIBE QoS1
    packet.extend(encode_remaining_length(rem_payload.len()));
    packet.extend(rem_payload);
    packet
}

fn build_pingreq() -> Vec<u8> {
    vec![0xC0, 0x00]
}

async fn read_packet<R: AsyncReadExt + Unpin>(
    reader: &mut R,
) -> Result<(u8, Vec<u8>), String> {
    let mut fixed = [0u8; 1];
    reader
        .read_exact(&mut fixed)
        .await
        .map_err(|e| e.to_string())?;
    let mut multiplier = 1usize;
    let mut remaining = 0usize;
    loop {
        let mut b = [0u8; 1];
        reader
            .read_exact(&mut b)
            .await
            .map_err(|e| e.to_string())?;
        remaining += (b[0] as usize & 0x7F) * multiplier;
        if b[0] & 0x80 == 0 {
            break;
        }
        multiplier *= 128;
        if multiplier > 128 * 128 * 128 {
            return Err("remaining length overflow".into());
        }
    }
    let mut payload = vec![0u8; remaining];
    if remaining > 0 {
        reader
            .read_exact(&mut payload)
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok((fixed[0], payload))
}

fn parse_publish(flags: u8, body: &[u8]) -> Result<(String, Vec<u8>), String> {
    if body.len() < 2 {
        return Err("short publish".into());
    }
    let tlen = u16::from_be_bytes([body[0], body[1]]) as usize;
    if body.len() < 2 + tlen {
        return Err("short topic".into());
    }
    let topic = String::from_utf8_lossy(&body[2..2 + tlen]).into_owned();
    let qos = (flags >> 1) & 0x03;
    let mut offset = 2 + tlen;
    if qos > 0 {
        if body.len() < offset + 2 {
            return Err("short packet id".into());
        }
        offset += 2;
    }
    Ok((topic, body[offset..].to_vec()))
}

enum StreamKind {
    Plain(TcpStream),
    Tls(tokio_rustls::client::TlsStream<TcpStream>),
}

impl StreamKind {
    async fn write_all(&mut self, data: &[u8]) -> Result<(), String> {
        match self {
            Self::Plain(s) => s.write_all(data).await.map_err(|e| e.to_string()),
            Self::Tls(s) => s.write_all(data).await.map_err(|e| e.to_string()),
        }
    }

    async fn read_packet(&mut self) -> Result<(u8, Vec<u8>), String> {
        match self {
            Self::Plain(s) => read_packet(s).await,
            Self::Tls(s) => read_packet(s).await,
        }
    }
}

async fn connect_stream(cfg: &MqttWeatherConfig) -> Result<StreamKind, String> {
    let addr = format!("{}:{}", cfg.broker, cfg.port);
    let tcp = TcpStream::connect(&addr)
        .await
        .map_err(|e| format!("connect {addr}: {e}"))?;
    if !cfg.tls {
        return Ok(StreamKind::Plain(tcp));
    }
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut root_store = rustls::RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    let connector = tokio_rustls::TlsConnector::from(std::sync::Arc::new(config));
    let server_name = rustls::pki_types::ServerName::try_from(cfg.broker.clone())
        .map_err(|e| format!("TLS server name: {e}"))?;
    let tls = connector
        .connect(server_name, tcp)
        .await
        .map_err(|e| format!("TLS: {e}"))?;
    Ok(StreamKind::Tls(tls))
}

async fn mqtt_session(db: &Db, cfg: &MqttWeatherConfig) -> Result<(), String> {
    let mut stream = connect_stream(cfg).await?;
    let client_id = format!(
        "metis-hq-wx-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() % 100_000)
            .unwrap_or(0)
    );
    stream
        .write_all(&build_connect(
            &client_id,
            &cfg.username,
            &cfg.password,
        ))
        .await?;

    let (fixed, body) = tokio::time::timeout(Duration::from_secs(10), stream.read_packet())
        .await
        .map_err(|_| "CONNACK timeout".to_string())?
        .map_err(|e| e)?;
    if fixed & 0xF0 != 0x20 || body.len() < 2 || body[1] != 0 {
        return Err(format!(
            "CONNACK failed (type={fixed:#x} code={})",
            body.get(1).copied().unwrap_or(255)
        ));
    }

    stream
        .write_all(&build_subscribe(1, &cfg.topic))
        .await?;
    // SUBACK optional — brokers may delay; continue either way.
    set_status(true, "");
    hq_log!(
        info,
        "mqtt weather: connected {}:{} topic={}",
        cfg.broker,
        cfg.port,
        cfg.topic
    );

    let mut last_ping = Instant::now();
    let start_gen = config_gen().lock().map(|g| *g).unwrap_or(0);
    loop {
        let gen_now = config_gen().lock().map(|g| *g).unwrap_or(0);
        if gen_now != start_gen {
            return Ok(());
        }
        let live = load_mqtt_weather_config(db, &crate::data_dir());
        if !live.enabled
            || live.broker != cfg.broker
            || live.topic != cfg.topic
            || live.port != cfg.port
            || live.tls != cfg.tls
        {
            return Ok(());
        }

        if last_ping.elapsed() > Duration::from_secs(20) {
            stream.write_all(&build_pingreq()).await?;
            last_ping = Instant::now();
        }

        match tokio::time::timeout(Duration::from_secs(5), stream.read_packet()).await {
            Ok(Ok((fixed, body))) => {
                let kind = fixed & 0xF0;
                match kind {
                    0x30 => {
                        // PUBLISH
                        if let Ok((topic, payload)) = parse_publish(fixed, &body) {
                            if topic == cfg.topic || topic == live.topic {
                                let unit_f = crate::services::weather_unit_f(db);
                                let cfg_now = load_mqtt_weather_config(db, &crate::data_dir());
                                ingest_payload(&payload, &cfg_now, unit_f);
                            }
                        }
                    }
                    0xD0 => {} // PINGRESP
                    0x90 => {} // SUBACK
                    0xE0 => return Err("broker DISCONNECT".into()),
                    _ => {}
                }
            }
            Ok(Err(err)) => return Err(err),
            Err(_) => {
                // idle — keep looping
            }
        }
    }
}

/// Long-lived MQTT subscriber. Call from the metis-services Tokio runtime.
pub async fn mqtt_weather_loop(db: std::sync::Arc<Db>) {
    let mut backoff = Duration::from_secs(2);
    loop {
        let data_dir = crate::data_dir();
        let cfg = load_mqtt_weather_config(&db, &data_dir);
        if !cfg.enabled || cfg.broker.is_empty() || cfg.topic.is_empty() {
            set_status(
                false,
                if !cfg.enabled {
                    ""
                } else {
                    "missing broker/topic"
                },
            );
            tokio::time::sleep(Duration::from_secs(5)).await;
            continue;
        }

        match mqtt_session(&db, &cfg).await {
            Ok(()) => {
                backoff = Duration::from_secs(2);
            }
            Err(err) => {
                set_status(false, err.clone());
                hq_log!(warn, "mqtt weather: {err}");
            }
        }
        set_status(false, "reconnecting");
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(60));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_template_formats_temp_humidity() {
        let payload = br#"{"temperature_f":68.2,"humidity":41,"device":"GW3002"}"#;
        let (body, full) = format_mqtt_payload(
            payload,
            OutputMode::JsonTemplate,
            "{temperature_f}F {humidity}%",
            true,
        )
        .unwrap();
        assert!(!full);
        assert!(body.contains("68"));
        assert!(body.contains("41"));
    }

    #[test]
    fn json_accepts_temp_f_alias() {
        let payload = br#"{"temp_f":70,"rh":55}"#;
        let (body, _) = format_mqtt_payload(
            payload,
            OutputMode::JsonTemplate,
            "{temperature_f}F {humidity}%",
            true,
        )
        .unwrap();
        assert!(body.contains("70"));
        assert!(body.contains("55"));
    }

    #[test]
    fn missing_temp_fails_when_template_needs_it() {
        let payload = br#"{"humidity":40}"#;
        let err = format_mqtt_payload(
            payload,
            OutputMode::JsonTemplate,
            "{temperature_f}F {humidity}%",
            true,
        )
        .unwrap_err();
        assert_eq!(err, "missing_temp");
    }

    #[test]
    fn passthrough_preserves_wx_prefix() {
        let (body, full) =
            format_mqtt_payload(b"WX Yard: Fair 58F 40%", OutputMode::Passthrough, "", true)
                .unwrap();
        assert!(full);
        assert!(body.starts_with("WX "));
    }

    #[test]
    fn passthrough_strips_controls() {
        let (body, full) =
            format_mqtt_payload(b"68F\x00100%", OutputMode::Passthrough, "", true).unwrap();
        assert!(!full);
        assert!(!body.contains('\0'));
        assert!(body.contains("68F"));
    }

    #[test]
    fn remaining_length_encoding() {
        assert_eq!(encode_remaining_length(0), vec![0]);
        assert_eq!(encode_remaining_length(127), vec![127]);
        assert_eq!(encode_remaining_length(128), vec![0x80, 0x01]);
    }

}
