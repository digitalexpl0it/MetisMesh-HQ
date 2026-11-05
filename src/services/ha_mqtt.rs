//! Opt-in Home Assistant MQTT discovery (M7).
//!
//! Publishes retained discovery configs + periodic state for the same sensors
//! documented for Local API REST (companion online/state, enrolled count, first
//! claimed repeater battery/noise/probe time). Separate from Weather MQTT.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::db::Db;
use crate::radio::RadioHandle;
use crate::services::mqtt_client::{
    build_pingreq, build_publish, connect_stream, mqtt_handshake,
};
use crate::signup::Enrollment;

const PASS_FILE: &str = "ha-mqtt.pass";
const DEFAULT_PORT: u16 = 1883;
const DEFAULT_DISCOVERY_PREFIX: &str = "homeassistant";
const DEFAULT_NODE_ID: &str = "metis_mesh_hq";
const DEFAULT_STATE_PREFIX: &str = "metis_mesh_hq";
const DEFAULT_INTERVAL_SECS: u64 = 30;

#[derive(Debug, Clone)]
pub struct HaMqttConfig {
    pub enabled: bool,
    pub broker: String,
    pub port: u16,
    pub tls: bool,
    pub username: String,
    pub password: String,
    pub discovery_prefix: String,
    pub node_id: String,
    pub state_prefix: String,
    pub interval_secs: u64,
}

#[derive(Debug, Clone, Default)]
struct RuntimeStatus {
    connected: bool,
    last_error: String,
    last_pub: Option<Instant>,
}

fn status() -> &'static Mutex<RuntimeStatus> {
    static STATUS: OnceLock<Mutex<RuntimeStatus>> = OnceLock::new();
    STATUS.get_or_init(|| Mutex::new(RuntimeStatus::default()))
}

fn config_gen() -> &'static Mutex<u64> {
    static GEN: OnceLock<Mutex<u64>> = OnceLock::new();
    GEN.get_or_init(|| Mutex::new(0))
}

pub fn bump_ha_mqtt_config() {
    if let Ok(mut g) = config_gen().lock() {
        *g = g.wrapping_add(1);
    }
}

pub fn ha_mqtt_pass_path(data_dir: &Path) -> PathBuf {
    data_dir.join(PASS_FILE)
}

