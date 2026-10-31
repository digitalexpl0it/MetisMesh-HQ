//! One task owns the companion radio. The window only sends commands.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use meshcore_rs::commands::Destination;
use meshcore_rs::events::{ContactMessage, EventPayload, EventType, MeshCoreEvent};
use meshcore_rs::MeshCore;
use serde_json::json;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use crate::db::{contact_snapshot, unix_now, Db};
use crate::path::{hop_count_from_path_len, parse_advert_path};
use crate::secrets::{Secrets, CHANNEL_NAME};
use crate::signup::{decide, parse_command};

const REPLY_GAP: Duration = Duration::from_secs(20);

/// One Bluetooth advertisement the scan was able to identify.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BleRadio {
    pub name: String,
    pub address: String,
    pub address_kind: String,
    pub rssi: Option<i16>,
    pub companion: bool,
    pub manufacturer: String,
}

impl BleRadio {
    /// Third line of the device card: signal, address type, and what the advert says it is.
    pub fn detail_line(&self) -> String {
        let mut parts = Vec::new();
        if let Some(rssi) = self.rssi {
            parts.push(format!("{rssi} dBm, {} signal", signal_word(rssi)));
        } else {
            parts.push("Signal not reported".into());
        }
        if self.address_kind.is_empty() {
            parts.push("address type unknown".into());
        } else {
            parts.push(format!("{} address", self.address_kind));
        }
        if self.companion {
            parts.push("MeshCore companion".into());
        }
        if !self.manufacturer.is_empty() {
            parts.push(self.manufacturer.clone());
        } else if !self.companion {
            parts.push("general Bluetooth device".into());
        }
        parts.join(", ")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SyncPhase {
    #[default]
    Idle,
    Connecting,
    AppStart,
    Contacts,
    Channels,
    Messages,
    Ready,
}

impl SyncPhase {
    pub fn is_active(self) -> bool {
        !matches!(self, Self::Idle | Self::Ready)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RadioStatus {
    pub state: String,
    pub detail: String,
    pub devices: Vec<BleRadio>,
    pub connected_name: String,
    pub sync_phase: SyncPhase,
    pub sync_label: String,
    /// `(done, total)` when a determinate step is known.
    pub sync_progress: Option<(u32, u32)>,
    pub battery_pct: Option<u8>,
    pub battery_mv: Option<u16>,
    /// Inferred from rising voltage between polls (companion has no charge flag).
    pub battery_charging: bool,
    pub firmware: String,
    pub model: String,
    pub hq_lat: Option<f64>,
    pub hq_lon: Option<f64>,
    pub tx_power: Option<u8>,
    pub max_tx_power: Option<u8>,
    /// LoRa frequency in MHz (from SelfInfo milli-units / 1000).
    pub radio_freq_mhz: Option<f64>,
    /// LoRa bandwidth in kHz.
    pub radio_bw_khz: Option<f64>,
    pub radio_sf: Option<u8>,
    pub radio_cr: Option<u8>,
    /// Advert location policy from SelfInfo (0 none / 1 share / 2 prefs).
    pub adv_loc_policy: Option<u8>,
    /// Round-trip fields for `CMD_SET_OTHER_PARAMS` (38).
    pub manual_add_contacts: bool,
    pub telemetry_mode_base: u8,
    pub telemetry_mode_loc: u8,
    pub telemetry_mode_env: u8,
    pub multi_acks: u8,
    /// Path hash mode 0/1/2 → 1/2/3-byte (`CMD_SET_PATH_HASH_MODE` 61).
    pub path_hash_mode: Option<u8>,
    /// HQ public key hex from SelfInfo.
    pub public_key: String,
    /// Client repeat / off-grid repeat (device query + radio params).
    pub client_repeat: bool,
    /// Last flood-scope region string applied or cleared this session.
    pub flood_scope: Option<String>,
    /// Firmware custom vars (`get_custom_vars`), sorted by key for stable UI.
    pub custom_vars: BTreeMap<String, String>,
    /// Unix seconds when the current BLE session became ready.
    pub connected_since: Option<i64>,
    /// Latest local/remote CLI reply for the CLI page transcript.
    pub cli_seq: u64,
    pub cli_line: String,
}

impl Default for RadioStatus {
    fn default() -> Self {
        Self {
            state: "disconnected".into(),
            detail: "Pick a saved companion to reconnect, or pair a new radio from Settings."
                .into(),
            devices: Vec::new(),
            connected_name: String::new(),
            sync_phase: SyncPhase::Idle,
            sync_label: String::new(),
            sync_progress: None,
            battery_pct: None,
            battery_mv: None,
            battery_charging: false,
            firmware: String::new(),
            model: String::new(),
            hq_lat: None,
            hq_lon: None,
            tx_power: None,
            max_tx_power: None,
            radio_freq_mhz: None,
            radio_bw_khz: None,
            radio_sf: None,
            radio_cr: None,
            adv_loc_policy: None,
            manual_add_contacts: false,
            telemetry_mode_base: 0,
            telemetry_mode_loc: 0,
            telemetry_mode_env: 0,
            multi_acks: 0,
            path_hash_mode: None,
            public_key: String::new(),
            client_repeat: false,
            flood_scope: None,
            custom_vars: BTreeMap::new(),
            connected_since: None,
            cli_seq: 0,
            cli_line: String::new(),
        }
    }
}

pub enum RadioCmd {
    Scan,
    /// First-time Bluetooth pairing with the MeshCore PIN.
    Pair { target: String, pin: u32 },
    /// Reconnect to a radio that is already paired and saved.
    Reconnect(String),
    Disconnect,
    ApplyName(String),
    /// Push a channel definition onto the companion radio.
    PushChannel { slot: u8, name: String },
    /// Soft-delete / clear a non-system channel on the radio.
    ClearChannel { slot: u8 },
    /// Send text on a channel slot.
    SendChannel { slot: u8, text: String },
    RefreshDeviceSettings,
    SetCustomVar { key: String, value: String },
    SetCoords { lat: f64, lon: f64 },
    SetTxPower(u8),
    SetRadioParams {
        freq_mhz: f64,
        bw_khz: f64,
        sf: u8,
        cr: u8,
        repeat: bool,
    },
    /// Companion `CMD_SET_PATH_HASH_MODE` (61): mode 0/1/2 → 1/2/3-byte.
    SetPathHashMode(u8),
    /// Companion `CMD_SET_OTHER_PARAMS` (38): advert location policy 0/1/(2).
    SetAdvLocPolicy(u8),
    SetFloodScope(Option<String>),
    SendAdvert { flood: bool },
    Reboot,
    /// Local companion CLI (CMD 66) or remote TXT_TYPE_CLI_COMMAND to a contact prefix.
    RunCli {
        /// Empty = local companion; otherwise 12-char hex pubkey prefix (or longer hex pubkey).
        contact_hex: String,
        text: String,
    },
    /// Probe noise floor via status requests for GPS contacts.
    ProbeNoise,
}

#[derive(Clone)]
pub struct RadioHandle {
    cmd: UnboundedSender<RadioCmd>,
    status: Arc<Mutex<RadioStatus>>,
}

impl RadioHandle {
    pub fn scan(&self) {
        let _ = self.cmd.send(RadioCmd::Scan);
    }

    pub fn pair(&self, target: String, pin: u32) {
        let _ = self.cmd.send(RadioCmd::Pair { target, pin });
    }

    pub fn reconnect(&self, target: String) {
        let _ = self.cmd.send(RadioCmd::Reconnect(target));
    }

    pub fn disconnect(&self) {
        let _ = self.cmd.send(RadioCmd::Disconnect);
    }

    pub fn apply_name(&self, name: String) {
        let _ = self.cmd.send(RadioCmd::ApplyName(name));
    }

    pub fn push_channel(&self, slot: u8, name: String) {
        let _ = self.cmd.send(RadioCmd::PushChannel { slot, name });
    }

    pub fn clear_channel(&self, slot: u8) {
        let _ = self.cmd.send(RadioCmd::ClearChannel { slot });
    }

    pub fn send_channel(&self, slot: u8, text: String) {
        let _ = self.cmd.send(RadioCmd::SendChannel { slot, text });
    }

    pub fn refresh_device_settings(&self) {
        let _ = self.cmd.send(RadioCmd::RefreshDeviceSettings);
    }

    pub fn set_custom_var(&self, key: String, value: String) {
        let _ = self.cmd.send(RadioCmd::SetCustomVar { key, value });
    }

    pub fn set_coords(&self, lat: f64, lon: f64) {
        let _ = self.cmd.send(RadioCmd::SetCoords { lat, lon });
    }

    pub fn set_tx_power(&self, power: u8) {
        let _ = self.cmd.send(RadioCmd::SetTxPower(power));
    }

    pub fn set_radio_params(&self, freq_mhz: f64, bw_khz: f64, sf: u8, cr: u8, repeat: bool) {
        let _ = self.cmd.send(RadioCmd::SetRadioParams {
            freq_mhz,
            bw_khz,
            sf,
            cr,
            repeat,
        });
    }

    pub fn set_flood_scope(&self, scope: Option<String>) {
        let _ = self.cmd.send(RadioCmd::SetFloodScope(scope));
    }

    pub fn set_path_hash_mode(&self, mode: u8) {
        let _ = self.cmd.send(RadioCmd::SetPathHashMode(mode.min(2)));
    }

    pub fn set_adv_loc_policy(&self, policy: u8) {
        let _ = self.cmd.send(RadioCmd::SetAdvLocPolicy(policy.min(2)));
    }

    pub fn send_advert(&self, flood: bool) {
        let _ = self.cmd.send(RadioCmd::SendAdvert { flood });
    }

    pub fn reboot(&self) {
        let _ = self.cmd.send(RadioCmd::Reboot);
    }

    pub fn run_cli(&self, contact_hex: String, text: String) {
        let _ = self.cmd.send(RadioCmd::RunCli { contact_hex, text });
    }

    pub fn probe_noise_samples(&self) {
        let _ = self.cmd.send(RadioCmd::ProbeNoise);
    }

    pub fn snapshot(&self) -> RadioStatus {
        self.status
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone()
    }
}

pub fn spawn_radio(db: Arc<Db>, secrets: Arc<Secrets>) -> RadioHandle {
    let (cmd, rx) = unbounded_channel();
    let status = Arc::new(Mutex::new(RadioStatus::default()));
    let status_thread = status.clone();
    std::thread::Builder::new()
        .name("metis-radio".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("radio runtime");
            runtime.block_on(radio_loop(db, secrets, status_thread, rx));
        })
        .expect("radio thread");
    RadioHandle { cmd, status }
}

fn write_status(status: &Mutex<RadioStatus>, state: &str, detail: impl Into<String>) {
    let mut guard = status.lock().unwrap_or_else(|err| err.into_inner());
    guard.state = state.to_string();
    guard.detail = detail.into();
}

fn write_sync(
    status: &Mutex<RadioStatus>,
    phase: SyncPhase,
    label: impl Into<String>,
    progress: Option<(u32, u32)>,
) {
    let mut guard = status.lock().unwrap_or_else(|err| err.into_inner());
    guard.sync_phase = phase;
    guard.sync_label = label.into();
    guard.sync_progress = progress;
}

fn clear_session_telemetry(status: &Mutex<RadioStatus>) {
    let mut guard = status.lock().unwrap_or_else(|err| err.into_inner());
    guard.connected_name.clear();
    guard.sync_phase = SyncPhase::Idle;
    guard.sync_label.clear();
    guard.sync_progress = None;
    guard.battery_pct = None;
    guard.battery_mv = None;
    guard.battery_charging = false;
    guard.firmware.clear();
    guard.model.clear();
    guard.hq_lat = None;
    guard.hq_lon = None;
    guard.tx_power = None;
    guard.max_tx_power = None;
    guard.radio_freq_mhz = None;
    guard.radio_bw_khz = None;
    guard.radio_sf = None;
    guard.radio_cr = None;
    guard.adv_loc_policy = None;
    guard.manual_add_contacts = false;
    guard.telemetry_mode_base = 0;
    guard.telemetry_mode_loc = 0;
    guard.telemetry_mode_env = 0;
    guard.multi_acks = 0;
    guard.path_hash_mode = None;
    guard.public_key.clear();
    guard.client_repeat = false;
    guard.flood_scope = None;
    guard.custom_vars.clear();
    guard.connected_since = None;
}

/// Firmware uses `key:value` (comma-separated on get). meshcore-rs uses `=` — send `:` ourselves.
async fn apply_custom_var(
    commands: &meshcore_rs::commands::CommandHandler,
    key: &str,
    value: &str,
) -> Result<(), String> {
    let mut data = vec![0x29u8]; // CMD_SET_CUSTOM_VAR = 41
    data.extend_from_slice(key.as_bytes());
    data.push(b':');
    data.extend_from_slice(value.as_bytes());
    commands
        .send(&data, Some(EventType::Ok))
        .await
        .map_err(|err| err.to_string())?;
    Ok(())
}

/// Companion `CMD_SET_PATH_HASH_MODE` (61): `[61, 0, mode]`.
async fn apply_path_hash_mode(
    commands: &meshcore_rs::commands::CommandHandler,
    mode: u8,
) -> Result<(), String> {
    if mode > 2 {
        return Err("Path hash mode must be 0, 1, or 2.".into());
    }
    let data = [0x3du8, 0, mode];
    commands
        .send(&data, Some(EventType::Ok))
        .await
        .map_err(|err| err.to_string())?;
    Ok(())
}

/// Companion `CMD_SET_OTHER_PARAMS` (38) — preserve other prefs, change advert loc policy.
async fn apply_adv_loc_policy(
    commands: &meshcore_rs::commands::CommandHandler,
    status: &Mutex<RadioStatus>,
    policy: u8,
) -> Result<(), String> {
    let (manual, telem, multi) = {
        let guard = status.lock().unwrap_or_else(|err| err.into_inner());
        let telem = (guard.telemetry_mode_base & 0x03)
            | ((guard.telemetry_mode_loc & 0x03) << 2)
            | ((guard.telemetry_mode_env & 0x03) << 4);
        (
            u8::from(guard.manual_add_contacts),
            telem,
            guard.multi_acks,
        )
    };
    let data = [0x26u8, manual, telem, policy.min(2), multi];
    commands
        .send(&data, Some(EventType::Ok))
        .await
        .map_err(|err| err.to_string())?;
    Ok(())
}

fn parse_custom_vars_blob(text: &str) -> BTreeMap<String, String> {
    let mut vars = BTreeMap::new();
    for part in text.split(|c| c == ',' || c == '\n') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((key, value)) = part.split_once(':').or_else(|| part.split_once('=')) {
            let key = key.trim();
            if !key.is_empty() {
                vars.insert(key.to_string(), value.trim().to_string());
            }
        }
    }
    vars
}

async fn refresh_custom_vars(
    commands: &meshcore_rs::commands::CommandHandler,
    status: &Mutex<RadioStatus>,
) {
    match commands.get_custom_vars().await {
        Ok(map) => {
            let mut vars: BTreeMap<String, String> = map.into_iter().collect();
            // meshcore-rs splits on `=`; firmware emits `key:value,key:value`. Recover when possible.
            if vars.is_empty() {
                // leave empty — Identity still offers known gps keys for write
            } else if vars.len() == 1 {
                let (k, v) = vars.iter().next().map(|(k, v)| (k.clone(), v.clone())).unwrap();
                let recovered = parse_custom_vars_blob(&format!("{k}:{v}"));
                if recovered.len() > 1 || k.contains(':') {
                    vars = recovered;
                }
            }
            let mut guard = status.lock().unwrap_or_else(|err| err.into_inner());
            guard.custom_vars = vars;
        }
        Err(err) => eprintln!("custom vars: {err}"),
    }
}

/// Companion `CMD_SET_RADIO_PARAMS` (11). Units match firmware: MHz×1000, kHz×1000.
async fn apply_radio_params(
    commands: &meshcore_rs::commands::CommandHandler,
    freq_mhz: f64,
    bw_khz: f64,
    sf: u8,
    cr: u8,
    repeat: bool,
) -> Result<(), String> {
    if !(300.0..=2500.0).contains(&freq_mhz) {
        return Err("Frequency must be between 300 and 2500 MHz.".into());
    }
    if !(7.8..=500.0).contains(&bw_khz) {
        return Err("Bandwidth must be between 7.8 and 500 kHz.".into());
    }
    if !(5..=12).contains(&sf) {
        return Err("Spreading factor must be 5–12.".into());
    }
    if !(5..=8).contains(&cr) {
        return Err("Coding rate must be 5–8.".into());
    }
    let freq = (freq_mhz * 1000.0).round() as u32;
    let bw = (bw_khz * 1000.0).round() as u32;
    let mut data = vec![0x0bu8];
    data.extend_from_slice(&freq.to_le_bytes());
    data.extend_from_slice(&bw.to_le_bytes());
    data.push(sf);
    data.push(cr);
    data.push(u8::from(repeat));
    commands
        .send(&data, Some(meshcore_rs::events::EventType::Ok))
        .await
        .map_err(|err| err.to_string())?;
    Ok(())
}

fn apply_battery(
    db: &Db,
    status: &Mutex<RadioStatus>,
    battery: &meshcore_rs::events::BatteryInfo,
) {
    let pct = battery_pct_from_mv(battery.battery_mv);
    let mut guard = status.lock().unwrap_or_else(|err| err.into_inner());
    let charging = match guard.battery_mv {
        Some(prev) if battery.battery_mv + 15 < prev => false,
        Some(prev) if battery.battery_mv > prev.saturating_add(20) => true,
        Some(_) => guard.battery_charging,
        None => battery.battery_mv >= 4180,
    } || battery.battery_mv >= 4190;
    guard.battery_mv = Some(battery.battery_mv);
    guard.battery_pct = Some(pct);
    guard.battery_charging = charging;
    if !guard.connected_name.is_empty() {
        let charge = if charging { " charging" } else { "" };
        guard.detail = format!(
            "Connected to {}. Battery {pct}% ({} mV{charge}).",
            guard.connected_name, battery.battery_mv
        );
    }
    drop(guard);
    let _ = db.insert_telemetry_sample("battery", battery.battery_mv as f64, pct as f64);
}

/// Single-cell Li-ion / LiPo estimate for MeshCore companions.
/// meshcore-rs treats ≥3930 mV as 100%, which keeps a fresh pack pinned at 100%
/// for a long time; this curve drops sooner so live status is usable.
pub fn battery_pct_from_mv(mv: u16) -> u8 {
    // (millivolts, percent) — interpolate between points.
    const TABLE: [(u16, u8); 16] = [
        (4200, 100),
        (4150, 95),
        (4100, 90),
        (4050, 85),
        (4000, 80),
        (3950, 75),
        (3900, 70),
        (3850, 60),
        (3800, 50),
        (3750, 40),
        (3700, 30),
        (3650, 20),
        (3600, 12),
        (3500, 5),
        (3400, 2),
        (3000, 0),
    ];
    if mv >= TABLE[0].0 {
        return 100;
    }
    if mv <= TABLE[TABLE.len() - 1].0 {
        return 0;
    }
    for window in TABLE.windows(2) {
        let (hi_mv, hi_pct) = window[0];
        let (lo_mv, lo_pct) = window[1];
        if mv <= hi_mv && mv >= lo_mv {
            let span = (hi_mv - lo_mv) as u32;
            if span == 0 {
                return hi_pct;
            }
            let t = (hi_mv - mv) as u32;
            let delta = hi_pct.saturating_sub(lo_pct) as u32;
            return hi_pct.saturating_sub(((t * delta) / span) as u8);
        }
    }
    0
}

fn connected_detail(status: &Mutex<RadioStatus>, name: &str) -> String {
    let guard = status.lock().unwrap_or_else(|err| err.into_inner());
    match (guard.battery_pct, guard.battery_mv) {
        (Some(pct), Some(mv)) => format!("Connected to {name}. Battery {pct}% ({mv} mV)."),
        _ => format!("Connected to {name}."),
    }
}

async fn radio_loop(
    db: Arc<Db>,
    secrets: Arc<Secrets>,
    status: Arc<Mutex<RadioStatus>>,
    mut rx: UnboundedReceiver<RadioCmd>,
) {
    let mut desired: Option<String> = None;
    let mut backoff = Duration::from_secs(2);
    loop {
        if let Some(name) = desired.clone() {
            write_status(&status, "connecting", format!("Connecting to {name}"));
            write_sync(
                &status,
                SyncPhase::Connecting,
                format!("Connecting to {name}…"),
                None,
            );
            clear_session_telemetry(&status);
            match run_session(&db, &secrets, &status, &mut rx, &name).await {
                SessionEnd::Stopped => {
                    desired = None;
                    backoff = Duration::from_secs(2);
                    write_status(&status, "disconnected", "Disconnected.");
                    clear_session_telemetry(&status);
                }
                SessionEnd::Switch(next) => {
                    desired = Some(next);
                    backoff = Duration::from_secs(2);
                    clear_session_telemetry(&status);
                }
                SessionEnd::Lost(error) => {
                    {
                        let mut guard = status.lock().unwrap_or_else(|err| err.into_inner());
                        guard.connected_name.clear();
                        guard.sync_phase = SyncPhase::Idle;
                        guard.sync_label.clear();
                        guard.sync_progress = None;
                        guard.battery_pct = None;
                        guard.battery_mv = None;
                        guard.battery_charging = false;
                        guard.firmware.clear();
                        guard.model.clear();
                        guard.hq_lat = None;
                        guard.hq_lon = None;
                        guard.tx_power = None;
                        guard.max_tx_power = None;
                        guard.radio_freq_mhz = None;
                        guard.radio_bw_khz = None;
                        guard.radio_sf = None;
                        guard.radio_cr = None;
                        guard.adv_loc_policy = None;
                        guard.manual_add_contacts = false;
                        guard.telemetry_mode_base = 0;
                        guard.telemetry_mode_loc = 0;
                        guard.telemetry_mode_env = 0;
                        guard.multi_acks = 0;
                        guard.path_hash_mode = None;
                        guard.public_key.clear();
                        guard.client_repeat = false;
                        guard.flood_scope = None;
                        guard.custom_vars.clear();
                        guard.connected_since = None;
                        guard.state = "reconnecting".into();
                        guard.detail = error;
                    }
                    if let Some(id) = radio_id(&db, &name) {
                        let _ = db.set_radio_error(id, &status_detail(&status));
                    }
                    tokio::select! {
                        _ = tokio::time::sleep(backoff) => {
                            backoff = (backoff * 2).min(Duration::from_secs(30));
                        }
                        cmd = rx.recv() => {
                            if !apply_idle_cmd(cmd, &db, &status, &mut desired).await {
                                break;
                            }
                        }
                    }
                }
            }
        } else {
            let cmd = rx.recv().await;
            if !apply_idle_cmd(cmd, &db, &status, &mut desired).await {
                break;
            }
        }
    }
}

fn status_detail(status: &Mutex<RadioStatus>) -> String {
    status
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .detail
        .clone()
}

fn radio_id(db: &Db, name: &str) -> Option<i64> {
    db.upsert_radio("ble", name, "Companion").ok()
}

fn scanned_label(status: &Mutex<RadioStatus>, target: &str) -> String {
    let guard = status.lock().unwrap_or_else(|err| err.into_inner());
    guard
        .devices
        .iter()
        .find(|device| {
            device.address.eq_ignore_ascii_case(target) || device.name.eq_ignore_ascii_case(target)
        })
        .and_then(|device| {
            let name = device.name.trim();
            if name.is_empty() || name == "Unnamed device" {
                None
            } else {
                Some(name.to_string())
            }
        })
        .unwrap_or_else(|| "Companion".into())
}

fn stable_radio_address(status: &Mutex<RadioStatus>, target: &str) -> String {
    let guard = status.lock().unwrap_or_else(|err| err.into_inner());
    guard
        .devices
        .iter()
        .find(|device| {
            device.address.eq_ignore_ascii_case(target) || device.name.eq_ignore_ascii_case(target)
        })
        .map(|device| device.address.clone())
        .unwrap_or_else(|| target.to_string())
}

/// Returns false when the command channel has closed.
async fn apply_idle_cmd(
    cmd: Option<RadioCmd>,
    db: &Db,
    status: &Mutex<RadioStatus>,
    desired: &mut Option<String>,
) -> bool {
    match cmd {
        None => false,
        Some(RadioCmd::Scan) => {
            write_status(status, "scanning", "Listening for nearby Bluetooth devices…");
            match discover_radios(Duration::from_secs(5)).await {
                Ok(devices) => {
                    let companions = devices.iter().filter(|device| device.companion).count();
                    let count = devices.len();
                    let mut guard = status.lock().unwrap_or_else(|err| err.into_inner());
                    guard.devices = devices;
                    guard.state = "disconnected".into();
                    guard.detail = if count == 0 {
                        "No Bluetooth devices answered. Confirm the adapter is on and the radio is powered.".into()
                    } else if companions == 0 {
                        format!("Found {count} devices. None advertised the MeshCore companion service. Match the upstairs radio by name and address.")
                    } else {
                        format!("Found {count} devices, {companions} advertising the MeshCore companion service.")
                    };
                }
                Err(err) => write_status(status, "error", err),
            }
            true
        }
        Some(RadioCmd::Pair { target, pin }) => {
            let target = target.trim().to_string();
            if target.is_empty() {
                write_status(status, "error", "Enter the Bluetooth name or address of the radio.");
            } else {
                write_status(
                    status,
                    "pairing",
                    "Pairing with the radio. Enter the PIN shown on the radio if asked by the system.",
                );
                match crate::ble_pair::ensure_paired(&target, pin).await {
                    Ok(()) => {
                        let address = stable_radio_address(status, &target);
                        let label = scanned_label(status, &target);
                        let _ = db.remember_radio("ble", &address, &label);
                        *desired = Some(address);
                    }
                    Err(err) => write_status(status, "error", err),
                }
            }
            true
        }
        Some(RadioCmd::Reconnect(target)) => {
            let target = target.trim().to_string();
            if target.is_empty() {
                write_status(status, "error", "Choose a saved companion radio first.");
            } else {
                let address = stable_radio_address(status, &target);
                let label = scanned_label(status, &target);
                let _ = db.remember_radio("ble", &address, &label);
                write_status(status, "connecting", format!("Reconnecting to {address}…"));
                *desired = Some(address);
            }
            true
        }
        Some(RadioCmd::Disconnect) => {
            *desired = None;
            write_status(status, "disconnected", "Disconnected.");
            true
        }
        Some(RadioCmd::ApplyName(_)) => {
            write_status(status, "disconnected", "Connect the radio before applying the advert name.");
            true
        }
        Some(RadioCmd::PushChannel { .. })
        | Some(RadioCmd::ClearChannel { .. })
        | Some(RadioCmd::SendChannel { .. }) => {
            write_status(
                status,
                "disconnected",
                "Connect the radio before changing channels or sending.",
            );
            true
        }
        Some(RadioCmd::RunCli { .. }) => {
            push_cli_line(
                status,
                "error: Connect the radio before running CLI commands.",
            );
            write_status(
                status,
                "disconnected",
                "Connect the radio before changing device settings.",
            );
            true
        }
        Some(RadioCmd::RefreshDeviceSettings)
        | Some(RadioCmd::SetCustomVar { .. })
        | Some(RadioCmd::SetCoords { .. })
        | Some(RadioCmd::SetTxPower(_))
        | Some(RadioCmd::SetRadioParams { .. })
        | Some(RadioCmd::SetPathHashMode(_))
        | Some(RadioCmd::SetAdvLocPolicy(_))
        | Some(RadioCmd::SetFloodScope(_))
        | Some(RadioCmd::SendAdvert { .. })
        | Some(RadioCmd::Reboot)
        | Some(RadioCmd::ProbeNoise) => {
            write_status(
                status,
                "disconnected",
                "Connect the radio before changing device settings.",
            );
            true
        }
    }
}

fn push_cli_line(status: &Mutex<RadioStatus>, line: impl Into<String>) {
    let mut guard = status.lock().unwrap_or_else(|err| err.into_inner());
    guard.cli_seq = guard.cli_seq.wrapping_add(1);
    guard.cli_line = line.into();
}

fn parse_pubkey_prefix(hex_key: &str) -> Result<[u8; 6], String> {
    let cleaned: String = hex_key
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .collect();
    if cleaned.len() < 12 {
        return Err("Contact key must be at least 12 hex characters.".into());
    }
    let bytes = hex::decode(&cleaned[..12]).map_err(|err| err.to_string())?;
    let mut prefix = [0u8; 6];
    prefix.copy_from_slice(&bytes);
    Ok(prefix)
}

fn cli_correlation_tag() -> String {
    // Firmware / mesh-client use two [a-z0-9] chars so replies can be matched.
    const ALPH: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let a = ALPH[(nanos % ALPH.len() as u128) as usize] as char;
    let b = ALPH[((nanos / 7) % ALPH.len() as u128) as usize] as char;
    format!("{a}{b}")
}

fn parse_cli_reply_bytes(bytes: &[u8], tag: &str) -> Option<String> {
    if bytes.is_empty() {
        return None;
    }
    let body = if bytes[0] == 29 {
        String::from_utf8_lossy(&bytes[1..]).to_string()
    } else if bytes[0].is_ascii_graphic() || bytes[0].is_ascii_whitespace() {
        String::from_utf8_lossy(bytes).to_string()
    } else {
        return None;
    };
    let body = body.trim();
    let prefix = format!("{tag}|");
    if let Some(rest) = body.strip_prefix(&prefix) {
        return Some(rest.trim().to_string());
    }
    // Older firmwares may echo the command without the correlation tag.
    Some(body.to_string())
}

/// Companion `CMD_RUN_CLI_COMMAND` (66). Reply is code 29 → EventType::Unknown + Bytes.
/// Wire body is `xx|command` (protocol v14+), matching mesh-client / firmware.
async fn apply_local_cli(
    commands: &meshcore_rs::commands::CommandHandler,
    text: &str,
) -> Result<String, String> {
    let command = text.trim_start();
    if command.is_empty() {
        return Err("Empty command.".into());
    }
    let tag = cli_correlation_tag();
    let framed = format!("{tag}|{command}");
    if framed.len() > 150 {
        return Err("CLI command too long (max ~148 bytes with tag).".into());
    }
    let mut data = vec![66u8];
    data.extend_from_slice(framed.as_bytes());
    let event = commands
        .send_multi(
            &data,
            &[EventType::Unknown, EventType::Error, EventType::Ok],
            Duration::from_secs(12),
        )
        .await
        .map_err(|err| {
            format!(
                "{err} — companion CLI needs firmware protocol v14+. Try `version` after updating."
            )
        })?;
    match event.event_type {
        EventType::Error => match event.payload {
            EventPayload::String(s) => Err(s),
            _ => Err("Companion rejected the CLI command (unsupported or busy).".into()),
        },
        EventType::Ok => Ok("(ok)".into()),
        _ => match event.payload {
            EventPayload::Bytes(bytes) => match parse_cli_reply_bytes(&bytes, &tag) {
                Some(s) if !s.is_empty() => Ok(s),
                Some(_) => Ok("(empty CLI reply)".into()),
                None => Ok(format!("raw reply: {}", hex::encode(bytes))),
            },
            EventPayload::String(s) => {
                let prefix = format!("{tag}|");
                Ok(s.strip_prefix(&prefix).unwrap_or(&s).trim().to_string())
            }
            _ => Ok("(empty CLI reply)".into()),
        },
    }
}

/// Remote CLI: CMD_SEND_TXT_MSG with TXT_TYPE_CLI_COMMAND = 3.
async fn apply_remote_cli(
    commands: &meshcore_rs::commands::CommandHandler,
    prefix: [u8; 6],
    text: &str,
) -> Result<String, String> {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as u32)
        .unwrap_or(0);
    let mut data = vec![0x02u8, 0x03, 0x00]; // SEND_TXT, CLI_COMMAND, attempt
    data.extend_from_slice(&ts.to_le_bytes());
    data.extend_from_slice(&prefix);
    data.extend_from_slice(text.as_bytes());
    commands
        .send_with_timeout(&data, Some(EventType::MsgSent), Duration::from_secs(10))
        .await
        .map_err(|err| err.to_string())?;
    Ok("CLI command sent to contact (replies arrive as mesh messages).".into())
}

enum SessionEnd {
    Stopped,
    Lost(String),
    Switch(String),
}

async fn run_session(
    db: &Arc<Db>,
    secrets: &Secrets,
    status: &Mutex<RadioStatus>,
    rx: &mut UnboundedReceiver<RadioCmd>,
    name: &str,
) -> SessionEnd {
    let meshcore = match MeshCore::ble_connect(name).await {
        Ok(meshcore) => meshcore,
        Err(err) => return SessionEnd::Lost(err.to_string()),
    };
    let radio = match db.upsert_radio("ble", name, "Companion") {
        Ok(id) => id,
        Err(err) => return SessionEnd::Lost(err.to_string()),
    };

    let max_channels;
    {
        let commands = meshcore.commands().lock().await;
        write_sync(status, SyncPhase::AppStart, "Reading companion identity…", None);
        match commands.send_appstart().await {
            Ok(info) => {
                let pubkey = hex::encode(info.public_key);
                let raw = json!({
                    "name": info.name,
                    "tx_power": info.tx_power,
                    "max_tx_power": info.max_tx_power,
                    "multi_acks": info.multi_acks,
                    "adv_loc_policy": info.adv_loc_policy,
                    "telemetry_base": info.telemetry_mode_base,
                    "telemetry_loc": info.telemetry_mode_loc,
                    "telemetry_env": info.telemetry_mode_env,
                    "manual_add_contacts": info.manual_add_contacts,
                    "freq": info.radio_freq,
                    "bw": info.radio_bw,
                    "sf": info.sf,
                    "cr": info.cr,
                });
                if let Err(err) = db.save_hq(&pubkey, &info.name, info.adv_lat, info.adv_lon, &raw) {
                    return SessionEnd::Lost(err.to_string());
                }
                let label = if info.name.trim().is_empty() {
                    name
                } else {
                    info.name.trim()
                };
                if !info.name.trim().is_empty() {
                    let _ = db.set_setting("hq_name", info.name.trim());
                }
                let _ = db.remember_radio("ble", name, label);
                let (lat, lon) = crate::db::coords_from_micro(info.adv_lat, info.adv_lon);
                let mut guard = status.lock().unwrap_or_else(|err| err.into_inner());
                guard.hq_lat = lat;
                guard.hq_lon = lon;
                guard.tx_power = Some(info.tx_power);
                guard.max_tx_power = Some(info.max_tx_power);
                guard.radio_freq_mhz = Some(info.radio_freq as f64 / 1000.0);
                guard.radio_bw_khz = Some(info.radio_bw as f64 / 1000.0);
                guard.radio_sf = Some(info.sf);
                guard.radio_cr = Some(info.cr);
                guard.adv_loc_policy = Some(info.adv_loc_policy);
                guard.manual_add_contacts = info.manual_add_contacts;
                guard.telemetry_mode_base = info.telemetry_mode_base;
                guard.telemetry_mode_loc = info.telemetry_mode_loc;
                guard.telemetry_mode_env = info.telemetry_mode_env;
                guard.multi_acks = info.multi_acks;
                guard.public_key = pubkey;
            }
            Err(err) => return SessionEnd::Lost(err.to_string()),
        }
        write_sync(status, SyncPhase::AppStart, "Querying device info…", None);
        max_channels = match commands.send_device_query().await {
            Ok(info) => {
                // Companion firmwares report up to ~40 slots; clamp so a bad
                // value cannot queue BLE with dozens of timed-out queries.
                let max_channels = info.max_channels.unwrap_or(8).clamp(1, 40);
                let firmware = info.version.clone().unwrap_or_default();
                let model = info.model.clone().unwrap_or_default();
                let raw = json!({
                    "firmware_code": info.fw_version_code,
                    "firmware": info.version,
                    "build": info.fw_build,
                    "model": info.model,
                    "max_contacts": info.max_contacts,
                    "max_channels": info.max_channels,
                    "repeat": info.repeat,
                });
                let _ = db.save_device_info(radio, &raw);
                let mut guard = status.lock().unwrap_or_else(|err| err.into_inner());
                guard.firmware = firmware;
                guard.model = model;
                if let Some(repeat) = info.repeat {
                    guard.client_repeat = repeat;
                }
                max_channels
            }
            Err(err) => return SessionEnd::Lost(err.to_string()),
        };
        let _ = commands.set_time(unix_now() as u32).await;
        {
            let mut guard = status.lock().unwrap_or_else(|err| err.into_inner());
            guard.connected_name = name.to_string();
        }
        if let Ok(battery) = commands.get_bat().await {
            apply_battery(db, status, &battery);
            write_status(status, "connected", connected_detail(status, name));
        } else {
            write_status(status, "connected", format!("Connected to {name}."));
        }
        write_sync(
            status,
            SyncPhase::Contacts,
            "Fetching contacts (BLE can take a few minutes)…",
            None,
        );
        // Contact lists over BLE can outlast the default 5s command timeout.
        // Keep the session if sync is slow or empty; retry from the live loop.
        if let Err(err) = sync_contacts(db, &commands, Some(status)).await {
            eprintln!("contact sync: {err}");
            write_status(
                status,
                "connected",
                format!("Connected to {name}. Contact sync will retry ({err})."),
            );
        }
        write_sync(
            status,
            SyncPhase::Channels,
            "Syncing channels…",
            Some((0, max_channels as u32)),
        );
        match sync_channels(db, secrets, &commands, max_channels, status).await {
            Ok(()) => {}
            Err(err) if is_ble_link_error(&err) => {
                eprintln!("channel sync: {err}");
                return SessionEnd::Lost(err);
            }
            Err(err) => eprintln!("channel sync: {err}"),
        }
        if let Err(err) = push_channel(db, secrets, &commands, max_channels).await {
            eprintln!("channel setup: {err}");
            if is_ble_link_error(&err) {
                return SessionEnd::Lost(err);
            }
        }
        // Cap the initial queue drain. On a busy Public channel, new RF traffic
        // can refill the companion queue forever — holding this lock for hours
        // blocks Ready / CLI / settings while the UI says "Draining…".
        write_sync(
            status,
            SyncPhase::Messages,
            "Draining queued messages…",
            Some((0, 1)),
        );
        let drain_deadline = Instant::now() + Duration::from_secs(60);
        let mut drained = 0u32;
        const DRAIN_MAX: u32 = 400;
        loop {
            if drained >= DRAIN_MAX || Instant::now() >= drain_deadline {
                write_sync(
                    status,
                    SyncPhase::Messages,
                    format!(
                        "Queued drain paused after {drained} msg(s); live fetch continues…"
                    ),
                    Some((drained, drained.max(1))),
                );
                break;
            }
            match tokio::time::timeout(Duration::from_secs(4), commands.get_msg()).await {
                Ok(Ok(Some(event))) => {
                    drained = drained.saturating_add(1);
                    write_sync(
                        status,
                        SyncPhase::Messages,
                        format!("Draining queued messages… ({drained})"),
                        Some((drained, (drained + 8).max(1))),
                    );
                    if let Err(err) = handle_event(db, &meshcore, event, radio).await {
                        eprintln!("message: {err}");
                    }
                }
                Ok(Ok(None)) => break,
                Ok(Err(err)) => return SessionEnd::Lost(err.to_string()),
                Err(_) => {
                    // No reply in time — treat as empty / move on to live mode.
                    break;
                }
            }
        }
        write_sync(status, SyncPhase::Ready, "Companion ready", None);
        {
            let mut guard = status.lock().unwrap_or_else(|err| err.into_inner());
            if guard.connected_since.is_none() {
                guard.connected_since = Some(unix_now());
            }
        }
        {
            let commands = meshcore.commands().lock().await;
            refresh_custom_vars(&commands, status).await;
        }
        write_status(status, "connected", connected_detail(status, name));
    }

    let (event_tx, mut event_rx) = unbounded_channel();
    let mut subscriptions = Vec::new();
    for kind in [
        EventType::ContactMsgRecv,
        EventType::ChannelMsgRecv,
        EventType::Advertisement,
        EventType::PathUpdate,
        EventType::Ack,
        EventType::Disconnected,
        EventType::MessagesWaiting,
    ] {
        let event_tx = event_tx.clone();
        let subscription = meshcore
            .subscribe(kind, HashMap::new(), move |event| {
                let _ = event_tx.send(event);
            })
            .await;
        subscriptions.push(subscription);
    }
    drop(event_tx);
    meshcore.start_auto_message_fetching().await;
    let _keep = subscriptions;

    let mut replies: HashMap<String, Instant> = HashMap::new();
    let mut pushed_generation = 0i64;
    let mut last_battery = Instant::now();
    let mut last_advert_sync = Instant::now() - Duration::from_secs(120);
    let mut contact_retry_at = Instant::now() + Duration::from_secs(15);
    let mut need_contact_retry = db
        .setting("contacts_since")
        .ok()
        .flatten()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(0)
        == 0;
    // Persistent interval — recreating `sleep(1s)` inside select! starves battery
    // polls whenever mesh events keep arriving (auto-fetch / advertisements).
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        // Prefer UI/radio commands over mesh event floods so CLI/settings stay responsive.
        tokio::select! {
            biased;
            cmd = rx.recv() => {
                match cmd {
                    None | Some(RadioCmd::Disconnect) => {
                        let _ = meshcore.disconnect().await;
                        return SessionEnd::Stopped;
                    }
                    Some(RadioCmd::Pair { target: next, pin }) if next.trim() != name => {
                        let next = next.trim().to_string();
                        write_status(status, "pairing", "Pairing with the next radio…");
                        if let Err(err) = crate::ble_pair::ensure_paired(&next, pin).await {
                            write_status(status, "connected", err);
                            continue;
                        }
                        let _ = db.remember_radio("ble", &next, "Companion");
                        let _ = meshcore.disconnect().await;
                        return SessionEnd::Switch(next);
                    }
                    Some(RadioCmd::Reconnect(next)) if next.trim() != name => {
                        let next = next.trim().to_string();
                        let _ = db.set_setting("radio_name", &next);
                        let _ = meshcore.disconnect().await;
                        return SessionEnd::Switch(next);
                    }
                    Some(RadioCmd::ApplyName(advert)) => {
                        let advert = advert.trim().to_string();
                        if !advert.is_empty() {
                            let commands = meshcore.commands().lock().await;
                            if let Err(err) = commands.set_name(&advert).await {
                                write_status(status, "connected", err.to_string());
                            } else {
                                let _ = db.set_setting("hq_name", &advert);
                                write_status(status, "connected", format!("Advert name set to {advert}."));
                            }
                        }
                    }
                    Some(RadioCmd::PushChannel { slot, name: channel_name }) => {
                        let commands = meshcore.commands().lock().await;
                        if let Err(err) =
                            push_named_channel(db, secrets, &commands, slot, &channel_name).await
                        {
                            write_status(status, "connected", err);
                        } else {
                            write_status(
                                status,
                                "connected",
                                format!("Channel {channel_name} written to radio slot {slot}."),
                            );
                        }
                    }
                    Some(RadioCmd::ClearChannel { slot }) => {
                        let commands = meshcore.commands().lock().await;
                        let blank = [0u8; 16];
                        if let Err(err) = commands.set_channel(slot, "", &blank).await {
                            write_status(status, "connected", err.to_string());
                        } else {
                            write_status(
                                status,
                                "connected",
                                format!("Cleared radio channel slot {slot}."),
                            );
                        }
                    }
                    Some(RadioCmd::SendChannel { slot, text }) => {
                        let text = text.trim().to_string();
                        if text.is_empty() {
                            continue;
                        }
                        let advert_name = db.hq_advert_name().unwrap_or_else(|_| "hq".into());
                        let wire = prefix_channel_sender(&advert_name, &text);
                        if wire.len() > 160 {
                            write_status(
                                status,
                                "connected",
                                "Mesh text is limited to 160 bytes after adding the device name.",
                            );
                            continue;
                        }
                        let commands = meshcore.commands().lock().await;
                        match commands
                            .send_channel_msg(slot, &wire, Some(unix_now() as u32))
                            .await
                        {
                            Ok(()) => {
                                let raw = json!({
                                    "channel": slot,
                                    "text": wire,
                                    "direction": "tx",
                                });
                                let _ = db.insert_message(
                                    "channel",
                                    "tx",
                                    None,
                                    &advert_name,
                                    Some(slot as i64),
                                    Some(0),
                                    unix_now() as u32,
                                    &wire,
                                    &wire,
                                    None,
                                    None,
                                    &raw,
                                    "sent",
                                    Some(radio),
                                );
                            }
                            Err(err) => write_status(status, "connected", err.to_string()),
                        }
                    }
                    Some(RadioCmd::RefreshDeviceSettings) => {
                        let commands = meshcore.commands().lock().await;
                        refresh_custom_vars(&commands, status).await;
                        write_status(
                            status,
                            "connected",
                            "Device settings refreshed from the companion.",
                        );
                    }
                    Some(RadioCmd::SetCustomVar { key, value }) => {
                        let key = key.trim().to_string();
                        if key.is_empty() {
                            continue;
                        }
                        let commands = meshcore.commands().lock().await;
                        match apply_custom_var(&commands, &key, &value).await {
                            Ok(()) => {
                                {
                                    let mut guard =
                                        status.lock().unwrap_or_else(|err| err.into_inner());
                                    guard.custom_vars.insert(key.clone(), value.clone());
                                }
                                write_status(
                                    status,
                                    "connected",
                                    format!("Set {key}={value} on the companion."),
                                );
                            }
                            Err(err) => write_status(status, "connected", err),
                        }
                    }
                    Some(RadioCmd::SetCoords { lat, lon }) => {
                        let commands = meshcore.commands().lock().await;
                        match commands.set_coords(lat, lon).await {
                            Ok(_) => {
                                {
                                    let mut guard =
                                        status.lock().unwrap_or_else(|err| err.into_inner());
                                    guard.hq_lat = Some(lat);
                                    guard.hq_lon = Some(lon);
                                }
                                let _ = db.update_hq_coords(lat, lon);
                                write_status(
                                    status,
                                    "connected",
                                    format!("Advert location set to {lat:.5}, {lon:.5}."),
                                );
                            }
                            Err(err) => write_status(status, "connected", err.to_string()),
                        }
                    }
                    Some(RadioCmd::SetTxPower(power)) => {
                        let commands = meshcore.commands().lock().await;
                        match commands.set_tx_power(power).await {
                            Ok(_) => {
                                {
                                    let mut guard =
                                        status.lock().unwrap_or_else(|err| err.into_inner());
                                    guard.tx_power = Some(power);
                                }
                                write_status(
                                    status,
                                    "connected",
                                    format!("TX power set to {power} dBm."),
                                );
                            }
                            Err(err) => write_status(status, "connected", err.to_string()),
                        }
                    }
                    Some(RadioCmd::SetRadioParams {
                        freq_mhz,
                        bw_khz,
                        sf,
                        cr,
                        repeat,
                    }) => {
                        let commands = meshcore.commands().lock().await;
                        match apply_radio_params(&commands, freq_mhz, bw_khz, sf, cr, repeat).await
                        {
                            Ok(()) => {
                                {
                                    let mut guard =
                                        status.lock().unwrap_or_else(|err| err.into_inner());
                                    guard.radio_freq_mhz = Some(freq_mhz);
                                    guard.radio_bw_khz = Some(bw_khz);
                                    guard.radio_sf = Some(sf);
                                    guard.radio_cr = Some(cr);
                                    guard.client_repeat = repeat;
                                }
                                let mut detail = format!(
                                    "Radio set to {freq_mhz:.3} MHz SF{sf} BW{bw_khz} CR{cr}."
                                );
                                if let Some(mode) =
                                    crate::radio_presets::take_pending_path_hash_mode()
                                {
                                    match apply_path_hash_mode(&commands, mode).await {
                                        Ok(()) => {
                                            let mut guard = status
                                                .lock()
                                                .unwrap_or_else(|err| err.into_inner());
                                            guard.path_hash_mode = Some(mode);
                                            detail.push_str(&format!(
                                                " Path hash → {}-byte.",
                                                mode + 1
                                            ));
                                        }
                                        Err(err) => {
                                            crate::radio_presets::queue_path_hash_mode(mode);
                                            detail.push_str(&format!(" Path hash pending: {err}"));
                                        }
                                    }
                                }
                                write_status(status, "connected", detail);
                            }
                            Err(err) => write_status(status, "connected", err),
                        }
                    }
                    Some(RadioCmd::SetPathHashMode(mode)) => {
                        let commands = meshcore.commands().lock().await;
                        match apply_path_hash_mode(&commands, mode).await {
                            Ok(()) => {
                                {
                                    let mut guard =
                                        status.lock().unwrap_or_else(|err| err.into_inner());
                                    guard.path_hash_mode = Some(mode);
                                }
                                write_status(
                                    status,
                                    "connected",
                                    format!("Path hash mode set to {}-byte.", mode + 1),
                                );
                            }
                            Err(err) => write_status(status, "connected", err),
                        }
                    }
                    Some(RadioCmd::SetAdvLocPolicy(policy)) => {
                        let commands = meshcore.commands().lock().await;
                        match apply_adv_loc_policy(&commands, status, policy).await {
                            Ok(()) => {
                                {
                                    let mut guard =
                                        status.lock().unwrap_or_else(|err| err.into_inner());
                                    guard.adv_loc_policy = Some(policy);
                                }
                                write_status(
                                    status,
                                    "connected",
                                    format!(
                                        "Share position: {}.",
                                        crate::radio_presets::loc_policy_label(policy)
                                    ),
                                );
                            }
                            Err(err) => write_status(status, "connected", err),
                        }
                    }
                    Some(RadioCmd::SetFloodScope(scope)) => {
                        let commands = meshcore.commands().lock().await;
                        match commands.set_flood_scope(scope.as_deref()).await {
                            Ok(()) => {
                                {
                                    let mut guard =
                                        status.lock().unwrap_or_else(|err| err.into_inner());
                                    guard.flood_scope = scope.clone();
                                }
                                let detail = match scope.as_deref() {
                                    Some(scope) if !scope.is_empty() => {
                                        format!("Flood scope set to {scope}.")
                                    }
                                    _ => "Flood scope cleared.".into(),
                                };
                                write_status(status, "connected", detail);
                            }
                            Err(err) => write_status(status, "connected", err.to_string()),
                        }
                    }
                    Some(RadioCmd::SendAdvert { flood }) => {
                        let commands = meshcore.commands().lock().await;
                        match commands.send_advert(flood).await {
                            Ok(_) => {
                                let kind = if flood { "Flood advert" } else { "Advert" };
                                write_status(
                                    status,
                                    "connected",
                                    format!("{kind} sent from the companion."),
                                );
                            }
                            Err(err) => write_status(status, "connected", err.to_string()),
                        }
                    }
                    Some(RadioCmd::RunCli { contact_hex, text }) => {
                        let text = text.trim().to_string();
                        if text.is_empty() {
                            push_cli_line(status, "Empty command.");
                            continue;
                        }
                        let commands = meshcore.commands().lock().await;
                        let result = if contact_hex.trim().is_empty() {
                            apply_local_cli(&commands, &text).await
                        } else {
                            match parse_pubkey_prefix(&contact_hex) {
                                Ok(prefix) => apply_remote_cli(&commands, prefix, &text).await,
                                Err(err) => Err(err),
                            }
                        };
                        match result {
                            Ok(reply) => {
                                // UI already echoes `> cmd`; push reply body only.
                                push_cli_line(status, reply);
                                write_status(status, "connected", "CLI command completed.");
                            }
                            Err(err) => {
                                push_cli_line(status, format!("error: {err}"));
                                write_status(status, "connected", err);
                            }
                        }
                    }
                    Some(RadioCmd::ProbeNoise) => {
                        let contacts = {
                            let nodes = db.list_nodes().unwrap_or_default();
                            nodes
                                .into_iter()
                                .filter(|n| n.has_gps && n.enrollment.as_str() != "hq")
                                .take(8)
                                .collect::<Vec<_>>()
                        };
                        if contacts.is_empty() {
                            write_status(
                                status,
                                "connected",
                                "No GPS contacts to probe for noise floor.",
                            );
                            continue;
                        }
                        let commands = meshcore.commands().lock().await;
                        let mut ok = 0usize;
                        for node in contacts {
                            let prefix = match parse_pubkey_prefix(&node.pubkey) {
                                Ok(p) => p,
                                Err(_) => continue,
                            };
                            match commands
                                .request_status(Destination::Bytes(prefix.to_vec()))
                                .await
                            {
                                Ok(info) => {
                                    let _ = db.insert_rf_sample(
                                        &node.pubkey,
                                        info.noise_floor,
                                        info.last_rssi,
                                        info.snr,
                                    );
                                    ok += 1;
                                }
                                Err(err) => {
                                    eprintln!("noise probe {}: {err}", node.name);
                                }
                            }
                        }
                        write_status(
                            status,
                            "connected",
                            format!("Noise probe stored {ok} sample(s)."),
                        );
                    }
                    Some(RadioCmd::Reboot) => {
                        let commands = meshcore.commands().lock().await;
                        match commands.reboot().await {
                            Ok(()) => {
                                write_status(
                                    status,
                                    "reconnecting",
                                    "Companion reboot requested…",
                                );
                                let _ = meshcore.disconnect().await;
                                return SessionEnd::Lost("Companion rebooting.".into());
                            }
                            Err(err) => write_status(status, "connected", err.to_string()),
                        }
                    }
                    Some(RadioCmd::Scan)
                    | Some(RadioCmd::Pair { .. })
                    | Some(RadioCmd::Reconnect(_)) => {}
                }
            }
            event = event_rx.recv() => {
                match event {
                    None => return SessionEnd::Lost("Radio event stream ended.".into()),
                    Some(event) if event.event_type == EventType::Disconnected => {
                        return SessionEnd::Lost("Bluetooth link closed.".into());
                    }
                    Some(event) => {
                        // Do NOT contact-sync on MessagesWaiting — auto-fetch already
                        // drains messages, and a 30s get_contacts held the command lock
                        // so CLI/settings never ran on a busy mesh.
                        if event.event_type == EventType::Advertisement
                            && last_advert_sync.elapsed() >= Duration::from_secs(60)
                        {
                            last_advert_sync = Instant::now();
                            let commands = meshcore.commands().lock().await;
                            if let Err(err) = sync_contacts(db, &commands, None).await {
                                eprintln!("contact sync: {err}");
                            }
                        }
                        if let Err(err) = handle_event(db, &meshcore, event, radio).await {
                            eprintln!("event: {err}");
                        }
                    }
                }
            }
            _ = tick.tick() => {
                if let Err(err) = flush_one_outbound(db, &meshcore, &mut replies).await {
                    eprintln!("send: {err}");
                }
                let generation = db.channel_generation().unwrap_or(0);
                if generation != pushed_generation {
                    let commands = meshcore.commands().lock().await;
                    match push_channel(db, secrets, &commands, max_channels).await {
                        Ok(true) => pushed_generation = generation,
                        Ok(false) => {}
                        Err(err) => eprintln!("channel: {err}"),
                    }
                }
                if last_battery.elapsed() >= Duration::from_secs(15) {
                    let commands = meshcore.commands().lock().await;
                    match commands.get_bat().await {
                        Ok(battery) => apply_battery(db, status, &battery),
                        Err(err) => eprintln!("battery poll: {err}"),
                    }
                    last_battery = Instant::now();
                }
                // If initial contact dump never finished (contacts_since still 0),
                // retry once the live loop is up so we don't block connect forever.
                if need_contact_retry && Instant::now() >= contact_retry_at {
                    contact_retry_at = Instant::now() + Duration::from_secs(120);
                    let since_now = db
                        .setting("contacts_since")
                        .ok()
                        .flatten()
                        .and_then(|v| v.parse::<u32>().ok())
                        .unwrap_or(0);
                    if since_now == 0 {
                        write_sync(
                            status,
                            SyncPhase::Contacts,
                            "Retrying contact sync…",
                            None,
                        );
                        let commands = meshcore.commands().lock().await;
                        match sync_contacts(db, &commands, Some(status)).await {
                            Ok(()) => {
                                need_contact_retry = false;
                                write_sync(status, SyncPhase::Ready, "Companion ready", None);
                                write_status(status, "connected", connected_detail(status, name));
                            }
                            Err(err) => {
                                eprintln!("contact sync retry: {err}");
                                write_sync(status, SyncPhase::Ready, "Companion ready", None);
                                write_status(
                                    status,
                                    "connected",
                                    format!(
                                        "{}. Contact sync still pending.",
                                        connected_detail(status, name)
                                    ),
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

async fn sync_contacts(
    db: &Db,
    commands: &meshcore_rs::commands::CommandHandler,
    status: Option<&Mutex<RadioStatus>>,
) -> Result<(), String> {
    let since = db
        .setting("contacts_since")
        .map_err(|err| err.to_string())?
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    // Full dumps of hundreds of contacts over BLE routinely exceed 30s.
    let contacts = commands
        .get_contacts_with_timeout(since, Duration::from_secs(180))
        .await
        .map_err(|err| err.to_string())?;
    let total = contacts.len() as u32;
    if let Some(status) = status {
        write_sync(
            status,
            SyncPhase::Contacts,
            if total == 0 {
                "No contacts to sync"
            } else {
                "Syncing contacts…"
            },
            Some((0, total.max(1))),
        );
    }
    let mut newest = since;
    for (idx, contact) in contacts.into_iter().enumerate() {
        newest = newest.max(contact.last_modification_timestamp);
        let pubkey = contact.public_key_hex();
        let hop_bytes = if contact.path_len > 0 {
            contact
                .out_path
                .iter()
                .take(contact.path_len as usize)
                .copied()
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let raw = contact_snapshot(
            &contact.adv_name,
            contact.contact_type,
            contact.flags,
            contact.path_len as i64,
            contact.last_advert as i64,
            contact.adv_lat,
            contact.adv_lon,
            contact.last_modification_timestamp as i64,
        );
        db.upsert_contact(
            &pubkey,
            &contact.adv_name,
            contact.contact_type,
            contact.flags,
            contact.path_len as i64,
            &hop_bytes,
            contact.last_advert as i64,
            contact.adv_lat,
            contact.adv_lon,
            contact.last_modification_timestamp as i64,
            &raw,
        )
        .map_err(|err| err.to_string())?;
        if let Some(status) = status {
            write_sync(
                status,
                SyncPhase::Contacts,
                "Syncing contacts…",
                Some((idx as u32 + 1, total.max(1))),
            );
        }
    }
    if let Some(status) = status {
        write_sync(
            status,
            SyncPhase::Contacts,
            if total == 1 {
                "Synced 1 contact".into()
            } else {
                format!("Synced {total} contacts")
            },
            Some((total.max(1), total.max(1))),
        );
    }
    db.set_setting("contacts_since", &newest.to_string())
        .map_err(|err| err.to_string())?;
    Ok(())
}

fn is_ble_link_error(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    lower.contains("not connected")
        || lower.contains("disconnected")
        || lower.contains("ble tx")
        || lower.contains("link closed")
}

fn is_channel_query_timeout(err: &str) -> bool {
    err.to_ascii_lowercase().contains("timeout")
}

async fn sync_channels(
    db: &Db,
    secrets: &Secrets,
    commands: &meshcore_rs::commands::CommandHandler,
    max_channels: u8,
    status: &Mutex<RadioStatus>,
) -> Result<(), String> {
    // Prefer a short discovery window on BLE; also refresh any higher slots we
    // already track locally so custom channels beyond the window still update.
    let discovery = max_channels.min(16);
    let mut slots: Vec<u8> = (0..discovery).collect();
    if let Ok(existing) = db.list_channels() {
        for row in existing {
            if row.slot >= 0 && row.slot <= u8::MAX as i64 {
                let slot = row.slot as u8;
                if slot < max_channels && !slots.contains(&slot) {
                    slots.push(slot);
                }
            }
        }
    }
    slots.sort_unstable();
    slots.dedup();
    let total = slots.len() as u32;
    let mut completed = 0u32;
    write_sync(
        status,
        SyncPhase::Channels,
        "Syncing channels…",
        Some((0, total.max(1))),
    );
    for (step, idx) in slots.into_iter().enumerate() {
        write_sync(
            status,
            SyncPhase::Channels,
            format!("Syncing channels… slot {idx}"),
            Some((step as u32, total.max(1))),
        );
        match commands.get_channel(idx).await {
            Ok(info) => {
                completed = step as u32 + 1;
                let name = info.name.trim();
                if !name.is_empty() {
                    let _ = db.upsert_channel_from_radio(name, idx as i64, &info.secret, secrets);
                }
                write_sync(
                    status,
                    SyncPhase::Channels,
                    "Syncing channels…",
                    Some((completed, total.max(1))),
                );
            }
            Err(err) => {
                let msg = err.to_string();
                eprintln!("get_channel({idx}): {msg}");
                if is_ble_link_error(&msg) {
                    return Err(format!("Channel sync stopped at slot {idx}: {msg}"));
                }
                // Timeout on a later slot usually means the table ended or the
                // radio stalled — keep what we have instead of hammering BLE.
                if is_channel_query_timeout(&msg) {
                    break;
                }
                return Err(format!("Channel sync stopped at slot {idx}: {msg}"));
            }
        }
    }
    write_sync(
        status,
        SyncPhase::Channels,
        if completed == 1 {
            "Synced 1 channel".into()
        } else {
            format!("Synced {completed} channels")
        },
        Some((completed.min(total.max(1)), total.max(1))),
    );
    Ok(())
}

async fn push_channel(
    db: &Db,
    secrets: &Secrets,
    commands: &meshcore_rs::commands::CommandHandler,
    max_channels: u8,
) -> Result<bool, String> {
    let Some(secret) = secrets.load(CHANNEL_NAME).map_err(|err| err)? else {
        return Ok(false);
    };
    let slot = if max_channels > 1 { 1 } else { 0 };
    commands
        .set_channel(slot, CHANNEL_NAME, &secret)
        .await
        .map_err(|err| err.to_string())?;
    let _ = db.ensure_channel_row(CHANNEL_NAME, slot as i64, "private", "emergency");
    db.set_setting("channel_slot", &slot.to_string())
        .map_err(|err| err.to_string())?;
    Ok(true)
}

async fn push_named_channel(
    db: &Db,
    secrets: &Secrets,
    commands: &meshcore_rs::commands::CommandHandler,
    slot: u8,
    name: &str,
) -> Result<(), String> {
    let secret = if name.eq_ignore_ascii_case(CHANNEL_NAME) {
        db.ensure_channel_secret(secrets)
            .map_err(|err| err.to_string())?
    } else if let Some(existing) = secrets.load(name).map_err(|err| err.to_string())? {
        existing
    } else if name.eq_ignore_ascii_case("Public") {
        [0u8; 16]
    } else {
        let generated = Secrets::generate();
        secrets
            .store(name, &generated)
            .map_err(|err| err.to_string())?;
        generated
    };
    commands
        .set_channel(slot, name, &secret)
        .await
        .map_err(|err| err.to_string())?;
    Ok(())
}

async fn handle_event(
    db: &Arc<Db>,
    meshcore: &MeshCore,
    event: MeshCoreEvent,
    radio_id: i64,
) -> Result<(), String> {
    match event.payload {
        EventPayload::ContactMessage(message) => {
            on_direct(db, meshcore, message, radio_id).await?;
        }
        EventPayload::ChannelMessage(message) => {
            let hops = hop_count_from_path_len(message.path_len).map(i64::from);
            let (sender_name, body) = channel_sender_and_body(&message.text);
            let sender_prefix = sender_name
                .clone()
                .unwrap_or_else(|| format!("ch{}", message.channel_idx));
            let display = if let Some(name) = sender_name.as_ref() {
                format!("{name}: {body}")
            } else {
                message.text.clone()
            };
            let raw = json!({
                "channel": message.channel_idx,
                "path_len": message.path_len,
                "txt_type": message.txt_type,
                "timestamp": message.sender_timestamp,
                "snr": message.snr,
                "text": message.text,
                "sender_name": sender_name,
                "body": body,
            });
            db.insert_message(
                "channel",
                "rx",
                None,
                &sender_prefix,
                Some(message.channel_idx as i64),
                hops,
                message.sender_timestamp,
                &message.text,
                &display,
                message.snr.map(f64::from),
                None,
                &raw,
                "received",
                Some(radio_id),
            )
            .map_err(|err| err.to_string())?;
        }
        EventPayload::PathUpdate(update) => {
            let prefix = hex::encode(update.prefix);
            let nodes = db.nodes_by_prefix(&prefix).map_err(|err| err.to_string())?;
            if nodes.len() == 1 {
                db.set_route(&nodes[0].pubkey, &update.path)
                    .map_err(|err| err.to_string())?;
                db.save_path(None, Some(&nodes[0].pubkey), "path_update", &update.path, 1)
                    .map_err(|err| err.to_string())?;
            }
        }
        EventPayload::Ack { tag } => {
            db.mark_ack(&hex::encode(tag))
                .map_err(|err| err.to_string())?;
        }
        _ => {}
    }
    Ok(())
}

async fn on_direct(
    db: &Db,
    meshcore: &MeshCore,
    message: ContactMessage,
    radio_id: i64,
) -> Result<(), String> {
    let prefix = message.sender_prefix_hex();
    let matches = db.nodes_by_prefix(&prefix).map_err(|err| err.to_string())?;
    let unique = matches.len() == 1;
    let current = matches
        .first()
        .map(|node| node.enrollment)
        .unwrap_or(crate::signup::Enrollment::Heard);
    let command = parse_command(&message.text);
    let decision = decide(&command, current, unique);
    let pubkey = unique.then(|| matches[0].pubkey.clone());
    let hops = hop_count_from_path_len(message.path_len).map(i64::from);
    let raw = json!({
        "prefix": prefix,
        "path_len": message.path_len,
        "txt_type": message.txt_type,
        "timestamp": message.sender_timestamp,
        "snr": message.snr,
        "signature": message.signature.map(hex::encode),
        "text": message.text,
    });
    let inserted = db
        .insert_message(
            "direct",
            "rx",
            pubkey.as_deref(),
            &prefix,
            None,
            hops,
            message.sender_timestamp,
            &message.text,
            &message.text,
            message.snr.map(f64::from),
            message.signature.as_ref().map(|bytes| bytes.as_slice()),
            &raw,
            "received",
            Some(radio_id),
        )
        .map_err(|err| err.to_string())?;
    let Some(message_id) = inserted else {
        return Ok(());
    };
    if let Some(pubkey) = pubkey {
        db.set_enrollment(&pubkey, decision.enrollment, decision.requested_name.clone())
            .map_err(|err| err.to_string())?;
        remember_route(db, meshcore, &pubkey, message_id).await;
    }
    if let Some(reply) = decision.reply {
        if let Some(pubkey) = matches.first().map(|node| node.pubkey.clone()) {
            db.queue_text(&pubkey, &reply, &reply)
                .map_err(|err| err.to_string())?;
        }
    }
    Ok(())
}

async fn remember_route(db: &Db, meshcore: &MeshCore, pubkey_hex: &str, message_id: i64) {
    let Ok(bytes) = hex::decode(pubkey_hex) else {
        return;
    };
    if bytes.len() != 32 {
        return;
    }
    let mut frame = Vec::with_capacity(34);
    frame.push(0x2A);
    frame.push(0x00);
    frame.extend_from_slice(&bytes);
    let commands = meshcore.commands().lock().await;
    if let Ok(event) = commands
        .send_multi(
            &frame,
            &[EventType::Unknown, EventType::Error],
            Duration::from_secs(2),
        )
        .await
    {
        let raw = match event.payload {
            EventPayload::Bytes(raw) => raw,
            _ => Vec::new(),
        };
        if let Some((path, hash_size)) = parse_advert_path(&raw) {
            let _ = db.save_path(
                Some(message_id),
                Some(pubkey_hex),
                "advert",
                &path,
                hash_size as i64,
            );
            return;
        }
    }
    if let Ok(nodes) = db.nodes_by_prefix(&pubkey_hex[..12.min(pubkey_hex.len())]) {
        if let Some(node) = nodes.into_iter().find(|node| node.pubkey == pubkey_hex) {
            if !node.hop_bytes.is_empty() {
                let _ = db.save_path(
                    Some(message_id),
                    Some(pubkey_hex),
                    "out_path",
                    &node.hop_bytes,
                    1,
                );
            }
        }
    }
}

async fn flush_one_outbound(
    db: &Arc<Db>,
    meshcore: &MeshCore,
    replies: &mut HashMap<String, Instant>,
) -> Result<(), String> {
    let Some(outbound) = db.next_outbound().map_err(|err| err.to_string())? else {
        return Ok(());
    };
    let automatic = outbound.body != "[secret sent]" && !outbound.body.starts_with("Approved.");
    if automatic {
        if let Some(last) = replies.get(&outbound.pubkey) {
            if last.elapsed() < REPLY_GAP {
                return Ok(());
            }
        }
    }
    let commands = meshcore.commands().lock().await;
    match commands
        .send_msg(Destination::Hex(outbound.pubkey.clone()), &outbound.body, None)
        .await
    {
        Ok(info) => {
            let ack = hex::encode(info.expected_ack);
            let display = if outbound.body.starts_with("Approved.") {
                "[secret sent]"
            } else {
                outbound.body.as_str()
            };
            db.mark_outbound_sent(outbound.id, display, &ack, "sent")
                .map_err(|err| err.to_string())?;
            let prefix = outbound.pubkey.chars().take(12).collect::<String>();
            let raw = json!({
                "expected_ack": ack,
                "message_type": info.message_type,
                "timeout_ms": info.suggested_timeout,
            });
            let _ = db.insert_message(
                "direct",
                "tx",
                Some(&outbound.pubkey),
                &prefix,
                None,
                None,
                unix_now() as u32,
                display,
                display,
                None,
                None,
                &raw,
                "sent",
                None,
            );
            if automatic {
                replies.insert(outbound.pubkey.clone(), Instant::now());
            }
            let db = db.clone();
            let id = outbound.id;
            let timeout = info.suggested_timeout.max(1_000);
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(timeout as u64)).await;
                let _ = db.mark_timed_out(id);
            });
        }
        Err(err) => {
            db.mark_outbound_failed(outbound.id, &err.to_string())
                .map_err(|err| err.to_string())?;
        }
    }
    Ok(())
}

const MESHCORE_UART: uuid::Uuid = uuid::Uuid::from_u128(0x6e400001_b5a3_f393_e0a9_e50e24dcca9e);

/// MeshCore channel frames are usually `DeviceName: message body`.
pub fn channel_sender_and_body(text: &str) -> (Option<String>, String) {
    let trimmed = text.trim();
    let Some((name, body)) = trimmed.split_once(':') else {
        return (None, trimmed.to_string());
    };
    let name = name.trim();
    if name.is_empty() || name.len() > 48 || name.contains('\n') {
        return (None, trimmed.to_string());
    }
    // Avoid treating URLs / times as sender labels.
    if name.contains("://") || name.chars().all(|ch| ch.is_ascii_digit() || ch == '.') {
        return (None, trimmed.to_string());
    }
    (Some(name.to_string()), body.trim_start().to_string())
}

/// Ensure outbound channel text carries our advert name like other MeshCore clients.
fn prefix_channel_sender(name: &str, text: &str) -> String {
    let name = name.trim();
    let text = text.trim();
    if name.is_empty() {
        return text.to_string();
    }
    let already = channel_sender_and_body(text)
        .0
        .is_some_and(|parsed| parsed.eq_ignore_ascii_case(name));
    if already {
        text.to_string()
    } else {
        format!("{name}: {text}")
    }
}

fn signal_word(rssi: i16) -> &'static str {
    if rssi >= -60 {
        "strong"
    } else if rssi >= -75 {
        "nearby"
    } else if rssi >= -90 {
        "far"
    } else {
        "weak"
    }
}

fn known_company(id: u16) -> Option<&'static str> {
    Some(match id {
        0x0006 => "Microsoft",
        0x000D => "Texas Instruments",
        0x000F => "Broadcom",
        0x004C => "Apple",
        0x0059 => "Nordic Semiconductor",
        0x0075 => "Samsung",
        0x00E0 => "Google",
        0x02E5 => "Espressif",
        _ => return None,
    })
}

fn manufacturer_label(data: &std::collections::HashMap<u16, Vec<u8>>) -> String {
    let mut ids: Vec<u16> = data.keys().copied().collect();
    ids.sort_unstable();
    for id in &ids {
        if let Some(name) = known_company(*id) {
            return name.to_string();
        }
    }
    ids.first()
        .map(|id| format!("manufacturer {id:#06X}"))
        .unwrap_or_default()
}

fn merge_radio(previous: &BleRadio, next: BleRadio) -> BleRadio {
    let rssi = match (previous.rssi, next.rssi) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (Some(left), None) => Some(left),
        (None, Some(right)) => Some(right),
        (None, None) => None,
    };
    let name = if next.name == "Unnamed device" {
        previous.name.clone()
    } else {
        next.name
    };
    let manufacturer = if next.manufacturer.is_empty() {
        previous.manufacturer.clone()
    } else {
        next.manufacturer
    };
    let address_kind = if next.address_kind.is_empty() {
        previous.address_kind.clone()
    } else {
        next.address_kind
    };
    BleRadio {
        name,
        address: next.address,
        address_kind,
        rssi,
        companion: previous.companion || next.companion,
        manufacturer,
    }
}

fn sort_radios(devices: &mut [BleRadio]) {
    devices.sort_by(|left, right| {
        right.companion.cmp(&left.companion).then(
            right
                .rssi
                .unwrap_or(i16::MIN)
                .cmp(&left.rssi.unwrap_or(i16::MIN)),
        ).then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
}

fn radio_from_properties(props: &btleplug::api::PeripheralProperties) -> BleRadio {
    let name = props
        .local_name
        .clone()
        .filter(|name| !name.trim().is_empty())
        .or_else(|| {
            props
                .advertisement_name
                .clone()
                .filter(|name| !name.trim().is_empty())
        })
        .unwrap_or_else(|| "Unnamed device".into());
    let companion = props.services.iter().any(|service| *service == MESHCORE_UART)
        || props.service_data.keys().any(|service| *service == MESHCORE_UART)
        || name.to_ascii_lowercase().contains("meshcore");
    let address_kind = match props.address_type {
        Some(btleplug::api::AddressType::Public) => "public",
        Some(btleplug::api::AddressType::Random) => "random",
        None => "",
    };
    BleRadio {
        name,
        address: props.address.to_string(),
        address_kind: address_kind.into(),
        rssi: props.rssi,
        companion,
        manufacturer: manufacturer_label(&props.manufacturer_data),
    }
}

async fn discover_radios(scan_for: Duration) -> Result<Vec<BleRadio>, String> {
    use btleplug::api::{Central, CentralEvent, Manager as _, Peripheral as _, ScanFilter};
    use btleplug::platform::Manager;
    use futures_util::StreamExt;

    let manager = Manager::new()
        .await
        .map_err(|err| format!("Bluetooth is unavailable: {err}"))?;
    let adapter = manager
        .adapters()
        .await
        .map_err(|err| format!("Could not list Bluetooth adapters: {err}"))?
        .into_iter()
        .next()
        .ok_or_else(|| "No Bluetooth adapter found.".to_string())?;
    let mut events = adapter
        .events()
        .await
        .map_err(|err| format!("Could not listen for Bluetooth devices: {err}"))?;
    adapter
        .start_scan(ScanFilter::default())
        .await
        .map_err(|err| format!("Bluetooth scan failed: {err}"))?;

    let mut found: HashMap<String, BleRadio> = HashMap::new();
    let _ = tokio::time::timeout(scan_for, async {
        while let Some(event) = events.next().await {
            let id = match event {
                CentralEvent::DeviceDiscovered(id) | CentralEvent::DeviceUpdated(id) => id,
                _ => continue,
            };
            let Ok(peripheral) = adapter.peripheral(&id).await else {
                continue;
            };
            let Ok(Some(props)) = peripheral.properties().await else {
                continue;
            };
            let radio = radio_from_properties(&props);
            let incoming = radio.clone();
            found
                .entry(radio.address.clone())
                .and_modify(|previous| *previous = merge_radio(previous, incoming))
                .or_insert(radio);
        }
    })
    .await;
    let _ = adapter.stop_scan().await;

    let mut devices: Vec<BleRadio> = found.into_values().collect();
    sort_radios(&mut devices);
    Ok(devices)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(name: &str, rssi: Option<i16>, companion: bool) -> BleRadio {
        BleRadio {
            name: name.into(),
            address: format!("AA:BB:CC:DD:EE:{:02X}", name.len()),
            address_kind: "public".into(),
            rssi,
            companion,
            manufacturer: if companion {
                "Nordic Semiconductor".into()
            } else {
                String::new()
            },
        }
    }

    #[test]
    fn companion_card_lists_signal_address_and_maker() {
        let radio = sample("Upstairs", Some(-48), true);
        assert_eq!(
            radio.detail_line(),
            "-48 dBm, strong signal, public address, MeshCore companion, Nordic Semiconductor"
        );
    }

    #[test]
    fn companions_sort_ahead_of_louder_strangers() {
        let mut devices = vec![
            sample("Lamp", Some(-40), false),
            sample("Radio", Some(-80), true),
            sample("Watch", None, false),
        ];
        sort_radios(&mut devices);
        assert_eq!(devices[0].name, "Radio");
        assert_eq!(devices[1].name, "Lamp");
        assert!(devices[2].rssi.is_none());
    }

    #[test]
    fn channel_text_splits_device_name_from_body() {
        let (name, body) = channel_sender_and_body("Foo 🤷🏡: @[RCZ001] 👍");
        assert_eq!(name.as_deref(), Some("Foo 🤷🏡"));
        assert_eq!(body, "@[RCZ001] 👍");
    }

    #[test]
    fn battery_curve_is_not_stuck_at_full_above_3930mv() {
        assert_eq!(battery_pct_from_mv(4200), 100);
        let mid = battery_pct_from_mv(4024);
        assert!(mid >= 80 && mid <= 85, "4024 mV should be ~80–85%, got {mid}");
        assert_eq!(battery_pct_from_mv(3800), 50);
        assert_eq!(battery_pct_from_mv(3700), 30);
        assert_eq!(battery_pct_from_mv(3000), 0);
        assert!(battery_pct_from_mv(4024) < 100);
    }
}