pub fn save_ha_mqtt_password(data_dir: &Path, password: &str) -> Result<(), String> {
    fs::create_dir_all(data_dir).map_err(|e| e.to_string())?;
    let path = ha_mqtt_pass_path(data_dir);
    let password = password.trim();
    if password.is_empty() {
        let _ = fs::remove_file(&path);
        bump_ha_mqtt_config();
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
    bump_ha_mqtt_config();
    Ok(())
}

pub fn load_ha_mqtt_password(data_dir: &Path) -> String {
    fs::read_to_string(ha_mqtt_pass_path(data_dir))
        .ok()
        .map(|s| s.trim_end_matches(['\r', '\n']).to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_default()
}

pub fn clear_ha_mqtt_password(data_dir: &Path) {
    let _ = fs::remove_file(ha_mqtt_pass_path(data_dir));
    bump_ha_mqtt_config();
}

pub fn ha_mqtt_password_present(data_dir: &Path) -> bool {
    ha_mqtt_pass_path(data_dir).is_file()
}

fn setting(db: &Db, key: &str) -> String {
    db.setting(key)
        .ok()
        .flatten()
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// Clamp publish interval to 10..=300 seconds.
pub fn clamp_ha_mqtt_interval(secs: u64) -> u64 {
    secs.clamp(10, 300)
}

pub fn discovery_config_topic(
    discovery_prefix: &str,
    component: &str,
    node_id: &str,
    object_id: &str,
) -> String {
    format!("{discovery_prefix}/{component}/{node_id}/{object_id}/config")
}

pub fn companion_online_payload(state: &str) -> &'static str {
    if state == "connected" {
        "ON"
    } else {
        "OFF"
    }
}

pub fn load_ha_mqtt_config(db: &Db, data_dir: &Path) -> HaMqttConfig {
    let interval = setting(db, "ha_mqtt_interval_secs")
        .parse::<u64>()
        .unwrap_or(DEFAULT_INTERVAL_SECS);
    let discovery_prefix = {
        let raw = setting(db, "ha_mqtt_discovery_prefix");
        if raw.is_empty() {
            DEFAULT_DISCOVERY_PREFIX.to_string()
        } else {
            raw
        }
    };
    let node_id = {
        let raw = setting(db, "ha_mqtt_node_id");
        if raw.is_empty() {
            DEFAULT_NODE_ID.to_string()
        } else {
            raw
        }
    };
    let state_prefix = {
        let raw = setting(db, "ha_mqtt_state_prefix");
        if raw.is_empty() {
            DEFAULT_STATE_PREFIX.to_string()
        } else {
            raw
        }
    };
    HaMqttConfig {
        enabled: crate::services::setting_bool(db, "ha_mqtt_enabled", false),
        broker: setting(db, "ha_mqtt_broker"),
        port: setting(db, "ha_mqtt_port")
            .parse::<u16>()
            .unwrap_or(DEFAULT_PORT),
        tls: crate::services::setting_bool(db, "ha_mqtt_tls", false),
        username: setting(db, "ha_mqtt_username"),
        password: load_ha_mqtt_password(data_dir),
        discovery_prefix,
        node_id,
        state_prefix,
        interval_secs: clamp_ha_mqtt_interval(interval),
    }
}

fn set_status(connected: bool, err: &str) {
    if let Ok(mut s) = status().lock() {
        s.connected = connected;
        s.last_error = err.to_string();
        if connected {
            s.last_pub = Some(Instant::now());
        }
    }
}

fn mark_pub() {
    if let Ok(mut s) = status().lock() {
        s.last_pub = Some(Instant::now());
        s.last_error.clear();
    }
}

pub fn ha_mqtt_status_line(db: &Db) -> String {
    let cfg = load_ha_mqtt_config(db, &crate::data_dir());
    if !cfg.enabled {
        return "Home Assistant MQTT discovery off".into();
    }
    if cfg.broker.is_empty() {
        return "HA MQTT: enabled — set broker".into();
    }
    let guard = status().lock().unwrap_or_else(|e| e.into_inner());
    if !guard.last_error.is_empty() {
        return format!("HA MQTT: error — {}", guard.last_error);
    }
    if guard.connected {
        let age = guard
            .last_pub
            .map(|t| format!("{}s ago", t.elapsed().as_secs()))
            .unwrap_or_else(|| "just now".into());
        format!(
            "HA MQTT: publishing {}:{} every {}s (last {age})",
            cfg.broker, cfg.port, cfg.interval_secs
        )
    } else {
        format!("HA MQTT: connecting {}:{}…", cfg.broker, cfg.port)
    }
}

fn device_block(node_id: &str) -> Value {
    json!({
        "identifiers": [node_id],
        "name": "Metis Mesh HQ",
        "manufacturer": "Metis Mesh",
    })
}

fn discovery_entities(cfg: &HaMqttConfig) -> Vec<(String, Value)> {
    // (discovery_topic, config_json)
    let node = &cfg.node_id;
    let state = &cfg.state_prefix;
    let disc = &cfg.discovery_prefix;
    let device = device_block(node);

    let mut out = Vec::new();

    let online_state = format!("{state}/companion_online");
    out.push((
        discovery_config_topic(disc, "binary_sensor", node, "companion_online"),
        json!({
            "name": "Companion online",
            "unique_id": format!("{node}_companion_online"),
            "state_topic": online_state,
            "payload_on": "ON",
            "payload_off": "OFF",
            "device_class": "connectivity",
            "device": device.clone(),
        }),
    ));

    let companion_state = format!("{state}/companion_state");
    out.push((
        discovery_config_topic(disc, "sensor", node, "companion_state"),
        json!({
            "name": "Companion state",
            "unique_id": format!("{node}_companion_state"),
            "state_topic": companion_state,
            "device": device.clone(),
        }),
    ));

    let enrolled_topic = format!("{state}/enrolled");
    out.push((
        discovery_config_topic(disc, "sensor", node, "enrolled"),
        json!({
            "name": "Enrolled count",
            "unique_id": format!("{node}_enrolled"),
            "state_topic": enrolled_topic,
            "state_class": "measurement",
            "device": device.clone(),
        }),
    ));

    let batt_topic = format!("{state}/repeater_battery_mv");
    out.push((
        discovery_config_topic(disc, "sensor", node, "repeater_battery_mv"),
        json!({
            "name": "Repeater battery",
            "unique_id": format!("{node}_repeater_battery_mv"),
            "state_topic": batt_topic,
            "unit_of_measurement": "mV",
            "state_class": "measurement",
            "device": device.clone(),
        }),
    ));

    let noise_topic = format!("{state}/repeater_noise");
    out.push((
        discovery_config_topic(disc, "sensor", node, "repeater_noise"),
        json!({
            "name": "Repeater noise",
            "unique_id": format!("{node}_repeater_noise"),
            "state_topic": noise_topic,
            "unit_of_measurement": "dBm",
            "state_class": "measurement",
            "device": device.clone(),
        }),
    ));

    let probed_topic = format!("{state}/repeater_probed_at");
    out.push((
        discovery_config_topic(disc, "sensor", node, "repeater_probed_at"),
        json!({
            "name": "Repeater probed at",
            "unique_id": format!("{node}_repeater_probed_at"),
            "state_topic": probed_topic,
            "device": device,
        }),
    ));

    out
}

fn enrolled_count(db: &Db) -> usize {
    db.list_nodes()
        .unwrap_or_default()
        .into_iter()
        .filter(|n| matches!(n.enrollment, Enrollment::Enrolled))
        .count()
}

/// First claimed infra row (same order as Local API `/infra` / Help M6).
fn first_claimed_latest(db: &Db) -> Option<(i64, i64, i64)> {
    let claimed = db.list_claimed_infra().ok()?;
    let first = claimed.first()?;
    let latest = db.list_infra_status_latest().ok()?;
    let row = latest.into_iter().find(|s| s.pubkey == first.pubkey)?;
    Some((row.battery_mv, row.noise_floor, row.probed_at))
}

fn collect_states(db: &Db, radio: &RadioHandle) -> Vec<(String, String)> {
    let snap = radio.snapshot();
    let mut out = Vec::new();
    // Caller prepends state_prefix when publishing.
    out.push((
        "companion_online".into(),
        companion_online_payload(&snap.state).to_string(),
    ));
    out.push(("companion_state".into(), snap.state));
    out.push(("enrolled".into(), enrolled_count(db).to_string()));
    match first_claimed_latest(db) {
        Some((batt, noise, probed)) => {
            out.push(("repeater_battery_mv".into(), batt.to_string()));
            out.push(("repeater_noise".into(), noise.to_string()));
            out.push(("repeater_probed_at".into(), probed.to_string()));
        }
        None => {
            out.push(("repeater_battery_mv".into(), String::new()));
            out.push(("repeater_noise".into(), String::new()));
            out.push(("repeater_probed_at".into(), String::new()));
        }
    }
    out
}

async fn mqtt_session(
    db: &Db,
    radio: &RadioHandle,
    cfg: &HaMqttConfig,
) -> Result<(), String> {
    let mut stream = connect_stream(&cfg.broker, cfg.port, cfg.tls).await?;
    let client_id = format!(
        "metis-hq-ha-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() % 100_000)
            .unwrap_or(0)
    );
    mqtt_handshake(&mut stream, &client_id, &cfg.username, &cfg.password).await?;

    let entities = discovery_entities(cfg);
    for (topic, config) in &entities {
        let payload = serde_json::to_vec(config).map_err(|e| e.to_string())?;
        stream
            .write_all(&build_publish(topic, &payload, true))
            .await?;
    }

    set_status(true, "");
    hq_log!(
        info,
        "ha mqtt: connected {}:{} node={}",
        cfg.broker,
        cfg.port,
        cfg.node_id
    );

    let mut last_ping = Instant::now();
    let mut last_state = Instant::now()
        .checked_sub(Duration::from_secs(cfg.interval_secs))
        .unwrap_or_else(Instant::now);
    let start_gen = config_gen().lock().map(|g| *g).unwrap_or(0);

    loop {
        let gen_now = config_gen().lock().map(|g| *g).unwrap_or(0);
        if gen_now != start_gen {
            return Ok(());
        }
        let live = load_ha_mqtt_config(db, &crate::data_dir());
        if !live.enabled
            || live.broker != cfg.broker
            || live.port != cfg.port
            || live.tls != cfg.tls
            || live.discovery_prefix != cfg.discovery_prefix
            || live.node_id != cfg.node_id
            || live.state_prefix != cfg.state_prefix
        {
            return Ok(());
        }

        if last_state.elapsed() >= Duration::from_secs(live.interval_secs) {
            for (suffix, payload) in collect_states(db, radio) {
                let topic = format!("{}/{}", cfg.state_prefix, suffix);
                stream
                    .write_all(&build_publish(&topic, payload.as_bytes(), false))
                    .await?;
            }
            mark_pub();
            last_state = Instant::now();
        }

        if last_ping.elapsed() > Duration::from_secs(20) {
            stream.write_all(&build_pingreq()).await?;
            last_ping = Instant::now();
        }

        match tokio::time::timeout(Duration::from_secs(2), stream.read_packet()).await {
            Ok(Ok((fixed, _body))) => {
                let kind = fixed & 0xF0;
                match kind {
                    0xD0 => {} // PINGRESP
                    0xE0 => return Err("broker DISCONNECT".into()),
                    _ => {}
                }
            }
            Ok(Err(err)) => return Err(err),
            Err(_) => {}
        }
    }
}

/// Long-lived HA discovery publisher. Call from the metis-services Tokio runtime.
pub async fn ha_mqtt_loop(db: std::sync::Arc<Db>, radio: RadioHandle) {
    let mut backoff = Duration::from_secs(2);
    loop {
        let data_dir = crate::data_dir();
        let cfg = load_ha_mqtt_config(&db, &data_dir);
        if !cfg.enabled || cfg.broker.is_empty() {
            set_status(
                false,
                if !cfg.enabled {
                    ""
                } else {
                    "missing broker"
                },
            );
            tokio::time::sleep(Duration::from_secs(5)).await;
            continue;
        }

        match mqtt_session(&db, &radio, &cfg).await {
            Ok(()) => {
                backoff = Duration::from_secs(2);
                set_status(false, "");
            }
            Err(err) => {
                hq_log!(warn, "ha mqtt: {err}");
                set_status(false, &err);
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(60));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_topic_path() {
        assert_eq!(
            discovery_config_topic("homeassistant", "sensor", "metis_mesh_hq", "enrolled"),
            "homeassistant/sensor/metis_mesh_hq/enrolled/config"
        );
        assert_eq!(
            discovery_config_topic(
                "homeassistant",
                "binary_sensor",
                "metis_mesh_hq",
                "companion_online"
            ),
            "homeassistant/binary_sensor/metis_mesh_hq/companion_online/config"
        );
    }

    #[test]
    fn companion_online_maps_connected() {
        assert_eq!(companion_online_payload("connected"), "ON");
        assert_eq!(companion_online_payload("disconnected"), "OFF");
        assert_eq!(companion_online_payload("scanning"), "OFF");
    }

    #[test]
    fn interval_clamp() {
        assert_eq!(clamp_ha_mqtt_interval(5), 10);
        assert_eq!(clamp_ha_mqtt_interval(30), 30);
        assert_eq!(clamp_ha_mqtt_interval(999), 300);
    }
}
