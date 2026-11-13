//! A pool of companion radios (BLE and USB). One Tokio task owns each radio;
//! the window only sends commands. Only the primary companion transmits for HQ:
//! operator sends, bot replies, enrollment DMs, and channel pushes.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use meshcore_rs::commands::Destination;
use meshcore_rs::events::{
    Contact, ContactMessage, EventPayload, EventType, LogData, MeshCoreEvent, Subscription,
};
use meshcore_rs::packets::{PayloadType, RouteType};
use meshcore_rs::MeshCore;
use rand::RngCore;
use serde_json::json;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;
use tokio::sync::watch;

use crate::contact_qr;
use crate::db::{
    contact_snapshot, is_broadcast_pubkey, unix_now, BroadcastGroupKind, Db, InfraStatusSample,
    RadioRole,
};
use crate::path::{hop_count_from_path_len, parse_advert_path};
use crate::secrets::Secrets;
use crate::signup::{decide, parse_command};
use crate::usb_serial::{self, UsbPort};

const REPLY_GAP: Duration = Duration::from_secs(20);
/// Spacing between intentional broadcast fan-out legs (operator blast).
const BROADCAST_LEG_GAP: Duration = Duration::from_millis(250);

/// Type filter bits for `DISCOVER_REQ`: `(1 << ADV_TYPE_*)`.
/// Repeater (2) + room server (3).
pub const DISCOVER_FILTER_REPEATERS: u8 = (1 << 2) | (1 << 3);
/// Sensor (4).
pub const DISCOVER_FILTER_SENSORS: u8 = 1 << 4;

/// Cap in-memory RF RX log lines (CLI → RX log tab).
const RX_LOG_CAP: usize = 400;

#[derive(Debug, Clone)]
pub struct DiscoverSummary {
    pub returned: usize,
    pub added: usize,
}

/// One overheard RF frame for the CLI RX log (from companion `LogData`).
#[derive(Debug, Clone)]
pub struct RxLogLine {
    pub seq: u64,
    pub at: i64,
    pub text: String,
}

/// Result of an operator / passive advert-path fetch (`CMD_GET_ADVERT_PATH`).
#[derive(Debug, Clone)]
pub struct AdvertPathFetch {
    #[allow(dead_code)] // kept for callers that want raw path bytes
    pub path: Vec<u8>,
    #[allow(dead_code)]
    pub hash_size: usize,
    pub source: String,
    pub hop_count: u32,
}

/// Operator contact refresh: sync companion contact row, path cache, status, telemetry.
#[derive(Debug, Clone)]
pub struct ContactRefreshResult {
    pub summary: String,
}

/// Lets background fetches (weather/quake) enqueue a METIS-INFO channel reply.
/// Routes to whichever companion is primary at send time, so promotion needs no rewiring.
static RADIO_POOL: OnceLock<Arc<PoolShared>> = OnceLock::new();

fn enqueue_channel_text(slot: u8, text: String) {
    if let Some(pool) = RADIO_POOL.get() {
        pool.route_to_primary(RadioCmd::SendChannel {
            slot,
            text,
            bot_activity: true,
            reply: None,
        });
    } else {
        hq_log!(warn, "channel bot reply dropped — radio pool not ready");
    }
}

fn enqueue_channel_frames(slot: u8, frames: impl IntoIterator<Item = String>) {
    for text in frames {
        if !text.trim().is_empty() {
            enqueue_channel_text(slot, text);
        }
    }
}

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
    /// Operator mesh scan (regions / nearby discover) — shows footer progress.
    Discover,
    Ready,
}

impl SyncPhase {
    pub fn is_active(self) -> bool {
        !matches!(self, Self::Idle | Self::Ready)
    }

    pub fn is_discover(self) -> bool {
        matches!(self, Self::Discover)
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
    /// Companion protocol version from device query (`fw_version_code`). CLI needs ≥14.
    pub protocol_ver: u8,
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
    /// Bumped when a channel send finishes; `send_note` is empty on success.
    pub send_seq: u64,
    pub send_note: String,
    /// Bumped when a new RX message is inserted (Messages UI can refresh immediately).
    pub rx_seq: u64,
    /// Database id of the radio this status describes (`None` when no radio is live).
    pub radio_id: Option<i64>,
    /// `ble`, `usb`, `tcp`, or empty when no radio is live.
    pub transport: String,
    /// `companion`, `room`, `repeater`, `other`, or empty.
    pub role: String,
    /// Other radios with a live link besides the one described here.
    pub others_connected: u32,
    /// USB serial ports seen by the last scan.
    pub usb_ports: Vec<UsbPort>,
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
            protocol_ver: 0,
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
            send_seq: 0,
            send_note: String::new(),
            rx_seq: 0,
            radio_id: None,
            transport: String::new(),
            role: String::new(),
            others_connected: 0,
            usb_ports: Vec::new(),
        }
    }
}

/// Command for one radio session. Everything except `Disconnect` transmits or
/// changes the companion, so only the primary session accepts it.
pub enum RadioCmd {
    Disconnect,
    ApplyName(String),
    /// Push a channel definition onto the companion radio.
    PushChannel { slot: u8, name: String },
    /// Soft-delete / clear a non-system channel on the radio.
    ClearChannel {
        slot: u8,
        reply: Option<oneshot::Sender<Result<(), String>>>,
    },
    /// Send channel text. Prefixes the HQ name, waits for companion OK, then
    /// inserts the history row. `reply` tells the UI the outcome.
    /// `bot_activity` marks the history row as `origin=bot` so channel badges update.
    SendChannel {
        slot: u8,
        text: String,
        bot_activity: bool,
        reply: Option<oneshot::Sender<Result<(), String>>>,
    },
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
    /// Persist companion default flood scope via CMD 63 (name + SHA-256 key).
    SetDefaultFloodScope(Option<String>),
    /// Query repeater/room contacts for region lists (CMD 57 ANON regions).
    DiscoverRegions {
        reply: oneshot::Sender<Result<Vec<String>, String>>,
    },
    /// Zero-hop node discover (CMD 55 control DISCOVER_REQ).
    DiscoverNodes {
        filter: u8,
        reply: oneshot::Sender<Result<DiscoverSummary, String>>,
    },
    SendAdvert { flood: bool },
    Reboot,
    /// Local companion CLI (CMD 66) or remote TXT to a contact prefix.
    RunCli {
        /// Empty = local companion; otherwise 12-char hex pubkey prefix (or longer hex pubkey).
        contact_hex: String,
        text: String,
        /// `true` → TXT_TYPE_CLI_DATA (1) for claimed infra; `false` → CLI_COMMAND (3).
        infra: bool,
    },
    /// RF admin login to a repeater/room/sensor (`CMD_SEND_LOGIN`) + admin ACL proof.
    SendLogin {
        pubkey_hex: String,
        password: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Probe noise floor via status requests for GPS contacts.
    ProbeNoise,
    /// Operator path fetch (`CMD_GET_ADVERT_PATH` 0x2A). Local only — no channel/DM.
    FetchAdvertPath {
        pubkey_hex: String,
        reply: oneshot::Sender<Result<AdvertPathFetch, String>>,
    },
    /// Refresh one contact: companion contact sync + path + status + telemetry (airtime).
    RefreshContact {
        pubkey_hex: String,
        reply: oneshot::Sender<Result<ContactRefreshResult, String>>,
    },
    /// Binary status request for one claimed infra pubkey.
    ProbeInfraStatus {
        pubkey_hex: String,
        reply: oneshot::Sender<Result<meshcore_rs::events::StatusData, String>>,
    },
    /// Lightweight reachability probe (binary status only — no neighbours).
    PingNode {
        pubkey_hex: String,
        reply: oneshot::Sender<Result<meshcore_rs::events::StatusData, String>>,
    },
    /// Sign bytes with the primary companion (Ed25519). Used by map upload.
    SignBytes {
        data: Vec<u8>,
        reply: oneshot::Sender<Result<Vec<u8>, String>>,
    },
    /// Export private key into caller RAM only (never persisted by HQ).
    ExportPrivateKey {
        reply: oneshot::Sender<Result<[u8; 64], String>>,
    },
}

impl RadioCmd {
    /// True when only the primary companion may run this command.
    fn needs_primary(&self) -> bool {
        !matches!(self, Self::Disconnect)
    }
}

/// Why a secondary (non-primary) companion refuses operator or bot transmit commands.
const NOT_PRIMARY_REASON: &str =
    "This radio is not the primary companion. Make it primary in Settings to transmit.";

/// Message when no connected companion owns TX.
fn no_primary_reason(cmd: &RadioCmd) -> &'static str {
    match cmd {
        RadioCmd::SendChannel { .. } => {
            "Connect a primary companion radio before sending. The message was not transmitted."
        }
        RadioCmd::ClearChannel { .. } => {
            "Connect a primary companion radio before clearing a channel slot."
        }
        RadioCmd::DiscoverRegions { .. } => "Connect a primary companion radio before scanning regions.",
        RadioCmd::DiscoverNodes { .. } => {
            "Connect a primary companion radio before discovering nodes."
        }
        RadioCmd::RunCli { .. } => "Connect a primary companion radio before running CLI commands.",
        RadioCmd::SendLogin { .. } => {
            "Connect a primary companion radio before verifying infrastructure."
        }
        RadioCmd::ProbeInfraStatus { .. } => {
            "Connect a primary companion radio before refreshing infrastructure status."
        }
        RadioCmd::FetchAdvertPath { .. } => {
            "Connect a primary companion radio before tracing a path."
        }
        RadioCmd::RefreshContact { .. } => {
            "Connect a primary companion radio before refreshing a contact."
        }
        RadioCmd::SignBytes { .. } | RadioCmd::ExportPrivateKey { .. } => {
            "Connect a primary companion radio before authorizing map uploads."
        }
        _ => "Connect a primary companion radio before changing device settings.",
    }
}

/// Pick the primary companion: the operator's saved choice when it is connected,
/// otherwise the lowest-id connected companion. `None` when nothing qualifies.
fn pick_primary(preferred: Option<i64>, connected_companions: &[i64]) -> Option<i64> {
    if let Some(id) = preferred {
        if connected_companions.contains(&id) {
            return Some(id);
        }
    }
    connected_companions.iter().copied().min()
}

/// Refuse a command without transmitting. Replies carry the reason so the UI can show it.
fn reject_cmd(cmd: RadioCmd, status: &Mutex<RadioStatus>, reason: &str) {
    if matches!(cmd, RadioCmd::Disconnect) {
        return;
    }
    hq_log!(warn, "radio command refused: {reason}");
    match cmd {
        RadioCmd::SendChannel { reply, .. } => {
            push_send_note(status, reason);
            if let Some(reply) = reply {
                let _ = reply.send(Err(reason.into()));
            }
        }
        RadioCmd::ClearChannel { reply, .. } => {
            if let Some(reply) = reply {
                let _ = reply.send(Err(reason.into()));
            }
        }
        RadioCmd::DiscoverRegions { reply } => {
            let _ = reply.send(Err(reason.into()));
        }
        RadioCmd::DiscoverNodes { reply, .. } => {
            let _ = reply.send(Err(reason.into()));
        }
        RadioCmd::SendLogin { reply, .. } => {
            let _ = reply.send(Err(reason.into()));
        }
        RadioCmd::ProbeInfraStatus { reply, .. } => {
            let _ = reply.send(Err(reason.into()));
        }
        RadioCmd::FetchAdvertPath { reply, .. } => {
            let _ = reply.send(Err(reason.into()));
        }
        RadioCmd::RefreshContact { reply, .. } => {
            let _ = reply.send(Err(reason.into()));
        }
        RadioCmd::SignBytes { reply, .. } => {
            let _ = reply.send(Err(reason.into()));
        }
        RadioCmd::ExportPrivateKey { reply } => {
            let _ = reply.send(Err(reason.into()));
        }
        RadioCmd::RunCli { .. } => push_cli_line(status, format!("error: {reason}")),
        _ => {}
    }
    set_detail(status, reason);
}

fn set_detail(status: &Mutex<RadioStatus>, detail: impl Into<String>) {
    status.lock().unwrap_or_else(|err| err.into_inner()).detail = detail.into();
}

/// One radio in the pool as the operator sees it (saved radios plus live state).
#[derive(Debug, Clone, PartialEq)]
pub struct PoolRadioView {
    pub id: i64,
    pub transport: String,
    pub address: String,
    pub label: String,
    pub role: String,
    pub state: String,
    pub detail: String,
    pub battery_pct: Option<u8>,
    pub is_primary: bool,
    pub connected: bool,
    /// Unix seconds of the last successful connect, if any.
    pub last_connected: Option<i64>,
}

/// Work for the pool coordinator. Slot lifecycle changes go through one task so
/// starts, stops, and primary promotion never race each other.
enum PoolCmd {
    Scan,
    ScanUsb,
    Pair { target: String, pin: u32 },
    Reconnect(String),
    ConnectBle { address: String, label: String },
    ConnectUsb(String),
    /// Wi-Fi / Ethernet companion: normalized `host:port`.
    ConnectTcp(String),
    DisconnectRadio(i64),
    ForgetRadio(i64),
    SetPrimary(i64),
    SetRole { id: i64, role: RadioRole },
}

/// One radio the pool is running or reconnecting. Disconnected radios have no entry.
struct SlotEntry {
    id: i64,
    /// Distinguishes a replaced slot from the one that just ended.
    token: u64,
    role: RadioRole,
    status: Arc<Mutex<RadioStatus>>,
    cmd: UnboundedSender<RadioCmd>,
    /// Bumped on Disconnect/Reboot; cancels a live session wherever it is waiting.
    stop: Arc<watch::Sender<u64>>,
    /// When stop fires because of Reboot, send CMD_REBOOT before dropping the link.
    reboot_pending: Arc<AtomicBool>,
    /// Set while a region/node discover runs; `cancel_mesh_scan` requests stop.
    scan_cancel: Arc<AtomicBool>,
    /// True only for the companion that owns TX, bot replies, and the outbound queue.
    is_primary: Arc<AtomicBool>,
    /// True while the session holds a live link (after the companion identified itself).
    link_up: Arc<AtomicBool>,
    /// Operator asked this slot to stop; a new start waits for teardown.
    ending: Arc<AtomicBool>,
}

/// Everything one session task needs to know about its slot.
struct SlotCtx {
    pool: Arc<PoolShared>,
    id: i64,
    transport: String,
    /// Live open path. USB may rewrite this when the OS renumerates the port.
    address: Mutex<String>,
    status: Arc<Mutex<RadioStatus>>,
    is_primary: Arc<AtomicBool>,
    link_up: Arc<AtomicBool>,
    reboot_pending: Arc<AtomicBool>,
    scan_cancel: Arc<AtomicBool>,
}

impl SlotCtx {
    fn is_primary(&self) -> bool {
        self.is_primary.load(Ordering::SeqCst)
    }

    /// Mark the link up or down and let the pool re-pick the primary companion.
    fn set_link(&self, up: bool) {
        self.link_up.store(up, Ordering::SeqCst);
        self.pool.reconcile_primary();
    }
}

/// Shared state for every radio session.
struct PoolShared {
    db: Arc<Db>,
    secrets: Arc<Secrets>,
    ctl: UnboundedSender<PoolCmd>,
    slots: Mutex<BTreeMap<i64, SlotEntry>>,
    /// Scan results and operator notices; also the status line when no slot is live.
    idle: Arc<Mutex<RadioStatus>>,
    next_token: AtomicU64,
    ble_scan_busy: AtomicBool,
    /// CLI RX log tab is open — session should subscribe `LogData` (unless map upload already does).
    rx_log_wanted: AtomicBool,
    rx_log: Mutex<VecDeque<RxLogLine>>,
    rx_log_seq: AtomicU64,
}

/// The slot the header and Settings describe: primary, else a companion, else any.
fn view_slot(slots: &BTreeMap<i64, SlotEntry>) -> Option<&SlotEntry> {
    slots
        .values()
        .find(|slot| slot.is_primary.load(Ordering::SeqCst))
        .or_else(|| slots.values().find(|slot| slot.role == RadioRole::Companion))
        .or_else(|| slots.values().next())
}

fn usb_fallback_label(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(path)
        .to_string()
}

impl PoolShared {
    fn slots(&self) -> std::sync::MutexGuard<'_, BTreeMap<i64, SlotEntry>> {
        self.slots.lock().unwrap_or_else(|err| err.into_inner())
    }

    /// Status that operator-visible messages should land on.
    fn view_status(&self) -> Arc<Mutex<RadioStatus>> {
        let slots = self.slots();
        view_slot(&slots)
            .map(|slot| slot.status.clone())
            .unwrap_or_else(|| self.idle.clone())
    }

    /// Write a pool-level notice. The idle line always gets it; a live slot shows it as detail.
    fn notify(&self, state: &str, detail: impl Into<String>) {
        let detail = detail.into();
        write_status(&self.idle, state, detail.clone());
        let slots = self.slots();
        if let Some(view) = view_slot(&slots) {
            set_detail(&view.status, detail);
        }
    }

    fn snapshot(&self) -> RadioStatus {
        let idle = self
            .idle
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone();
        let slots = self.slots();
        let Some(view) = view_slot(&slots) else {
            return idle;
        };
        let mut status = view
            .status
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone();
        status.role = view.role.as_str().into();
        status.others_connected = slots
            .values()
            .filter(|slot| slot.id != view.id && slot.link_up.load(Ordering::SeqCst))
            .count() as u32;
        status.devices = idle.devices;
        status.usb_ports = idle.usb_ports;
        status
    }

    fn pool_snapshot(&self) -> Vec<PoolRadioView> {
        let rows = match self.db.list_saved_radios() {
            Ok(rows) => rows,
            Err(err) => {
                hq_log!(warn, "radio pool: could not list saved radios: {err}");
                Vec::new()
            }
        };
        let slots = self.slots();
        let mut out: Vec<PoolRadioView> = rows
            .into_iter()
            .map(|row| match slots.get(&row.id) {
                Some(slot) => {
                    let status = slot.status.lock().unwrap_or_else(|err| err.into_inner());
                    PoolRadioView {
                        id: row.id,
                        transport: row.transport,
                        address: row.address,
                        label: row.label,
                        role: row.role.as_str().into(),
                        state: status.state.clone(),
                        detail: status.detail.clone(),
                        battery_pct: status.battery_pct,
                        is_primary: slot.is_primary.load(Ordering::SeqCst),
                        connected: status.state == "connected",
                        last_connected: row.last_connected,
                    }
                }
                None => PoolRadioView {
                    id: row.id,
                    transport: row.transport,
                    address: row.address,
                    label: row.label,
                    role: row.role.as_str().into(),
                    state: "disconnected".into(),
                    detail: if row.last_error.is_empty() {
                        "Not connected.".into()
                    } else {
                        row.last_error
                    },
                    battery_pct: None,
                    is_primary: false,
                    connected: false,
                    last_connected: row.last_connected,
                },
            })
            .collect();
        out.sort_by_key(|radio| radio.id);
        out
    }

    /// Re-pick the primary companion after a link, role, or preference change.
    fn reconcile_primary(&self) {
        let preferred = match self.db.primary_radio_id() {
            Ok(id) => id,
            Err(err) => {
                hq_log!(warn, "primary radio setting: {err}");
                None
            }
        };
        let next = {
            let slots = self.slots();
            let connected: Vec<i64> = slots
                .values()
                .filter(|slot| {
                    slot.role == RadioRole::Companion && slot.link_up.load(Ordering::SeqCst)
                })
                .map(|slot| slot.id)
                .collect();
            let next = pick_primary(preferred, &connected);
            for slot in slots.values() {
                let want = next == Some(slot.id);
                if slot.is_primary.swap(want, Ordering::SeqCst) != want {
                    hq_log!(
                        info,
                        "radio {} is {} the primary companion",
                        slot.id,
                        if want { "now" } else { "no longer" }
                    );
                }
            }
            next
        };
        if let Some(id) = next {
            if preferred != Some(id) {
                if let Err(err) = self.db.set_primary_radio_id(id) {
                    hq_log!(warn, "could not save primary radio: {err}");
                }
            }
        }
    }

    /// Send a command to the primary companion, or refuse it with a reason.
    fn route_to_primary(&self, cmd: RadioCmd) {
        let target = {
            let slots = self.slots();
            slots
                .values()
                .find(|slot| slot.is_primary.load(Ordering::SeqCst))
                .map(|slot| (slot.cmd.clone(), slot.status.clone()))
        };
        match target {
            Some((tx, status)) => {
                if let Err(err) = tx.send(cmd) {
                    reject_cmd(err.0, &status, "Radio task is not running.");
                }
            }
            None => {
                let reason = no_primary_reason(&cmd);
                reject_cmd(cmd, &self.view_status(), reason);
            }
        }
    }

    /// Disconnect if live, then delete the saved radio row from the pool list.
    fn forget_radio(&self, id: i64) {
        let label = self.radio_label(id);
        self.disconnect_slot(id);
        match self.db.forget_radio(id) {
            Ok(()) => {
                self.reconcile_primary();
                self.notify("disconnected", format!("Removed {label} from the radio pool."));
            }
            Err(err) => self.notify("error", err.to_string()),
        }
    }

    /// Stop one slot. Returns false when no such slot is running.
    fn disconnect_slot(&self, id: i64) -> bool {
        let target = {
            let slots = self.slots();
            slots.get(&id).map(|slot| {
                (
                    slot.status.clone(),
                    slot.stop.clone(),
                    slot.cmd.clone(),
                    slot.reboot_pending.clone(),
                    slot.ending.clone(),
                )
            })
        };
        let Some((status, stop, cmd, reboot_pending, ending)) = target else {
            return false;
        };
        reboot_pending.store(false, Ordering::SeqCst);
        ending.store(true, Ordering::SeqCst);
        // Show progress right away; the slot task confirms with "Disconnected."
        write_status(&status, "disconnecting", "Disconnecting…");
        stop.send_modify(|generation| *generation = generation.wrapping_add(1));
        let _ = cmd.send(RadioCmd::Disconnect);
        true
    }

    /// Disconnect the radio the header describes (primary, else the first live slot).
    fn disconnect_view(&self) {
        let id = {
            let slots = self.slots();
            view_slot(&slots).map(|slot| slot.id)
        };
        match id {
            Some(id) => {
                self.disconnect_slot(id);
            }
            None => self.notify("disconnected", "Disconnected."),
        }
    }

    fn reboot_primary(&self) {
        let target = {
            let slots = self.slots();
            slots
                .values()
                .find(|slot| slot.is_primary.load(Ordering::SeqCst))
                .map(|slot| {
                    (
                        slot.status.clone(),
                        slot.stop.clone(),
                        slot.cmd.clone(),
                        slot.reboot_pending.clone(),
                    )
                })
        };
        let Some((status, stop, cmd, reboot_pending)) = target else {
            self.route_to_primary(RadioCmd::Reboot);
            return;
        };
        // Cancel whatever the session is doing, send CMD_REBOOT on the live
        // link, then force-drop the transport so we do not sit "Connected".
        reboot_pending.store(true, Ordering::SeqCst);
        write_status(&status, "reconnecting", "Rebooting companion…");
        stop.send_modify(|generation| *generation = generation.wrapping_add(1));
        let _ = cmd.send(RadioCmd::Reboot);
    }

    fn cancel_primary_scan(&self) {
        let target = {
            let slots = self.slots();
            slots
                .values()
                .find(|slot| slot.is_primary.load(Ordering::SeqCst))
                .map(|slot| (slot.scan_cancel.clone(), slot.status.clone()))
        };
        if let Some((cancel, status)) = target {
            cancel.store(true, Ordering::SeqCst);
            write_status(&status, "connected", "Stopping scan…");
        }
    }

    fn radio_label(&self, id: i64) -> String {
        self.db
            .list_saved_radios()
            .ok()
            .and_then(|rows| rows.into_iter().find(|row| row.id == id))
            .map(|row| row.label)
            .filter(|label| !label.trim().is_empty())
            .unwrap_or_else(|| format!("Radio {id}"))
    }

    fn apply_set_primary(&self, id: i64) {
        let label = self.radio_label(id);
        let connected = {
            let slots = self.slots();
            slots
                .get(&id)
                .is_some_and(|slot| slot.link_up.load(Ordering::SeqCst))
        };
        if !connected {
            self.notify(
                "disconnected",
                format!("Connect {label} before making it the primary companion."),
            );
            return;
        }
        match self.db.set_primary_radio_id(id) {
            Ok(()) => {
                self.reconcile_primary();
                self.notify("disconnected", format!("{label} is now the primary companion."));
            }
            Err(err) => self.notify("error", err.to_string()),
        }
    }

    fn apply_set_role(&self, id: i64, role: RadioRole) {
        if let Err(err) = self.db.set_radio_role(id, role) {
            self.notify("error", err.to_string());
            return;
        }
        {
            let mut slots = self.slots();
            if let Some(slot) = slots.get_mut(&id) {
                slot.role = role;
            }
        }
        self.reconcile_primary();
        let label = self.radio_label(id);
        self.notify("disconnected", format!("{label} role set to {}.", role.label()));
    }

    fn remove_slot(&self, id: i64, token: u64) {
        let empty = {
            let mut slots = self.slots();
            if slots.get(&id).is_some_and(|slot| slot.token == token) {
                slots.remove(&id);
            }
            slots.is_empty()
        };
        self.reconcile_primary();
        if empty {
            // Keep a specific disconnect reason the ending slot already wrote
            // (busy companion, USB permission, etc.).
            let mut idle = self.idle.lock().unwrap_or_else(|err| err.into_inner());
            let keep = idle.state == "disconnected"
                && !idle.detail.trim().is_empty()
                && idle.detail != "Disconnected.";
            if !keep {
                idle.state = "disconnected".into();
                idle.detail = "Disconnected.".into();
            }
        }
    }

    /// Save the radio, then run a session for it unless one is already live.
    async fn start_slot(self: &Arc<Self>, transport: &str, address: &str, label: &str) {
        self.start_slot_with_identity(transport, address, label, "")
            .await;
    }

    async fn start_slot_with_identity(
        self: &Arc<Self>,
        transport: &str,
        address: &str,
        label: &str,
        usb_identity: &str,
    ) {
        let id = match self
            .db
            .remember_radio_with_identity(transport, address, label, usb_identity)
        {
            Ok(id) => id,
            Err(err) => {
                self.notify("error", format!("Could not save the radio: {err}"));
                return;
            }
        };
        // A slot the operator just stopped may still be releasing the link.
        for _ in 0..100 {
            let ending = self
                .slots()
                .get(&id)
                .is_some_and(|slot| slot.ending.load(Ordering::SeqCst));
            if !ending {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let running = self.slots().contains_key(&id);
        if running {
            self.notify(
                "disconnected",
                format!("{label} is already connecting or connected."),
            );
            return;
        }
        let role = self.db.radio_role(id).unwrap_or(RadioRole::Companion);
        let token = self.next_token.fetch_add(1, Ordering::SeqCst);
        let (cmd_tx, cmd_rx) = unbounded_channel();
        let (stop_tx, stop_rx) = watch::channel(0u64);
        let mut initial = RadioStatus::default();
        initial.radio_id = Some(id);
        initial.transport = transport.to_string();
        initial.role = role.as_str().to_string();
        initial.state = "connecting".into();
        initial.detail = format!("Connecting to {label}");
        let status = Arc::new(Mutex::new(initial));
        let is_primary = Arc::new(AtomicBool::new(false));
        let link_up = Arc::new(AtomicBool::new(false));
        let reboot_pending = Arc::new(AtomicBool::new(false));
        let scan_cancel = Arc::new(AtomicBool::new(false));
        let entry = SlotEntry {
            id,
            token,
            role,
            status: status.clone(),
            cmd: cmd_tx,
            stop: Arc::new(stop_tx),
            reboot_pending: reboot_pending.clone(),
            scan_cancel: scan_cancel.clone(),
            is_primary: is_primary.clone(),
            link_up: link_up.clone(),
            ending: Arc::new(AtomicBool::new(false)),
        };
        let ctx = Arc::new(SlotCtx {
            pool: self.clone(),
            id,
            transport: transport.to_string(),
            address: Mutex::new(address.to_string()),
            status,
            is_primary,
            link_up,
            reboot_pending,
            scan_cancel,
        });
        self.slots().insert(id, entry);
        let pool = self.clone();
        tokio::spawn(async move {
            slot_loop(ctx, cmd_rx, stop_rx).await;
            pool.remove_slot(id, token);
        });
    }

    fn spawn_ble_scan(self: &Arc<Self>) {
        if self.ble_scan_busy.swap(true, Ordering::SeqCst) {
            return;
        }
        let pool = self.clone();
        tokio::spawn(async move {
            pool.notify("scanning", "Listening for nearby Bluetooth devices…");
            match discover_radios(Duration::from_secs(5)).await {
                Ok(devices) => {
                    let companions = devices.iter().filter(|device| device.companion).count();
                    let count = devices.len();
                    let detail = if count == 0 {
                        "No Bluetooth devices answered. Confirm the adapter is on and the radio is powered.".to_string()
                    } else if companions == 0 {
                        format!("Found {count} devices. None advertised the MeshCore companion service. Match the upstairs radio by name and address.")
                    } else {
                        format!("Found {count} devices, {companions} advertising the MeshCore companion service.")
                    };
                    pool.idle.lock().unwrap_or_else(|err| err.into_inner()).devices = devices;
                    pool.notify("disconnected", detail);
                }
                Err(err) => pool.notify("error", err),
            }
            pool.ble_scan_busy.store(false, Ordering::SeqCst);
        });
    }

    fn spawn_pair(self: &Arc<Self>, target: String, pin: u32) {
        let target = target.trim().to_string();
        if target.is_empty() {
            self.notify("error", "Enter the Bluetooth name or address of the radio.");
            return;
        }
        let pool = self.clone();
        tokio::spawn(async move {
            pool.notify(
                "pairing",
                "Pairing with the radio using the PIN you entered. If the system shows its own Bluetooth PIN prompt, enter the same six-digit PIN there.",
            );
            // Windows pairing needs the scanned MAC, not the advertised name.
            let address = stable_radio_address(&pool.idle, &target);
            match crate::ble_pair::ensure_paired(&address, pin).await {
                Ok(()) => {
                    let label = scanned_label(&pool.idle, &target);
                    let _ = pool.ctl.send(PoolCmd::ConnectBle { address, label });
                }
                Err(err) => pool.notify("error", err),
            }
        });
    }

    async fn reconnect_saved(self: &Arc<Self>, target: String) {
        let target = target.trim().to_string();
        if target.is_empty() {
            self.notify("error", "Choose a saved companion radio first.");
            return;
        }
        // USB and TCP reconnect by stored address; BLE resolves through the scan list.
        let saved_row = self.db.list_saved_radios().ok().and_then(|rows| {
            rows.into_iter().find(|row| {
                (row.transport == "usb" || row.transport == "tcp")
                    && (row.address.eq_ignore_ascii_case(&target)
                        || row.label.eq_ignore_ascii_case(&target))
            })
        });
        if let Some(row) = saved_row {
            if row.transport == "usb" {
                self.refresh_usb_ports(false).await;
                let ports = self
                    .idle
                    .lock()
                    .unwrap_or_else(|err| err.into_inner())
                    .usb_ports
                    .clone();
                let identity = row.usb_identity.trim();
                match usb_serial::resolve_usb_target(
                    &row.address,
                    (!identity.is_empty()).then_some(identity),
                    &ports,
                ) {
                    Ok(port) => {
                        if port.path != row.address {
                            if let Err(err) = self.db.remap_radio_address(row.id, &port.path) {
                                hq_log!(warn, "usb remap on reconnect: {err}");
                            }
                        }
                        if let Some(live_id) = port.identity.as_deref() {
                            let _ = self.db.set_radio_usb_identity(row.id, live_id);
                        }
                        self.notify(
                            "connecting",
                            format!("Reconnecting to {}…", port.path),
                        );
                        self.start_slot_with_identity(
                            "usb",
                            &port.path,
                            &port.label,
                            port.identity.as_deref().unwrap_or(identity),
                        )
                        .await;
                        return;
                    }
                    Err(_) => {
                        // Device may be unplugged; start the slot so backoff can rematch.
                        self.notify(
                            "connecting",
                            format!("Waiting for USB companion {}…", row.address),
                        );
                        self.start_slot_with_identity(
                            "usb",
                            &row.address,
                            &row.label,
                            identity,
                        )
                        .await;
                        return;
                    }
                }
            }
            self.notify("connecting", format!("Reconnecting to {}…", row.address));
            self.start_slot(&row.transport, &row.address, &row.label)
                .await;
            return;
        }
        let address = stable_radio_address(&self.idle, &target);
        let label = scanned_label(&self.idle, &target);
        self.notify("connecting", format!("Reconnecting to {address}…"));
        self.start_slot("ble", &address, &label).await;
    }

    async fn connect_tcp_now(self: &Arc<Self>, raw: String) {
        let address = match crate::tcp_radio::normalize_tcp_address(&raw) {
            Ok(addr) => addr,
            Err(err) => {
                self.notify("error", err);
                return;
            }
        };
        let label = crate::tcp_radio::tcp_fallback_label(&address);
        self.notify("connecting", format!("Opening TCP {address}…"));
        self.start_slot("tcp", &address, &label).await;
    }

    async fn connect_usb_now(self: &Arc<Self>, raw: String) {
        let normalized = usb_serial::normalize_usb_address(&raw);
        if normalized.is_empty() {
            self.notify("error", "Choose a USB serial port first.");
            return;
        }
        let (address, label, identity) = {
            let idle = self.idle.lock().unwrap_or_else(|err| err.into_inner());
            idle.usb_ports
                .iter()
                .find(|port| port.path == normalized || port.system_path == normalized)
                .map(|port| {
                    (
                        port.path.clone(),
                        port.label.clone(),
                        port.identity.clone().unwrap_or_default(),
                    )
                })
                .unwrap_or_else(|| {
                    (normalized.clone(), usb_fallback_label(&normalized), String::new())
                })
        };
        self.notify("connecting", format!("Opening USB port {address}…"));
        self.start_slot_with_identity("usb", &address, &label, &identity)
            .await;
    }

    async fn refresh_usb_ports(&self, announce: bool) {
        if announce {
            self.notify("scanning_usb", "Looking for USB serial ports…");
        }
        match tokio::task::spawn_blocking(usb_serial::list_usb_ports).await {
            Ok(Ok(ports)) => {
                let count = ports.len();
                self.idle
                    .lock()
                    .unwrap_or_else(|err| err.into_inner())
                    .usb_ports = ports;
                if announce {
                    let detail = match count {
                        0 => "No USB serial radios found. Plug in the companion and scan again."
                            .to_string(),
                        1 => "Found 1 USB serial port.".to_string(),
                        n => format!("Found {n} USB serial ports."),
                    };
                    self.notify("disconnected", detail);
                }
            }
            Ok(Err(err)) => {
                if announce {
                    self.notify("error", format!("USB scan failed: {err}"));
                } else {
                    hq_log!(debug, "USB port list: {err}");
                }
            }
            Err(err) => hq_log!(warn, "USB port scan task: {err}"),
        }
    }
}

/// Coordinator: owns slot starts and stops, scans, and primary changes.
async fn pool_loop(pool: Arc<PoolShared>, mut rx: UnboundedReceiver<PoolCmd>) {
    pool.refresh_usb_ports(false).await;
    while let Some(cmd) = rx.recv().await {
        match cmd {
            PoolCmd::Scan => pool.spawn_ble_scan(),
            PoolCmd::ScanUsb => pool.refresh_usb_ports(true).await,
            PoolCmd::Pair { target, pin } => pool.spawn_pair(target, pin),
            PoolCmd::Reconnect(target) => pool.reconnect_saved(target).await,
            PoolCmd::ConnectBle { address, label } => {
                pool.start_slot("ble", &address, &label).await;
            }
            PoolCmd::ConnectUsb(path) => pool.connect_usb_now(path).await,
            PoolCmd::ConnectTcp(address) => pool.connect_tcp_now(address).await,
            PoolCmd::DisconnectRadio(id) => {
                pool.disconnect_slot(id);
            }
            PoolCmd::ForgetRadio(id) => pool.forget_radio(id),
            PoolCmd::SetPrimary(id) => pool.apply_set_primary(id),
            PoolCmd::SetRole { id, role } => pool.apply_set_role(id, role),
        }
    }
}

/// Public handle used by the window. Transmit-side calls always go to the primary companion.
#[derive(Clone)]
pub struct RadioHandle {
    pool: Arc<PoolShared>,
}

impl RadioHandle {
    fn ctl(&self, cmd: PoolCmd) {
        if self.pool.ctl.send(cmd).is_err() {
            hq_log!(error, "radio coordinator is not running");
        }
    }

    /// Scan for nearby Bluetooth radios.
    pub fn scan(&self) {
        self.ctl(PoolCmd::Scan);
    }

    /// Pair a new Bluetooth radio with its PIN, then connect it alongside any others.
    pub fn pair(&self, target: String, pin: u32) {
        self.ctl(PoolCmd::Pair { target, pin });
    }

    /// Connect a saved radio (Bluetooth address, USB path, or TCP host:port).
    pub fn reconnect(&self, target: String) {
        self.ctl(PoolCmd::Reconnect(target));
    }

    /// Disconnect the primary companion (or the only live radio).
    pub fn disconnect(&self) {
        self.pool.disconnect_view();
    }

    /// Open a USB serial companion by path (prefer `/dev/serial/by-id/...`).
    pub fn connect_usb(&self, path: String) {
        self.ctl(PoolCmd::ConnectUsb(path));
    }

    /// Open a Wi-Fi / Ethernet companion over TCP (`host` or `host:port`, default 5000).
    pub fn connect_tcp(&self, address: String) {
        self.ctl(PoolCmd::ConnectTcp(address));
    }

    /// Disconnect one radio in the pool by database id.
    pub fn disconnect_radio(&self, radio_id: i64) {
        self.ctl(PoolCmd::DisconnectRadio(radio_id));
    }

    /// Disconnect and remove a saved radio from the pool list.
    pub fn forget_radio(&self, radio_id: i64) {
        self.ctl(PoolCmd::ForgetRadio(radio_id));
    }

    /// Make a connected companion the primary (TX, bot, outbound queue).
    pub fn set_primary(&self, radio_id: i64) {
        self.ctl(PoolCmd::SetPrimary(radio_id));
    }

    /// Set a radio's role: `companion`, `room`, `repeater`, or `other`.
    pub fn set_role(&self, radio_id: i64, role: String) {
        self.ctl(PoolCmd::SetRole {
            id: radio_id,
            role: RadioRole::parse(&role),
        });
    }

    /// Refresh the USB serial port list.
    pub fn scan_usb(&self) {
        self.ctl(PoolCmd::ScanUsb);
    }

    /// Saved radios merged with live session state.
    pub fn pool_snapshot(&self) -> Vec<PoolRadioView> {
        self.pool.pool_snapshot()
    }

    pub fn apply_name(&self, name: String) {
        self.pool.route_to_primary(RadioCmd::ApplyName(name));
    }

    pub fn push_channel(&self, slot: u8, name: String) {
        self.pool
            .route_to_primary(RadioCmd::PushChannel { slot, name });
    }

    pub fn clear_channel(&self, slot: u8) -> oneshot::Receiver<Result<(), String>> {
        let (tx, rx) = oneshot::channel();
        self.pool.route_to_primary(RadioCmd::ClearChannel {
            slot,
            reply: Some(tx),
        });
        rx
    }

    pub fn send_channel(&self, slot: u8, text: String) -> oneshot::Receiver<Result<(), String>> {
        self.send_channel_with_origin(slot, text, false)
    }

    /// Bot / service posts — rail badges treat these as unread until the channel is opened.
    pub fn send_channel_bot(&self, slot: u8, text: String) -> oneshot::Receiver<Result<(), String>> {
        self.send_channel_with_origin(slot, text, true)
    }

    fn send_channel_with_origin(
        &self,
        slot: u8,
        text: String,
        bot_activity: bool,
    ) -> oneshot::Receiver<Result<(), String>> {
        let (tx, rx) = oneshot::channel();
        self.pool.route_to_primary(RadioCmd::SendChannel {
            slot,
            text,
            bot_activity,
            reply: Some(tx),
        });
        rx
    }

    pub fn refresh_device_settings(&self) {
        self.pool.route_to_primary(RadioCmd::RefreshDeviceSettings);
    }

    pub fn set_custom_var(&self, key: String, value: String) {
        self.pool
            .route_to_primary(RadioCmd::SetCustomVar { key, value });
    }

    pub fn set_coords(&self, lat: f64, lon: f64) {
        self.pool.route_to_primary(RadioCmd::SetCoords { lat, lon });
    }

    pub fn set_tx_power(&self, power: u8) {
        self.pool.route_to_primary(RadioCmd::SetTxPower(power));
    }

    pub fn set_radio_params(&self, freq_mhz: f64, bw_khz: f64, sf: u8, cr: u8, repeat: bool) {
        self.pool.route_to_primary(RadioCmd::SetRadioParams {
            freq_mhz,
            bw_khz,
            sf,
            cr,
            repeat,
        });
    }

    pub fn set_default_flood_scope(&self, scope: Option<String>) {
        self.pool
            .route_to_primary(RadioCmd::SetDefaultFloodScope(scope));
    }

    pub fn discover_regions(&self) -> oneshot::Receiver<Result<Vec<String>, String>> {
        let (tx, rx) = oneshot::channel();
        self.pool
            .route_to_primary(RadioCmd::DiscoverRegions { reply: tx });
        rx
    }

    pub fn discover_nodes(&self, filter: u8) -> oneshot::Receiver<Result<DiscoverSummary, String>> {
        let (tx, rx) = oneshot::channel();
        self.pool
            .route_to_primary(RadioCmd::DiscoverNodes { filter, reply: tx });
        rx
    }

    pub fn set_path_hash_mode(&self, mode: u8) {
        self.pool
            .route_to_primary(RadioCmd::SetPathHashMode(mode.min(2)));
    }

    pub fn set_adv_loc_policy(&self, policy: u8) {
        self.pool
            .route_to_primary(RadioCmd::SetAdvLocPolicy(policy.min(2)));
    }

    pub fn send_advert(&self, flood: bool) {
        self.pool.route_to_primary(RadioCmd::SendAdvert { flood });
    }

    /// CLI → RX log tab visibility. When true, the primary session subscribes `LogData`.
    pub fn set_rx_log_wanted(&self, wanted: bool) {
        self.pool.rx_log_wanted.store(wanted, Ordering::SeqCst);
    }

    /// Snapshot of the in-memory RF RX log (`seq` is the latest line id).
    pub fn rx_log_snapshot(&self) -> (u64, Vec<RxLogLine>) {
        let seq = self.pool.rx_log_seq.load(Ordering::SeqCst);
        let lines = self
            .pool
            .rx_log
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .iter()
            .cloned()
            .collect();
        (seq, lines)
    }

    pub fn clear_rx_log(&self) {
        self.pool
            .rx_log
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clear();
        // Bump so the UI refreshes an empty list.
        self.pool.rx_log_seq.fetch_add(1, Ordering::SeqCst);
    }

    pub fn reboot(&self) {
        self.pool.reboot_primary();
    }

    pub fn run_cli(&self, contact_hex: String, text: String) {
        self.pool.route_to_primary(RadioCmd::RunCli {
            contact_hex,
            text,
            infra: false,
        });
    }

    /// Remote CLI for claimed infrastructure (TXT_TYPE_CLI_DATA after ACL login).
    pub fn run_infra_cli(&self, contact_hex: String, text: String) {
        self.pool.route_to_primary(RadioCmd::RunCli {
            contact_hex,
            text,
            infra: true,
        });
    }

    /// Verify admin access on a repeater/room/sensor via RF login. Password is never logged.
    pub fn login_infra(
        &self,
        pubkey_hex: String,
        password: String,
    ) -> oneshot::Receiver<Result<(), String>> {
        let (tx, rx) = oneshot::channel();
        self.pool.route_to_primary(RadioCmd::SendLogin {
            pubkey_hex,
            password,
            reply: tx,
        });
        rx
    }

    pub fn probe_noise_samples(&self) {
        self.pool.route_to_primary(RadioCmd::ProbeNoise);
    }

    /// Operator path fetch for Map Trace (local companion only — no channel/DM).
    pub fn fetch_advert_path(
        &self,
        pubkey_hex: String,
    ) -> oneshot::Receiver<Result<AdvertPathFetch, String>> {
        let (tx, rx) = oneshot::channel();
        self.pool.route_to_primary(RadioCmd::FetchAdvertPath {
            pubkey_hex,
            reply: tx,
        });
        rx
    }

    /// Sync contact fields from the companion, then request path / status / telemetry.
    pub fn refresh_contact(
        &self,
        pubkey_hex: String,
    ) -> oneshot::Receiver<Result<ContactRefreshResult, String>> {
        let (tx, rx) = oneshot::channel();
        self.pool.route_to_primary(RadioCmd::RefreshContact {
            pubkey_hex,
            reply: tx,
        });
        rx
    }

    /// Binary status request for a claimed repeater / room / sensor (after admin ACL).
    pub fn probe_infra_status(
        &self,
        pubkey_hex: String,
    ) -> oneshot::Receiver<Result<meshcore_rs::events::StatusData, String>> {
        let (tx, rx) = oneshot::channel();
        self.pool.route_to_primary(RadioCmd::ProbeInfraStatus {
            pubkey_hex,
            reply: tx,
        });
        rx
    }

    /// Mesh status ping to any known pubkey (uses airtime; no neighbours follow-up).
    pub fn ping_node(
        &self,
        pubkey_hex: String,
    ) -> oneshot::Receiver<Result<meshcore_rs::events::StatusData, String>> {
        let (tx, rx) = oneshot::channel();
        self.pool.route_to_primary(RadioCmd::PingNode {
            pubkey_hex,
            reply: tx,
        });
        rx
    }

    pub fn cancel_mesh_scan(&self) {
        self.pool.cancel_primary_scan();
    }

    /// Ask the primary companion to Ed25519-sign `data` (map upload SHA-256 digest).
    pub async fn sign_bytes(&self, data: Vec<u8>) -> Result<Vec<u8>, String> {
        let (tx, rx) = oneshot::channel();
        self.pool
            .route_to_primary(RadioCmd::SignBytes { data, reply: tx });
        rx.await
            .unwrap_or_else(|_| Err("Radio task ended.".into()))
    }

    /// One-shot private key export into process memory for map-upload fallback.
    /// Caller must keep it in RAM only — never write to DB, keyring, or backup.
    pub async fn export_private_key_ram(&self) -> Result<[u8; 64], String> {
        let (tx, rx) = oneshot::channel();
        self.pool
            .route_to_primary(RadioCmd::ExportPrivateKey { reply: tx });
        rx.await
            .unwrap_or_else(|_| Err("Radio task ended.".into()))
    }

    /// Status of the primary companion (or the first live radio), plus scan results.
    pub fn snapshot(&self) -> RadioStatus {
        self.pool.snapshot()
    }
}

pub fn spawn_radio(db: Arc<Db>, secrets: Arc<Secrets>) -> RadioHandle {
    let (ctl, rx) = unbounded_channel();
    let pool = Arc::new(PoolShared {
        db,
        secrets,
        ctl,
        slots: Mutex::new(BTreeMap::new()),
        idle: Arc::new(Mutex::new(RadioStatus::default())),
        next_token: AtomicU64::new(1),
        ble_scan_busy: AtomicBool::new(false),
        rx_log_wanted: AtomicBool::new(false),
        rx_log: Mutex::new(VecDeque::new()),
        rx_log_seq: AtomicU64::new(0),
    });
    let _ = RADIO_POOL.set(pool.clone());
    let pool_thread = pool.clone();
    std::thread::Builder::new()
        .name("metis-radio".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("radio runtime");
            runtime.block_on(pool_loop(pool_thread, rx));
        })
        .expect("radio thread");
    RadioHandle { pool }
}

fn end_discover_sync(status: &Mutex<RadioStatus>, detail: impl Into<String>) {
    let detail = detail.into();
    write_sync(status, SyncPhase::Ready, "Companion ready", None);
    write_status(status, "connected", detail);
}

fn write_status(status: &Mutex<RadioStatus>, state: &str, detail: impl Into<String>) {
    let detail = detail.into();
    {
        let mut guard = status.lock().unwrap_or_else(|err| err.into_inner());
        let state_changed = guard.state != state;
        // Skip routine connected heartbeats (battery % refreshes) — still log
        // connect/disconnect/errors and one-shot operator messages.
        let heartbeat = state == "connected"
            && !state_changed
            && (detail.contains("Battery ") || detail.ends_with('.'))
            && detail.starts_with("Connected to ");
        if !heartbeat {
            if state == "error" {
                hq_log!(error, "radio [{state}] {detail}");
            } else {
                hq_log!(info, "radio [{state}] {detail}");
            }
        }
        guard.state = state.to_string();
        guard.detail = detail;
    }
}

fn write_sync(
    status: &Mutex<RadioStatus>,
    phase: SyncPhase,
    label: impl Into<String>,
    progress: Option<(u32, u32)>,
) {
    let label = label.into();
    let phase_name = match phase {
        SyncPhase::Idle => "idle",
        SyncPhase::Connecting => "connecting",
        SyncPhase::AppStart => "app_start",
        SyncPhase::Contacts => "contacts",
        SyncPhase::Channels => "channels",
        SyncPhase::Messages => "messages",
        SyncPhase::Discover => "discover",
        SyncPhase::Ready => "ready",
    };
    let phase_changed = {
        let guard = status.lock().unwrap_or_else(|err| err.into_inner());
        guard.sync_phase != phase
    };
    // Log phase changes and milestones; keep per-message drain ticks at verbose.
    let milestone = match progress {
        None => true,
        Some((0, _)) => true,
        Some((done, total)) if total > 0 && done == total => true,
        Some((done, _)) if done > 0 && done % 25 == 0 => true,
        _ => {
            label.contains("paused")
                || label.contains("ready")
                || label.contains("Synced")
                || label.contains("retry")
        }
    };
    if phase_changed || milestone {
        if let Some((done, total)) = progress {
            hq_log!(info, "sync [{phase_name}] {label} ({done}/{total})");
        } else {
            hq_log!(info, "sync [{phase_name}] {label}");
        }
    } else {
        hq_log!(verbose, "sync [{phase_name}] {label}");
    }
    let mut guard = status.lock().unwrap_or_else(|err| err.into_inner());
    guard.sync_phase = phase;
    guard.sync_label = label;
    guard.sync_progress = progress;
}

fn clear_session_telemetry(status: &Mutex<RadioStatus>) {
    // Map-upload RAM key (if any) dies with the radio session.
    crate::services::map_upload::clear_ram_private_key();
    let mut guard = status.lock().unwrap_or_else(|err| err.into_inner());
    guard.connected_name.clear();
    guard.sync_phase = SyncPhase::Idle;
    guard.sync_label.clear();
    guard.sync_progress = None;
    guard.battery_pct = None;
    guard.battery_mv = None;
    guard.battery_charging = false;
    guard.firmware.clear();
    guard.protocol_ver = 0;
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

/// Companion `CMD_SET_DEFAULT_FLOOD_SCOPE` (63) + clear session override (54).
async fn apply_default_flood_scope(
    commands: &meshcore_rs::commands::CommandHandler,
    scope: Option<&str>,
) -> Result<Option<String>, String> {
    let frame = crate::mesh_regions::default_flood_scope_frame(scope)?;
    commands
        .send(&frame, Some(EventType::Ok))
        .await
        .map_err(|err| err.to_string())?;
    if scope.is_none() {
        let clear = crate::mesh_regions::clear_session_flood_scope_frame();
        let _ = commands.send(&clear, Some(EventType::Ok)).await;
        return Ok(None);
    }
    let name = crate::mesh_regions::validate_region_name(scope.unwrap())?;
    Ok(Some(name))
}

const REGION_SCAN_MAX_TARGETS: usize = 12;
const REGION_SCAN_PER_TIMEOUT: Duration = Duration::from_secs(4);

/// `CMD_SEND_ANON_REQ` (57) regions subtype to one repeater/room pubkey.
async fn request_regions_from_contact(
    commands: &meshcore_rs::commands::CommandHandler,
    dispatcher: &meshcore_rs::events::EventDispatcher,
    pubkey: &[u8; 32],
) -> Result<Vec<String>, String> {
    let mut data = vec![57u8];
    data.extend_from_slice(pubkey);
    data.push(meshcore_rs::AnonReqType::Regions as u8);
    data.push(0x00);

    let mut rx = dispatcher.receiver();
    let mut send_fut = Box::pin(commands.send_with_timeout(
        &data,
        Some(EventType::MsgSent),
        REGION_SCAN_PER_TIMEOUT,
    ));
    let mut tag: Option<[u8; 4]> = None;
    let mut pending: Option<Vec<u8>> = None;
    let deadline = Instant::now() + REGION_SCAN_PER_TIMEOUT;

    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err("Region request timed out.".into());
        }
        tokio::select! {
            _ = tokio::time::sleep(left) => {
                return Err("Region request timed out.".into());
            }
            res = &mut send_fut, if tag.is_none() => {
                match res {
                    Ok(event) => match event.payload {
                        EventPayload::MsgSent(info) => {
                            tag = Some(info.expected_ack);
                            if let Some(data) = pending.take() {
                                return Ok(crate::mesh_regions::parse_regions_binary_response(&data));
                            }
                        }
                        _ => return Err("Unexpected response to region request.".into()),
                    },
                    Err(err) => return Err(err.to_string()),
                }
            }
            event = rx.recv() => {
                match event {
                    Ok(event) if event.event_type == EventType::MsgSent && tag.is_none() => {
                        if let EventPayload::MsgSent(info) = event.payload {
                            tag = Some(info.expected_ack);
                            if let Some(data) = pending.take() {
                                return Ok(crate::mesh_regions::parse_regions_binary_response(&data));
                            }
                        }
                    }
                    Ok(event) if event.event_type == EventType::BinaryResponse => {
                        if let EventPayload::BinaryResponse { tag: resp_tag, data } = event.payload {
                            match tag {
                                Some(t) if t == resp_tag => {
                                    return Ok(crate::mesh_regions::parse_regions_binary_response(&data));
                                }
                                None => {
                                    // Reply raced ahead of MsgSent — keep until tag arrives.
                                    pending = Some(data);
                                }
                                _ => {}
                            }
                        }
                    }
                    Ok(event) if event.event_type == EventType::Error => {
                        return Err(match event.payload {
                            EventPayload::String(msg) => msg,
                            _ => "Region request failed.".into(),
                        });
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(_) => return Err("Companion event stream closed.".into()),
                }
            }
        }
    }
}

/// Parse a DISCOVER_RESP control payload (firmware: `0x90|type`, snr, tag, pubkey).
fn parse_discover_resp_payload(
    payload: &[u8],
    expect_tag: [u8; 4],
) -> Option<(u8, [u8; 32])> {
    // Direct control payload (meshcore-rs test shape / stripped push).
    if let Some(found) = parse_discover_resp_at(payload, 0, expect_tag) {
        return Some(found);
    }
    // PUSH_CODE_CONTROL_DATA body: [snr][rssi][path_len=0][payload…]
    if payload.len() >= 3 {
        let path_len = payload[2] as usize;
        let start = 3 + path_len;
        if start < payload.len() {
            return parse_discover_resp_at(payload, start, expect_tag);
        }
    }
    None
}

fn parse_discover_resp_at(
    payload: &[u8],
    start: usize,
    expect_tag: [u8; 4],
) -> Option<(u8, [u8; 32])> {
    let body = payload.get(start..)?;
    if body.len() < 6 + 32 {
        return None;
    }
    if body[0] & 0xF0 != 0x90 {
        return None;
    }
    let node_type = body[0] & 0x0F;
    let mut tag = [0u8; 4];
    tag.copy_from_slice(&body[2..6]);
    if tag != expect_tag {
        return None;
    }
    let mut pubkey = [0u8; 32];
    pubkey.copy_from_slice(&body[6..38]);
    if pubkey.iter().all(|b| *b == 0) {
        return None;
    }
    Some((node_type, pubkey))
}

const NODE_DISCOVER_WINDOW: Duration = Duration::from_secs(8);

async fn run_node_discover(
    db: &Db,
    meshcore: &MeshCore,
    status: &Mutex<RadioStatus>,
    filter: u8,
    scan_cancel: &AtomicBool,
    radio_id: i64,
) -> Result<DiscoverSummary, String> {
    let label = if filter == DISCOVER_FILTER_SENSORS {
        "sensors"
    } else {
        "repeaters"
    };
    write_sync(
        status,
        SyncPhase::Discover,
        format!("Discovering nearby {label}…"),
        Some((0, NODE_DISCOVER_WINDOW.as_secs() as u32)),
    );

    let mut tag = [0u8; 4];
    rand::thread_rng().fill_bytes(&mut tag);

    // MeshMonitor / companion app wire form (works on live companions):
    // [55][0x80 DISCOVER_REQ][filter][tag:4]. Optional since is omitted.
    let mut frame = vec![55u8, 0x80, filter];
    frame.extend_from_slice(&tag);

    let dispatcher = meshcore.dispatcher();
    let mut rx = dispatcher.receiver();
    {
        let commands = meshcore.commands().lock().await;
        // Listen before write — Ok often arrives before send_with_timeout subscribes.
        match request_listening_first(
            &commands,
            dispatcher,
            &frame,
            &[EventType::Ok, EventType::Error],
            Duration::from_secs(3),
        )
        .await
        {
            Ok(event) if event.event_type == EventType::Error => {
                let msg = match event.payload {
                    EventPayload::String(s) => s,
                    _ => "Discover request rejected.".into(),
                };
                end_discover_sync(status, msg.clone());
                return Err(msg);
            }
            Ok(_) => {}
            Err(err) => {
                // Still collect 0x8E responses; some firmwares ACK too fast to catch.
                hq_log!(debug, "discover send ack missed ({err}); listening anyway");
            }
        }
    }

    let started = Instant::now();
    let deadline = started + NODE_DISCOVER_WINDOW;
    let mut seen: HashMap<[u8; 32], u8> = HashMap::new();
    let mut stopped = false;
    while Instant::now() < deadline {
        if scan_cancel.load(Ordering::SeqCst) {
            stopped = true;
            break;
        }
        let elapsed = started.elapsed().as_secs().min(NODE_DISCOVER_WINDOW.as_secs()) as u32;
        write_sync(
            status,
            SyncPhase::Discover,
            format!(
                "Discovering {label}… {} heard",
                seen.len()
            ),
            Some((elapsed, NODE_DISCOVER_WINDOW.as_secs() as u32)),
        );
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let slice = left.min(Duration::from_millis(400));
        match tokio::time::timeout(slice, rx.recv()).await {
            Ok(Ok(event)) => {
                let bytes = match event.payload {
                    EventPayload::Bytes(b) if event.event_type == EventType::ControlData => Some(b),
                    EventPayload::DiscoverResponse(entries) => {
                        for entry in entries {
                            if entry.pubkey.len() == 32 {
                                let mut key = [0u8; 32];
                                key.copy_from_slice(&entry.pubkey);
                                seen.entry(key).or_insert(2);
                            }
                        }
                        None
                    }
                    _ => None,
                };
                if let Some(payload) = bytes {
                    if let Some((node_type, pubkey)) =
                        parse_discover_resp_payload(&payload, tag)
                    {
                        seen.entry(pubkey).or_insert(node_type);
                    }
                }
            }
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => {}
            Ok(Err(_)) => break,
            Err(_) => {}
        }
    }

    let mut added = 0usize;
    let returned = seen.len();
    for (pubkey, node_type) in &seen {
        if scan_cancel.load(Ordering::SeqCst) {
            stopped = true;
            break;
        }
        let ty = match *node_type {
            1..=4 => *node_type,
            _ if filter == DISCOVER_FILTER_SENSORS => 4,
            _ => 2,
        };
        let hex = hex::encode(pubkey);
        let prefix: String = hex.chars().take(8).collect();
        let name = match ty {
            3 => format!("room-{prefix}"),
            4 => format!("sensor-{prefix}"),
            _ => format!("repeater-{prefix}"),
        };
        let existed = db.get_node(&hex).ok().flatten().is_some();
        let contact = Contact {
            public_key: *pubkey,
            contact_type: ty,
            flags: 0,
            path_len: -1,
            out_path: Vec::new(),
            adv_name: name.clone(),
            last_advert: unix_now() as u32,
            adv_lat: 0,
            adv_lon: 0,
            last_modification_timestamp: unix_now() as u32,
        };
        {
            let commands = meshcore.commands().lock().await;
            if let Err(err) = commands.add_contact(&contact).await {
                hq_log!(debug, "discover add_contact failed: {err}");
            }
        }
        let raw = contact_snapshot(&name, ty, 0, -1, unix_now(), 0, 0, unix_now());
        if db
            .upsert_contact(
                &hex,
                &name,
                ty,
                0,
                -1,
                &[],
                unix_now(),
                0,
                0,
                unix_now(),
                &raw,
                false, // live discover → Roster NEW
                Some(radio_id),
            )
            .is_ok()
        {
            contact_qr::enqueue_contact_qr(&hex);
            if !existed {
                added += 1;
            }
        }
    }
    let _ = db.audit(
        "nodes_discover",
        "",
        &format!("{label}: {returned} returned, {added} new"),
    );
    let detail = if stopped {
        format!("Discover stopped — {returned} returned, {added} new.")
    } else {
        format!("Discover finished — {returned} returned, {added} new.")
    };
    end_discover_sync(status, detail);
    Ok(DiscoverSummary { returned, added })
}

async fn run_region_discover(
    db: &Db,
    meshcore: &MeshCore,
    status: &Mutex<RadioStatus>,
    scan_cancel: &AtomicBool,
) -> Result<Vec<String>, String> {
    let targets: Vec<[u8; 32]> = db
        .list_nodes()
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|n| n.contact_type == 2 || n.contact_type == 3)
        .filter_map(|n| parse_pubkey_full(&n.pubkey).ok())
        .take(REGION_SCAN_MAX_TARGETS)
        .collect();
    if targets.is_empty() {
        return Err("No repeater or room contacts in the roster to query.".into());
    }

    let total = targets.len() as u32;
    write_sync(
        status,
        SyncPhase::Discover,
        "Scanning regions…",
        Some((0, total)),
    );

    let mut found = Vec::new();
    let mut stopped = false;
    let dispatcher = meshcore.dispatcher();
    for (i, pubkey) in targets.iter().enumerate() {
        if scan_cancel.load(Ordering::SeqCst) {
            stopped = true;
            break;
        }
        let done = (i + 1) as u32;
        write_sync(
            status,
            SyncPhase::Discover,
            format!("Scanning regions… ({done}/{total})"),
            Some((done, total)),
        );
        let Ok(commands) =
            tokio::time::timeout(SEND_LOCK_WAIT, meshcore.commands().lock()).await
        else {
            continue;
        };
        match request_regions_from_contact(&commands, &dispatcher, pubkey).await {
            Ok(names) => {
                found = crate::mesh_regions::merge_region_lists(&found, &names);
            }
            Err(err) => {
                hq_log!(debug, "region scan target failed: {err}");
            }
        }
        drop(commands);
    }
    let n = found.len();
    let detail = if stopped {
        format!("Region scan stopped — {n} region{}.", if n == 1 { "" } else { "s" })
    } else {
        format!("Region scan finished — {n} region{}.", if n == 1 { "" } else { "s" })
    };
    end_discover_sync(status, detail);
    Ok(found)
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
) -> Result<(), String> {
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
            Ok(())
        }
        Err(err) => Err(err.to_string()),
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

/// Open the transport for one radio. BLE names/addresses use btleplug; USB uses a serial path.
async fn connect_transport(transport: &str, address: &str) -> Result<MeshCore, String> {
    match transport {
        "usb" => MeshCore::serial(address, usb_serial::DEFAULT_BAUD)
            .await
            .map_err(|err| clarify_usb_connect_error(&err.to_string())),
        "ble" => MeshCore::ble_connect(address)
            .await
            .map_err(|err| clarify_ble_connect_error(&err.to_string())),
        "tcp" => {
            let (host, port) = crate::tcp_radio::parse_tcp_address(address)?;
            MeshCore::tcp(&host, port)
                .await
                .map_err(|err| crate::tcp_radio::clarify_tcp_connect_error(&err.to_string()))
        }
        other => Err(format!("Unknown radio transport `{other}`.")),
    }
}

fn clarify_ble_connect_error(raw: &str) -> String {
    if is_explicit_companion_busy_error(raw) {
        return format_held_disconnect("ble", raw);
    }
    let lower = raw.to_ascii_lowercase();
    if lower.contains("device not found") || lower.contains("not found") {
        return format!(
            "{raw} The companion is not advertising yet — after a failed link it often needs a few seconds (or a power cycle) before Scan can see it again. Wait, then Scan / Connect."
        );
    }
    raw.to_string()
}

/// Early BLE handshake failures (APPSTART / missing advert). Auto-reconnect
/// hammers the radio and can knock it offline; stop the slot instead.
fn is_ble_handshake_failure(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    lower.contains("appstart")
        || (lower.contains("timeout")
            && (lower.contains("selfinfo")
                || lower.contains("self info")
                || lower.contains("reading companion")))
        || lower.contains("device not found")
        || lower.contains("meshcore device not found")
        || err.contains(meshcore_rs::BLE_NOT_PAIRED)
}

fn clarify_usb_connect_error(raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    if is_usb_permission_error(&lower) {
        return format!("{raw} {}", usb_permission_hint());
    }
    if lower.contains("no such file")
        || lower.contains("not found")
        || lower.contains("the system cannot find")
        || lower.contains("cannot find the file")
    {
        return format!(
            "{raw} Waiting to rematch the same USB companion (by-id / serial). \
Unplug and replug if needed, or Refresh ports."
        );
    }
    if lower.contains("access is denied") || lower.contains("sharing violation") {
        return format!(
            "{raw} Close any other app using this COM port (serial monitor, another MeshCore client), then try again."
        );
    }
    raw.to_string()
}

/// Expand USB session failures after the port already opened (protocol / SelfInfo).
fn clarify_usb_session_error(raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    if lower.contains("timeout") && (lower.contains("selfinfo") || lower.contains("self info")) {
        return format!(
            "{raw} The USB port opened, but companion protocol did not answer. \
MeshCore ships separate companion firmwares: BLE builds speak over Bluetooth; USB builds speak \
framed serial on this CDC port (often no BLE). Flash the USB companion image for this board \
if you need cable TX, or connect over BLE instead. Close other serial apps and the phone MeshCore app first."
        );
    }
    clarify_usb_connect_error(raw)
}

fn is_usb_permission_error(lower: &str) -> bool {
    lower.contains("permission denied")
        || lower.contains("operation not permitted")
        || (cfg!(windows) && lower.contains("access is denied"))
}

fn usb_permission_hint() -> &'static str {
    if cfg!(target_os = "linux") {
        "Add your user to the serial group (`sudo usermod -aG dialout $USER` on Debian/Ubuntu, or `uucp` on Arch/CachyOS), then log out and back in."
    } else if cfg!(target_os = "macos") {
        "Quit other serial apps, confirm System Settings allows this app to use USB accessories, then unplug/replug the radio."
    } else if cfg!(windows) {
        "Close other apps using this COM port. If it still fails, reinstall/update the USB CDC driver, then unplug/replug."
    } else {
        "Close other apps using this serial port, then unplug/replug the radio."
    }
}

/// Errors that will not clear with backoff (permissions, busy port, no companion protocol).
///
/// Missing-device / renumeration is recoverable: the slot rematches by USB identity.
fn is_fatal_usb_session_error(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    is_usb_permission_error(&lower)
        || lower.contains("sharing violation")
        || lower.contains("access is denied")
        || (lower.contains("timeout") && (lower.contains("selfinfo") || lower.contains("self info")))
}

/// How long a session must live before an early connect failure streak resets.
const EARLY_SESSION: Duration = Duration::from_secs(20);
/// Consecutive early held-link failures before we stop auto-reconnect.
const COMPANION_BUSY_STREAK_LIMIT: u8 = 3;

fn companion_held_message(transport: &str) -> &'static str {
    match transport {
        "ble" => {
            "Companion busy — disconnect the phone MeshCore app (or any other Bluetooth client) from this radio, then Connect again."
        }
        "usb" => {
            "Companion busy — close any other app using this serial port, then Connect again."
        }
        "tcp" => {
            "Companion busy — another client may hold this TCP session, or the companion is not listening. Free the link, then Connect again."
        }
        _ => "Companion busy — free the link, then Connect again.",
    }
}

/// Clear "someone else holds this companion" signals (port busy, GATT already taken).
fn is_explicit_companion_busy_error(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    lower.contains("sharing violation")
        || lower.contains("device or resource busy")
        || lower.contains("resource busy")
        || lower.contains("already connected")
        || lower.contains("br-connection-busy")
        || lower.contains("connection busy")
        || lower.contains("in use by another")
        || lower.contains("companion busy")
}

/// Early-session failures that usually mean the link is held elsewhere (or never came up).
fn looks_like_held_companion(transport: &str, err: &str) -> bool {
    if is_explicit_companion_busy_error(err) {
        return true;
    }
    let lower = err.to_ascii_lowercase();
    match transport {
        "ble" => {
            is_link_error(err)
                || lower.contains("failed to connect")
                || lower.contains("connection refused")
                || lower.contains("connection aborted")
                || lower.contains("timed out")
                || lower.contains("timeout")
                || lower.contains("gatt")
        }
        "tcp" => {
            lower.contains("connection refused")
                || lower.contains("connection reset")
                || lower.contains("broken pipe")
                || lower.contains("timed out")
                || lower.contains("timeout")
                || is_link_error(err)
        }
        "usb" => {
            lower.contains("sharing violation")
                || lower.contains("access is denied")
                || (lower.contains("timeout")
                    && (lower.contains("selfinfo") || lower.contains("self info")))
        }
        _ => false,
    }
}

fn format_held_disconnect(transport: &str, raw: &str) -> String {
    let hint = companion_held_message(transport);
    let lower = raw.to_ascii_lowercase();
    if lower.contains("companion busy") || lower.contains("disconnect the phone") {
        return raw.to_string();
    }
    if raw.trim().is_empty() {
        return hint.to_string();
    }
    format!("{raw} {hint}")
}

/// End the slot with a lasting status line (copied to idle so remove_slot keeps it).
fn end_slot_disconnected(ctx: &SlotCtx, detail: String) {
    clear_session_telemetry(&ctx.status);
    write_status(&ctx.status, "disconnected", detail.clone());
    write_status(&ctx.pool.idle, "disconnected", detail.clone());
    let _ = ctx.pool.db.set_radio_error(ctx.id, &detail);
}

/// Rescan USB ports and rewrite the slot open path when the OS renumerated the device.
async fn resolve_and_store_usb_path(ctx: &SlotCtx) -> Result<String, String> {
    let saved = ctx
        .pool
        .db
        .radio_address(ctx.id)
        .unwrap_or_else(|_| {
            ctx.address
                .lock()
                .unwrap_or_else(|err| err.into_inner())
                .clone()
        });
    let identity = ctx
        .pool
        .db
        .radio_usb_identity(ctx.id)
        .unwrap_or_default();
    let ports = tokio::task::spawn_blocking(usb_serial::list_usb_ports)
        .await
        .map_err(|err| format!("USB scan join failed: {err}"))?
        .map_err(|err| format!("USB scan failed: {err}"))?;
    {
        let mut idle = ctx
            .pool
            .idle
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        idle.usb_ports = ports.clone();
    }
    let identity_ref = identity.trim();
    let port = usb_serial::resolve_usb_target(
        &saved,
        (!identity_ref.is_empty()).then_some(identity_ref),
        &ports,
    )?;
    if port.path != saved {
        ctx.pool
            .db
            .remap_radio_address(ctx.id, &port.path)
            .map_err(|err| format!("Could not update USB address: {err}"))?;
    }
    if let Some(live_id) = port.identity.as_deref() {
        let _ = ctx.pool.db.set_radio_usb_identity(ctx.id, live_id);
    }
    *ctx.address.lock().unwrap_or_else(|err| err.into_inner()) = port.path.clone();
    Ok(port.path.clone())
}

/// Wait out a reconnect backoff, still honouring Disconnect. Returns false to end the slot.
async fn wait_or_stop(
    status: &Mutex<RadioStatus>,
    rx: &mut UnboundedReceiver<RadioCmd>,
    stop: &mut watch::Receiver<u64>,
    wait: Duration,
) -> bool {
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => return true,
            _ = stop.changed() => return false,
            cmd = rx.recv() => match cmd {
                None | Some(RadioCmd::Disconnect) => return false,
                Some(other) => reject_cmd(
                    other,
                    status,
                    "This radio is reconnecting. Try again once it is connected.",
                ),
            },
        }
    }
}

/// One task per radio: connect, run the session, reconnect with backoff until the
/// operator disconnects (or a held companion stops auto-retry).
async fn slot_loop(
    ctx: Arc<SlotCtx>,
    mut rx: UnboundedReceiver<RadioCmd>,
    mut stop: watch::Receiver<u64>,
) {
    let status = &ctx.status;
    let mut backoff = Duration::from_secs(2);
    let mut busy_streak: u8 = 0;
    loop {
        if ctx.transport == "usb" {
            match resolve_and_store_usb_path(&ctx).await {
                Ok(path) => {
                    write_status(status, "connecting", format!("Connecting to {path}"));
                    write_sync(
                        status,
                        SyncPhase::Connecting,
                        format!("Connecting to {path}…"),
                        None,
                    );
                }
                Err(err) => {
                    let detail = if err.to_ascii_lowercase().contains("not found")
                        || err.to_ascii_lowercase().contains("no usb")
                    {
                        "USB companion not found; waiting for the same device…".to_string()
                    } else {
                        err
                    };
                    clear_session_telemetry(status);
                    write_status(status, "reconnecting", detail.clone());
                    let _ = ctx.pool.db.set_radio_error(ctx.id, &detail);
                    let wait = backoff;
                    backoff = (backoff * 2).min(Duration::from_secs(30));
                    if !wait_or_stop(status, &mut rx, &mut stop, wait).await {
                        clear_session_telemetry(status);
                        write_status(status, "disconnected", "Disconnected.");
                        return;
                    }
                    continue;
                }
            }
        } else {
            let name = ctx
                .address
                .lock()
                .unwrap_or_else(|err| err.into_inner())
                .clone();
            write_status(status, "connecting", format!("Connecting to {name}"));
            write_sync(
                status,
                SyncPhase::Connecting,
                format!("Connecting to {name}…"),
                None,
            );
        }
        clear_session_telemetry(status);
        // Ignore Disconnect presses from before this session started.
        stop.borrow_and_update();
        let live: Mutex<Option<Arc<MeshCore>>> = Mutex::new(None);
        let started = Instant::now();
        let end = tokio::select! {
            end = run_session(&ctx, &mut rx, &live) => end,
            _ = stop.changed() => SessionEnd::Stopped,
        };
        ctx.set_link(false);
        let want_reboot = ctx.reboot_pending.swap(false, Ordering::SeqCst);
        // Always release the companion: stop meshcore-rs tasks and drop the
        // OS-level link. meshcore-rs disconnect() alone leaves a BLE GATT session
        // up, so Settings reboot looked like a no-op.
        let session = live.lock().unwrap_or_else(|err| err.into_inner()).take();
        if let Some(meshcore) = session {
            if want_reboot {
                let _ = send_companion_reboot(&meshcore).await;
            }
            let _ = meshcore.disconnect().await;
        }
        // Tear down OS GATT when the operator stops the slot (so the phone can
        // take the radio), or after an early BLE handshake failure (so the
        // companion can advertise again for Scan). Skipping force_disconnect on
        // ordinary Windows Lost+reconnect avoided a WinRT loop; handshake
        // failures now stop the slot instead of reconnecting.
        let handshake_lost = matches!(&end, SessionEnd::Lost(err) if is_ble_handshake_failure(err));
        let force_ble_drop = ctx.transport == "ble"
            && match &end {
                SessionEnd::Stopped => true,
                SessionEnd::Lost(_) if handshake_lost => true,
                SessionEnd::Lost(_) => {
                    #[cfg(target_os = "windows")]
                    {
                        false
                    }
                    #[cfg(not(target_os = "windows"))]
                    {
                        true
                    }
                }
            };
        if force_ble_drop {
            let name = ctx
                .address
                .lock()
                .unwrap_or_else(|err| err.into_inner())
                .clone();
            // Brief pause so meshcore-rs disconnect settles before WinRT drop.
            if handshake_lost {
                tokio::time::sleep(Duration::from_millis(800)).await;
            }
            match tokio::time::timeout(
                Duration::from_secs(8),
                crate::ble_pair::force_disconnect(&name),
            )
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(err)) => hq_log!(warn, "bluetooth disconnect: {err}"),
                Err(_) => hq_log!(warn, "bluetooth disconnect: timed out"),
            }
        }
        let wait = match end {
            SessionEnd::Stopped if want_reboot => {
                // Operator reboot: come back after the companion restarts.
                backoff = Duration::from_secs(5);
                clear_session_telemetry(status);
                write_status(status, "reconnecting", "Companion reboot sent. Reconnecting…");
                backoff
            }
            SessionEnd::Stopped => {
                clear_session_telemetry(status);
                write_status(status, "disconnected", "Disconnected.");
                return;
            }
            SessionEnd::Lost(error) => {
                let error = if ctx.transport == "usb" {
                    clarify_usb_session_error(&error)
                } else if ctx.transport == "ble" {
                    clarify_ble_connect_error(&error)
                } else {
                    error
                };
                if ctx.transport == "usb" && is_fatal_usb_session_error(&error) {
                    end_slot_disconnected(&ctx, error);
                    return;
                }
                // Do not auto-reconnect after APPSTART / not-found — that loop
                // overloaded companions and hid them from Scan until app restart.
                if ctx.transport == "ble" && is_ble_handshake_failure(&error) {
                    let detail = if error.contains(meshcore_rs::BLE_NOT_PAIRED) {
                        error
                    } else {
                        format!(
                            "{error} Auto-reconnect stopped so the radio can recover. \
Wait ~10 seconds, Scan, then Connect once."
                        )
                    };
                    end_slot_disconnected(&ctx, detail);
                    return;
                }
                let early = started.elapsed() <= EARLY_SESSION;
                if started.elapsed() > Duration::from_secs(30) {
                    backoff = Duration::from_secs(2);
                    busy_streak = 0;
                }
                let held = looks_like_held_companion(&ctx.transport, &error);
                if early && held {
                    busy_streak = busy_streak.saturating_add(1);
                } else if early {
                    busy_streak = 0;
                }
                // Explicit busy (port / GATT taken): stop now. Soft early failures
                // (timeout / not connected while another client likely holds the
                // radio): stop after a short streak so we do not reconnect forever.
                let stop_for_busy = is_explicit_companion_busy_error(&error)
                    || busy_streak >= COMPANION_BUSY_STREAK_LIMIT;
                if stop_for_busy && held {
                    end_slot_disconnected(&ctx, format_held_disconnect(&ctx.transport, &error));
                    return;
                }
                // Windows BLE needs more settle time after a dropped GATT session.
                #[cfg(target_os = "windows")]
                if ctx.transport == "ble" {
                    backoff = backoff.max(Duration::from_secs(8));
                }
                clear_session_telemetry(status);
                write_status(status, "reconnecting", error.clone());
                let _ = ctx.pool.db.set_radio_error(ctx.id, &error);
                let wait = backoff;
                backoff = (backoff * 2).min(Duration::from_secs(30));
                wait
            }
        };
        if !wait_or_stop(status, &mut rx, &mut stop, wait).await {
            clear_session_telemetry(status);
            write_status(status, "disconnected", "Disconnected.");
            return;
        }
    }
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

fn push_cli_line(status: &Mutex<RadioStatus>, line: impl Into<String>) {
    let mut guard = status.lock().unwrap_or_else(|err| err.into_inner());
    guard.cli_seq = guard.cli_seq.wrapping_add(1);
    guard.cli_line = line.into();
}

fn route_type_label(route: RouteType) -> &'static str {
    match route {
        RouteType::Flood => "flood",
        RouteType::Direct => "direct",
        RouteType::TransportFlood => "tflood",
        RouteType::TransportDirect => "tdirect",
    }
}

fn payload_type_label(payload: PayloadType) -> &'static str {
    match payload {
        PayloadType::Req => "req",
        PayloadType::Response => "resp",
        PayloadType::TextMsg => "dm",
        PayloadType::Ack => "ack",
        PayloadType::Advert => "advert",
        PayloadType::GroupText => "channel",
        PayloadType::GroupData => "ch-data",
        PayloadType::AnonReq => "anon",
        PayloadType::Path => "path",
        PayloadType::Trace => "trace",
        PayloadType::Multipart => "multi",
        PayloadType::Control => "ctrl",
        PayloadType::RawCustom => "raw",
        PayloadType::Unknown => "?",
    }
}

/// Human line for CLI RX log — no encrypted payload bytes.
pub(crate) fn format_rx_log_line(log: &LogData) -> String {
    let mut parts = Vec::with_capacity(6);
    parts.push(format!("SNR {:+.1}", log.snr));
    parts.push(format!("RSSI {} dBm", log.rssi));
    if let Some(header) = log.header.as_ref() {
        parts.push(route_type_label(header.route_type).to_string());
        parts.push(payload_type_label(header.payload_type).to_string());
        parts.push(format!("hops={}", header.path_len));
    } else {
        parts.push("header?".into());
    }
    if let Some(adv) = log.advertisement.as_ref() {
        if let Some(name) = adv.name.as_ref().map(|n| n.trim()).filter(|n| !n.is_empty()) {
            parts.push(name.to_string());
        } else {
            parts.push(hex::encode(&adv.public_key[..6]));
        }
    }
    parts.join(" · ")
}

fn push_rx_log_line(pool: &PoolShared, log: &LogData) {
    if !pool.rx_log_wanted.load(Ordering::SeqCst) {
        return;
    }
    let seq = pool.rx_log_seq.fetch_add(1, Ordering::SeqCst).wrapping_add(1);
    let line = RxLogLine {
        seq,
        at: unix_now(),
        text: format_rx_log_line(log),
    };
    let mut guard = pool.rx_log.lock().unwrap_or_else(|err| err.into_inner());
    guard.push_back(line);
    while guard.len() > RX_LOG_CAP {
        guard.pop_front();
    }
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

/// Full 32-byte identity key (required for binary status / noise probes).
fn parse_pubkey_full(hex_key: &str) -> Result<[u8; 32], String> {
    let cleaned: String = hex_key
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .collect();
    if cleaned.len() < 64 {
        return Err("Contact key must be 64 hex characters (32 bytes).".into());
    }
    let bytes = hex::decode(&cleaned[..64]).map_err(|err| err.to_string())?;
    let mut key = [0u8; 32];
    key.copy_from_slice(&bytes);
    Ok(key)
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

/// Write a frame and wait for one of `expected`, listening *before* the write.
///
/// meshcore-rs `send` / `send_multi` subscribe only after writing, so a fast
/// companion reply is lost and the call sits until its timeout while holding
/// the command lock. The caller must already hold `commands`, so replies to
/// other callers' frames cannot match here.
async fn request_listening_first(
    commands: &meshcore_rs::commands::CommandHandler,
    dispatcher: &meshcore_rs::events::EventDispatcher,
    data: &[u8],
    expected: &[EventType],
    timeout: Duration,
) -> Result<MeshCoreEvent, String> {
    let mut early_rx = dispatcher.receiver();
    tokio::select! {
        result = commands.send_multi(data, expected, timeout) => {
            result.map_err(|err| err.to_string())
        }
        event = async {
            loop {
                match early_rx.recv().await {
                    Ok(event) if expected.contains(&event.event_type) => return Ok(event),
                    Ok(event) => {
                        if let Some(err) = ble_not_paired_error(&event) {
                            return Err(err);
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(_) => return Err("Companion event stream closed.".to_string()),
                }
            }
        } => event,
    }
}

/// The BLE write task reports a refused (unbonded) link as an Error event so
/// the handshake fails at once instead of waiting out the APPSTART timeout.
fn ble_not_paired_error(event: &MeshCoreEvent) -> Option<String> {
    match (&event.event_type, &event.payload) {
        (EventType::Error, EventPayload::String(msg))
            if msg.starts_with(meshcore_rs::BLE_NOT_PAIRED) =>
        {
            Some(msg.clone())
        }
        _ => None,
    }
}

/// Auto-fetch, battery polls, and contact sync share the command lock; give up
/// instead of leaving an operator send hanging forever.
const SEND_LOCK_WAIT: Duration = Duration::from_secs(20);
/// Background work (battery, channel push, contact retry) must not park the
/// session loop; otherwise operator CLI/sends never get a turn.
const BRIEF_LOCK_WAIT: Duration = Duration::from_millis(750);
/// Noise probe is background RF sampling — never hold the lock as long as a send timeout.
const PROBE_STATUS_TIMEOUT: Duration = Duration::from_secs(10);
const PROBE_CONTACTS_TIMEOUT: Duration = Duration::from_secs(20);
const PROBE_MAX_TARGETS: usize = 5;

/// Persist a binary status sample (+ RF map sample) for Overview / Infrastructure.
const PROBE_NEIGHBOURS_TIMEOUT: Duration = Duration::from_secs(12);
const PROBE_NEIGHBOURS_COUNT: u8 = 32;
const PROBE_NEIGHBOURS_PREFIX_LEN: u8 = 6;

/// After a status probe, ask the claimed node for its neighbour table and store links.
async fn probe_and_persist_neighbours(db: &Db, meshcore: &MeshCore, pubkey_hex: &str) {
    let Ok(key) = parse_pubkey_full(pubkey_hex) else {
        return;
    };
    let commands = match tokio::time::timeout(
        Duration::from_secs(8),
        meshcore.commands().lock(),
    )
    .await
    {
        Ok(guard) => guard,
        Err(_) => return,
    };
    let result = commands
        .request_neighbours_with_timeout(
            Destination::Bytes(key.to_vec()),
            PROBE_NEIGHBOURS_COUNT,
            0,
            0,
            PROBE_NEIGHBOURS_PREFIX_LEN,
            PROBE_NEIGHBOURS_TIMEOUT,
        )
        .await;
    drop(commands);
    match result {
        Ok(data) => {
            let now = unix_now();
            let from = pubkey_hex.trim().to_ascii_lowercase();
            let links: Vec<crate::db::InfraNeighborLink> = data
                .neighbours
                .into_iter()
                .filter_map(|n| {
                    if n.pubkey.is_empty() {
                        return None;
                    }
                    Some(crate::db::InfraNeighborLink {
                        from_pubkey: from.clone(),
                        neighbor_prefix: hex::encode(&n.pubkey),
                        snr: f64::from(n.snr),
                        secs_ago: i64::from(n.secs_ago),
                        probed_at: now,
                    })
                })
                .collect();
            let n = links.len();
            if let Err(err) = db.replace_infra_neighbors(&from, &links) {
                hq_log!(debug, "persist neighbours {from}: {err}");
                return;
            }
            let summary = if n == 0 {
                format!("{} reported (none in range)", data.total)
            } else {
                let sample: Vec<String> = links
                    .iter()
                    .take(4)
                    .map(|l| l.neighbor_prefix.chars().take(8).collect())
                    .collect();
                format!("{} reported · {}", data.total, sample.join(", "))
            };
            let _ = db.update_infra_status_cli_fields(
                &from,
                None,
                None,
                None,
                None,
                Some(&summary),
            );
            hq_log!(
                debug,
                "neighbours {}: {n} link(s)",
                &from.chars().take(12).collect::<String>()
            );
        }
        Err(err) => {
            hq_log!(
                debug,
                "neighbours probe {}: {err}",
                &pubkey_hex.chars().take(12).collect::<String>()
            );
        }
    }
}

pub fn persist_infra_status_probe(
    db: &Db,
    pubkey: &str,
    info: &meshcore_rs::events::StatusData,
    prior_tx_power_dbm: Option<i64>,
) {
    let raw = json!({
        "battery_mv": info.battery_mv,
        "tx_queue_len": info.tx_queue_len,
        "noise_floor": info.noise_floor,
        "last_rssi": info.last_rssi,
        "nb_recv": info.nb_recv,
        "nb_sent": info.nb_sent,
        "airtime": info.airtime,
        "uptime": info.uptime,
        "flood_sent": info.flood_sent,
        "direct_sent": info.direct_sent,
        "snr": info.snr,
        "dup_count": info.dup_count,
        "rx_airtime": info.rx_airtime,
    })
    .to_string();
    let sample = InfraStatusSample {
        pubkey: pubkey.to_string(),
        at: unix_now(),
        battery_mv: i64::from(info.battery_mv),
        noise_floor: i64::from(info.noise_floor),
        last_rssi: i64::from(info.last_rssi),
        snr: f64::from(info.snr),
        airtime_ms: i64::from(info.airtime),
        rx_airtime_ms: i64::from(info.rx_airtime),
        uptime_s: i64::from(info.uptime),
        nb_recv: i64::from(info.nb_recv),
        nb_sent: i64::from(info.nb_sent),
        flood_sent: i64::from(info.flood_sent),
        direct_sent: i64::from(info.direct_sent),
        tx_power_dbm: prior_tx_power_dbm,
        tx_queue_len: Some(i64::from(info.tx_queue_len)),
        raw_json: raw,
    };
    let _ = db.insert_infra_status_sample(&sample, None);
    let _ = db.insert_rf_sample(pubkey, info.noise_floor, info.last_rssi, info.snr);
}

/// Connect-time / off-interval freshness: skip nodes probed within this window.
const CLAIMED_PROBE_FRESH_DEFAULT_SECS: i64 = 15 * 60;

async fn run_claimed_infra_status_sweep(
    db: Arc<Db>,
    meshcore: Arc<MeshCore>,
    status: Arc<Mutex<RadioStatus>>,
) {
    let max_targets = db.infra_probe_max_targets();
    let interval_mins = db.infra_probe_interval_mins();
    let fresh_secs = if interval_mins > 0 {
        interval_mins * 60
    } else {
        CLAIMED_PROBE_FRESH_DEFAULT_SECS
    };
    let now = unix_now();
    let mut targets: Vec<String> = db
        .list_claimed_infra()
        .unwrap_or_default()
        .into_iter()
        .filter(|row| row.is_verified())
        .filter(|row| {
            match db.get_infra_status_latest(&row.pubkey).ok().flatten() {
                Some(latest) => now - latest.probed_at >= fresh_secs,
                None => true,
            }
        })
        .map(|row| row.pubkey)
        .take(max_targets)
        .collect();
    if targets.is_empty() {
        return;
    }
    write_status(
        &status,
        "connected",
        format!(
            "Auto-probing {} claimed infrastructure node(s)…",
            targets.len()
        ),
    );
    let mut ok = 0usize;
    for (i, pubkey) in targets.drain(..).enumerate() {
        if i > 0 {
            tokio::time::sleep(Duration::from_millis(1500)).await;
        }
        let prior_tx = db
            .get_infra_status_latest(&pubkey)
            .ok()
            .flatten()
            .and_then(|r| r.tx_power_dbm);
        match run_infra_status_probe(meshcore.clone(), status.clone(), &pubkey).await {
            Ok(info) => {
                persist_infra_status_probe(&db, &pubkey, &info, prior_tx);
                tokio::task::yield_now().await;
                probe_and_persist_neighbours(&db, &meshcore, &pubkey).await;
                ok += 1;
            }
            Err(err) => {
                hq_log!(
                    debug,
                    "claimed status auto-probe {}: {err}",
                    &pubkey.chars().take(12).collect::<String>()
                );
            }
        }
    }
    let name = {
        let guard = status.lock().unwrap_or_else(|err| err.into_inner());
        if guard.connected_name.is_empty() {
            "companion".into()
        } else {
            guard.connected_name.clone()
        }
    };
    write_status(
        &status,
        "connected",
        connected_detail(&status, &name),
    );
    if ok > 0 {
        hq_log!(info, "claimed infrastructure auto-probe: {ok} updated");
    }
}

/// Status probes are mesh DMs. Acquire the command lock per step and release
/// between contacts so channel sends / CLI stay responsive.
async fn run_infra_status_probe(
    meshcore: Arc<MeshCore>,
    status: Arc<Mutex<RadioStatus>>,
    pubkey_hex: &str,
) -> Result<meshcore_rs::events::StatusData, String> {
    run_binary_status_probe(
        meshcore,
        status,
        pubkey_hex,
        "Refreshing infrastructure status",
        "Infrastructure status refreshed.",
        "Status refresh",
    )
    .await
}

/// Binary Status (0x01) over the mesh — the practical MeshCore “ping”.
async fn run_binary_status_probe(
    meshcore: Arc<MeshCore>,
    status: Arc<Mutex<RadioStatus>>,
    pubkey_hex: &str,
    busy_prefix: &str,
    ok_line: &str,
    err_prefix: &str,
) -> Result<meshcore_rs::events::StatusData, String> {
    let key = parse_pubkey_full(pubkey_hex)?;
    write_status(
        &status,
        "connected",
        format!(
            "{busy_prefix} ({})…",
            &pubkey_hex.chars().take(12).collect::<String>()
        ),
    );
    let commands = match tokio::time::timeout(
        Duration::from_secs(8),
        meshcore.commands().lock(),
    )
    .await
    {
        Ok(guard) => guard,
        Err(_) => {
            let msg = format!("{err_prefix} skipped — radio busy.");
            write_status(&status, "connected", msg.clone());
            return Err(msg);
        }
    };
    let result = commands
        .request_status_with_timeout(Destination::Bytes(key.to_vec()), PROBE_STATUS_TIMEOUT)
        .await;
    drop(commands);
    match result {
        Ok(info) => {
            write_status(&status, "connected", ok_line.to_string());
            Ok(info)
        }
        Err(err) => {
            let msg = format!("{err_prefix} failed: {err}");
            write_status(&status, "connected", msg.clone());
            Err(msg)
        }
    }
}

/// Operator ping: longer MsgSent + Status waits than background infra probes.
const PING_MSG_SENT_TIMEOUT: Duration = Duration::from_secs(15);
const PING_STATUS_MAX: Duration = Duration::from_secs(60);

async fn run_node_ping(
    db: &Db,
    meshcore: Arc<MeshCore>,
    status: Arc<Mutex<RadioStatus>>,
    pubkey_hex: &str,
) -> Result<meshcore_rs::events::StatusData, String> {
    trace_rate_allows(pubkey_hex)?;
    let key = parse_pubkey_full(pubkey_hex)?;
    let short: String = pubkey_hex.chars().take(12).collect();
    write_status(
        &status,
        "connected",
        format!("Pinging ({short})…"),
    );
    // Refresh companion advert-path cache into HQ *before* we touch the contact
    // table. MeshCore cannot flood binary Status requests; we need a real route
    // or a direct (0-hop) contact — never path_len=-1.
    match fetch_advert_path(db, meshcore.as_ref(), pubkey_hex, None, "operator").await {
        Ok(fetch) => {
            if fetch.path.is_empty() && fetch.hop_count == 0 {
                let _ = db.set_route(pubkey_hex, &[]);
            } else if !fetch.path.is_empty() {
                let _ = db.set_route(pubkey_hex, &fetch.path);
            }
            hq_log!(
                debug,
                "ping path source={} hops={} for {short}",
                fetch.source,
                fetch.hop_count
            );
        }
        Err(err) => hq_log!(debug, "ping path prefetch: {err}"),
    }
    let mut commands = match tokio::time::timeout(SEND_LOCK_WAIT, meshcore.commands().lock()).await
    {
        Ok(guard) => guard,
        Err(_) => {
            let msg =
                "Ping skipped — radio busy (wait for probes/sends, then try again).".to_string();
            write_status(&status, "connected", msg.clone());
            return Err(msg);
        }
    };
    let route_mode = match ensure_radio_contact_for_probe(db, &commands, pubkey_hex).await {
        Ok(mode) => {
            hq_log!(debug, "ping radio contact mode={mode} for {short}");
            mode
        }
        Err(err) => {
            hq_log!(debug, "ping ensure contact: {err}");
            "unknown"
        }
    };
    // MsgSent uses default_timeout (normally 5s) — too tight when the companion
    // is mid-probe. Status wait registers for the same window (meshcore-rs).
    commands.set_default_timeout(PING_MSG_SENT_TIMEOUT);
    let result = commands
        .request_status_with_timeout(Destination::Bytes(key.to_vec()), PING_STATUS_MAX)
        .await;
    commands.set_default_timeout(Duration::from_secs(5));
    drop(commands);
    match result {
        Ok(info) => {
            write_status(&status, "connected", "Ping ok.");
            Ok(info)
        }
        Err(err) => {
            let hint = match route_mode {
                "route" | "direct" => {
                    "no status reply (out of range, asleep, or path stale — try Trace / hear advert)"
                        .to_string()
                }
                "unchanged" => {
                    "no cached unicast route yet (Status cannot flood — hear their advert or Trace first)"
                        .to_string()
                }
                _ => "no status reply".to_string(),
            };
            let msg = format!("Ping failed: {err} — {hint}");
            write_status(&status, "connected", msg.clone());
            Err(msg)
        }
    }
}

async fn run_noise_probe(db: Arc<Db>, meshcore: Arc<MeshCore>, status: Arc<Mutex<RadioStatus>>) {
    write_status(
        &status,
        "connected",
        "Noise probe: listing radio contacts…",
    );
    let radio_keys = {
        let Ok(mut commands) =
            tokio::time::timeout(SEND_LOCK_WAIT, meshcore.commands().lock()).await
        else {
            write_status(
                &status,
                "connected",
                "Noise probe skipped — radio busy (try again after sending).",
            );
            return;
        };
        let prev_timeout = Duration::from_secs(5);
        commands.set_default_timeout(Duration::from_secs(12));
        let result = commands
            .get_contacts_with_timeout(0, PROBE_CONTACTS_TIMEOUT)
            .await;
        commands.set_default_timeout(prev_timeout);
        match result {
            Ok(list) => list
                .into_iter()
                .map(|c| c.public_key_hex().to_ascii_lowercase())
                .collect::<std::collections::HashSet<_>>(),
            Err(err) => {
                write_status(
                    &status,
                    "connected",
                    format!("Noise probe aborted (contact list): {err}"),
                );
                return;
            }
        }
    };
    // Yield so a queued SendChannel can take the lock before we probe.
    tokio::task::yield_now().await;

    let mut candidates = db
        .list_nodes()
        .unwrap_or_default()
        .into_iter()
        .filter(|n| {
            n.has_gps
                && n.enrollment != crate::signup::Enrollment::Hq
                && radio_keys.contains(&n.pubkey.to_ascii_lowercase())
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|a, b| {
        let ae = matches!(a.enrollment, crate::signup::Enrollment::Enrolled);
        let be = matches!(b.enrollment, crate::signup::Enrollment::Enrolled);
        be.cmp(&ae).then(
            b.last_heard
                .unwrap_or(0)
                .cmp(&a.last_heard.unwrap_or(0)),
        )
    });
    candidates.truncate(PROBE_MAX_TARGETS);
    if candidates.is_empty() {
        write_status(
            &status,
            "connected",
            "No GPS nodes on the radio contact table to probe.",
        );
        return;
    }
    write_status(
        &status,
        "connected",
        format!("Noise probe: asking {} contact(s)…", candidates.len()),
    );

    let mut ok = 0usize;
    let mut failed = 0usize;
    for node in candidates {
        let key = match parse_pubkey_full(&node.pubkey) {
            Ok(k) => k,
            Err(err) => {
                hq_log!(warn, "noise probe {}: {err}", node.name);
                failed += 1;
                continue;
            }
        };
        // Prefer operator traffic: wait briefly, then skip this target if busy.
        let commands = match tokio::time::timeout(
            Duration::from_secs(3),
            meshcore.commands().lock(),
        )
        .await
        {
            Ok(guard) => guard,
            Err(_) => {
                hq_log!(
                    warn,
                    "noise probe {}: skipped (radio busy)",
                    node.name
                );
                failed += 1;
                tokio::task::yield_now().await;
                continue;
            }
        };
        let result = commands
            .request_status_with_timeout(
                Destination::Bytes(key.to_vec()),
                PROBE_STATUS_TIMEOUT,
            )
            .await;
        drop(commands);
        match result {
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
                failed += 1;
                hq_log!(warn, "noise probe {}: {err}", node.name);
            }
        }
        // Gap so SendChannel's lock wait can win between targets.
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    write_status(
        &status,
        "connected",
        if failed == 0 {
            format!("Noise probe stored {ok} sample(s).")
        } else {
            format!(
                "Noise probe stored {ok} sample(s); {failed} timed out or unreachable."
            )
        },
    );
}

async fn lock_commands_brief(
    meshcore: &MeshCore,
) -> Option<tokio::sync::MutexGuard<'_, meshcore_rs::commands::CommandHandler>> {
    tokio::time::timeout(BRIEF_LOCK_WAIT, meshcore.commands().lock())
        .await
        .ok()
}

async fn lock_commands_for_operator(
    meshcore: &MeshCore,
) -> Result<tokio::sync::MutexGuard<'_, meshcore_rs::commands::CommandHandler>, String> {
    tokio::time::timeout(SEND_LOCK_WAIT, meshcore.commands().lock())
        .await
        .map_err(|_| "Radio is busy (sync in progress). Try again in a moment.".to_string())
}

async fn send_companion_reboot(meshcore: &MeshCore) -> Result<(), String> {
    let commands = lock_commands_for_operator(meshcore).await?;
    let result = commands.reboot().await.map_err(|err| err.to_string());
    if result.is_ok() {
        // Give the write task a beat to flush before we tear the link down.
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    result
}

fn push_send_note(status: &Mutex<RadioStatus>, note: impl Into<String>) {
    let mut guard = status.lock().unwrap_or_else(|err| err.into_inner());
    guard.send_seq = guard.send_seq.wrapping_add(1);
    guard.send_note = note.into();
}

fn bump_rx_seq(status: &Mutex<RadioStatus>) {
    let mut guard = status.lock().unwrap_or_else(|err| err.into_inner());
    guard.rx_seq = guard.rx_seq.wrapping_add(1);
}

/// Soft operator hint while live RX drain is deferred (does not change `state`).
fn set_receiving_busy_hint(status: &Mutex<RadioStatus>, busy: bool) {
    let mut guard = status.lock().unwrap_or_else(|err| err.into_inner());
    if busy {
        if guard.state == "connected" {
            guard.detail = "Receiving… radio busy, retrying.".into();
        }
    } else if guard.detail.starts_with("Receiving…") {
        // Restore normal connected blurb without inventing a new phase.
        let name = guard.connected_name.clone();
        drop(guard);
        if !name.is_empty() {
            write_status(status, "connected", connected_detail(status, &name));
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MsgDrainResult {
    /// Companion reported no more queued messages.
    Cleared,
    /// Command lock busy — retry soon (do not clear MessagesWaiting).
    Busy,
    /// Fetched some frames but hit budget / error before empty.
    MorePending,
}

/// Whether the session should keep retrying the companion message queue.
fn need_msg_drain_after(result: MsgDrainResult) -> bool {
    !matches!(result, MsgDrainResult::Cleared)
}

/// `CMD_SYNC_NEXT_MESSAGE` (0x0A) with listen-before-write. meshcore-rs
/// `get_msg` uses `send_multi` (subscribe after write); on fast USB CDC the
/// reply can land before the waiter attaches and the call times out while
/// holding the lock.
async fn get_msg_listening_first(
    commands: &meshcore_rs::commands::CommandHandler,
    dispatcher: &meshcore_rs::events::EventDispatcher,
    timeout: Duration,
) -> Result<Option<MeshCoreEvent>, String> {
    const CMD_SYNC_NEXT_MESSAGE: u8 = 0x0A;
    let event = request_listening_first(
        commands,
        dispatcher,
        &[CMD_SYNC_NEXT_MESSAGE],
        &[
            EventType::ContactMsgRecv,
            EventType::ChannelMsgRecv,
            EventType::NoMoreMessages,
            EventType::Error,
        ],
        timeout,
    )
    .await?;
    match event.event_type {
        EventType::ContactMsgRecv | EventType::ChannelMsgRecv => Ok(Some(event)),
        EventType::NoMoreMessages => Ok(None),
        EventType::Error => match event.payload {
            EventPayload::String(msg) => Err(msg),
            _ => Err("get_msg error".into()),
        },
        other => Err(format!("unexpected get_msg reply: {other:?}")),
    }
}

/// Pull queued frames off the companion. Each message is also broadcast to our
/// ChannelMsgRecv / ContactMsgRecv subscriptions, which handle DB insert — so
/// this only drives `CMD_SYNC_NEXT_MESSAGE` and must not insert again.
///
/// Live RX depends on this: companion text is queued until `get_msg`, and
/// `PUSH_CODE_MSG_WAITING` (0x83) is coalescing — a missed drain leaves the
/// queue stuck until reconnect (GitHub #2).
///
/// `from_waiting`: true when draining in response to `MessagesWaiting` (field
/// logs Busy at info so `--dev` / log file catch deferred drains).
async fn drain_pending_messages(
    meshcore: &MeshCore,
    radio: i64,
    from_waiting: bool,
) -> MsgDrainResult {
    // MessagesWaiting / deferred live drain uses the operator lock budget so
    // background brief work cannot starve RX. Passive 2s polls stay brief.
    let lock_wait = if from_waiting {
        SEND_LOCK_WAIT
    } else {
        BRIEF_LOCK_WAIT
    };
    let Ok(commands) = tokio::time::timeout(lock_wait, meshcore.commands().lock()).await else {
        if from_waiting {
            hq_log!(
                info,
                "radio {radio}: MessagesWaiting drain deferred (radio busy) — will retry"
            );
        } else {
            hq_log!(debug, "radio {radio}: msg drain deferred: radio busy");
        }
        return MsgDrainResult::Busy;
    };
    let mut fetched = 0u32;
    const BUDGET: u32 = 24;
    loop {
        if fetched >= BUDGET {
            hq_log!(
                info,
                "radio {radio}: msg drain paused after {fetched} (more may remain)"
            );
            return MsgDrainResult::MorePending;
        }
        match get_msg_listening_first(
            &commands,
            meshcore.dispatcher(),
            Duration::from_millis(900),
        )
        .await
        {
            Ok(Some(_event)) => {
                fetched = fetched.saturating_add(1);
            }
            Ok(None) => {
                if fetched > 0 {
                    hq_log!(info, "radio {radio}: msg drain cleared ({fetched})");
                } else {
                    hq_log!(debug, "radio {radio}: msg drain empty");
                }
                return MsgDrainResult::Cleared;
            }
            Err(err) => {
                hq_log!(warn, "radio {radio}: get_msg: {err}");
                return MsgDrainResult::MorePending;
            }
        }
    }
}

/// MeshCore `CMD_SEND_CHANNEL_TXT_MSG` (0x03). Firmware replies `RESP_CODE_OK` /
/// `RESP_CODE_ERR` — subscribe before write so a fast OK is not missed.
async fn send_channel_text(
    meshcore: &MeshCore,
    slot: u8,
    wire: &str,
    timestamp: u32,
) -> Result<(), String> {
    let mut data = vec![0x03u8, 0x00, slot];
    data.extend_from_slice(&timestamp.to_le_bytes());
    data.extend_from_slice(wire.as_bytes());

    hq_log!(debug, "channel send: waiting for command lock (slot {slot}, {} bytes)", wire.len());
    let commands = lock_commands_for_operator(meshcore).await?;
    hq_log!(debug, "channel send: lock acquired, writing frame");
    let result = request_listening_first(
        &commands,
        meshcore.dispatcher(),
        &data,
        &[EventType::Ok, EventType::Error],
        Duration::from_secs(5),
    )
    .await;
    drop(commands);

    match result? {
        event if event.event_type == EventType::Error => match event.payload {
            EventPayload::String(s) => Err(s),
            _ => Err("Companion rejected the channel send (bad slot or radio busy).".into()),
        },
        _ => Ok(()),
    }
}

async fn transmit_channel_message(
    db: &Arc<Db>,
    meshcore: &MeshCore,
    status: &Mutex<RadioStatus>,
    connected_name: &str,
    radio_id: i64,
    slot: u8,
    text: &str,
    is_primary: bool,
    bot_activity: bool,
) -> Result<(), String> {
    let text = text.trim();
    if text.is_empty() {
        let msg = "Message is empty.";
        push_send_note(status, msg);
        return Err(msg.into());
    }
    let advert_name = db.hq_advert_name().unwrap_or_else(|_| "hq".into());
    let wire = prefix_channel_sender(&advert_name, text);
    if wire.len() > 160 {
        let msg = "Mesh text is limited to 160 bytes after adding the device name.";
        push_send_note(status, msg);
        write_status(status, "connected", msg);
        return Err(msg.into());
    }
    let timestamp = unix_now() as u32;
    let origin = if bot_activity { "bot" } else { "local" };
    match send_channel_text(meshcore, slot, &wire, timestamp).await {
        Ok(()) => {
            let raw = json!({
                "channel": slot,
                "text": wire,
                "direction": "tx",
                "origin": origin,
            });
            match db.insert_message(
                "channel",
                "tx",
                None,
                &advert_name,
                Some(slot as i64),
                Some(0),
                timestamp,
                &wire,
                &wire,
                None,
                None,
                &raw,
                "sent",
                Some(radio_id),
                origin,
            ) {
                Ok(None) => {
                    // Dedupe hit — still treat as sent so the UI refreshes.
                    let _ = db.audit("channel_send", "", &format!("slot {slot} (deduped)"));
                    push_send_note(status, String::new());
                    write_status(status, "connected", connected_detail(status, connected_name));
                }
                Ok(Some(_)) => {
                    let _ = db.audit("channel_send", "", &format!("slot {slot}"));
                    push_send_note(status, String::new());
                    write_status(status, "connected", connected_detail(status, connected_name));
                }
                Err(err) => {
                    let msg = format!("Sent on air, but could not save history: {err}");
                    push_send_note(status, &msg);
                    return Err(msg);
                }
            }
            // Companion does not echo our own TX back as ChannelMsgRecv — run
            // METIS-INFO bot commands locally so the operator can test from HQ.
            if is_primary {
                maybe_handle_local_metis_command(db, slot, text);
            }
            Ok(())
        }
        Err(err) => {
            let msg = format!("Send failed: {err}");
            push_send_note(status, &msg);
            write_status(status, "connected", &msg);
            Err(msg)
        }
    }
}

fn maybe_handle_local_metis_command(db: &Arc<Db>, slot: u8, text: &str) {
    if !crate::services::is_metis_info_slot(db, slot) {
        return;
    }
    let (_, body) = channel_sender_and_body(text);
    let Some(cmd) = crate::services::parse_bot_command(&body) else {
        return;
    };
    // Operator sends use wall-clock seconds so a deliberate second #weather works,
    // but an identical retry in the same second is ignored.
    let claim = bot_command_claim_id(slot, unix_now() as u32, &body);
    match db.claim_bot_command(&claim) {
        Ok(true) => {
            let hq = db.hq_node().ok().flatten();
            handle_bot_channel(db, slot, hq.as_ref(), cmd, true);
        }
        Ok(false) => hq_log!(debug, "local METIS-INFO cmd already handled ({claim})"),
        Err(err) => hq_log!(warn, "bot claim: {err}"),
    }
}

fn bot_command_claim_id(slot: u8, sender_timestamp: u32, body: &str) -> String {
    format!(
        "ch:{slot}:{sender_timestamp}:{}",
        body.trim().to_ascii_lowercase()
    )
}

/// Companion `CMD_RUN_CLI_COMMAND` (66). Reply is code 29 → EventType::Unknown + Bytes.
/// Wire body is `xx|command` (protocol v14+), matching mesh-client / firmware.
async fn apply_local_cli(
    meshcore: &MeshCore,
    text: &str,
    protocol_ver: u8,
) -> Result<String, String> {
    if protocol_ver > 0 && protocol_ver < 14 {
        return Err(format!(
            "Companion protocol v{protocol_ver} — local CLI needs v14+. Update the companion firmware."
        ));
    }
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

    let commands = lock_commands_for_operator(meshcore).await?;
    let event = request_listening_first(
        &commands,
        meshcore.dispatcher(),
        &data,
        &[EventType::Unknown, EventType::Error, EventType::Ok],
        Duration::from_secs(12),
    )
    .await;
    drop(commands);

    let event = event.map_err(|err| {
        format!("{err} — no CLI reply. Needs protocol v14+, and the radio must not be mid-sync.")
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

/// Remote CLI: CMD_SEND_TXT_MSG. `txt_type` 3 = companion remote; 1 = infra CLI_DATA.
async fn apply_remote_cli(
    commands: &meshcore_rs::commands::CommandHandler,
    prefix: [u8; 6],
    text: &str,
    txt_type: u8,
) -> Result<String, String> {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as u32)
        .unwrap_or(0);
    let mut data = vec![0x02u8, txt_type, 0x00]; // SEND_TXT, type, attempt
    data.extend_from_slice(&ts.to_le_bytes());
    data.extend_from_slice(&prefix);
    data.extend_from_slice(text.as_bytes());
    commands
        .send_with_timeout(&data, Some(EventType::MsgSent), Duration::from_secs(10))
        .await
        .map_err(|err| err.to_string())?;
    // Empty: do not push a send-ack onto cli_line. The real answer arrives later as
    // CLI_DATA (txt_type 1) and must be the next line infra capture sees.
    Ok(String::new())
}

const INFRA_LOGIN_TIMEOUT: Duration = Duration::from_secs(25);
const INFRA_ACL_TIMEOUT: Duration = Duration::from_secs(20);

/// `CMD_SEND_LOGIN` then prove admin via binary ACL request (guest must not pass).
async fn apply_infra_login(
    meshcore: &MeshCore,
    pubkey_hex: &str,
    password: &str,
) -> Result<(), String> {
    let key = parse_pubkey_full(pubkey_hex)?;
    let password = password.trim();
    if password.is_empty() {
        return Err("Admin password required.".into());
    }
    if password.as_bytes().len() > 15 {
        return Err("Password max 15 bytes (MeshCore limit).".into());
    }

    let commands = lock_commands_for_operator(meshcore).await?;
    let mut early_rx = meshcore.dispatcher().receiver();

    // Listen before send so a fast LOGIN_SUCCESS is not missed.
    let send_result = commands
        .send_login(Destination::Bytes(key.to_vec()), password)
        .await
        .map_err(|err| format!("Login send failed: {err}"));
    if let Err(err) = send_result {
        return Err(err);
    }

    let login_outcome = tokio::time::timeout(INFRA_LOGIN_TIMEOUT, async {
        loop {
            match early_rx.recv().await {
                Ok(event) if event.event_type == EventType::LoginSuccess => {
                    return Ok(());
                }
                Ok(event) if event.event_type == EventType::LoginFailed => {
                    return Err(
                        "Login failed — wrong password or node unreachable.".to_string(),
                    );
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(_) => return Err("Companion event stream closed.".into()),
            }
        }
    })
    .await;

    match login_outcome {
        Ok(Ok(())) => {}
        Ok(Err(err)) => return Err(err),
        Err(_) => return Err("Login timed out — no reply from node.".into()),
    }

    // Admin proof: guests should not receive ACL. Password never appears here.
    match commands
        .request_acl_with_timeout(Destination::Bytes(key.to_vec()), INFRA_ACL_TIMEOUT)
        .await
    {
        Ok(_) => Ok(()),
        Err(_) => Err(
            "Admin password required (guest login or ACL unread).".into(),
        ),
    }
}

enum SessionEnd {
    Stopped,
    Lost(String),
}

/// The companion's own identity from `appstart`. Only the primary writes it as the HQ identity.
struct SelfIdentity {
    pubkey: String,
    name: String,
    adv_lat: i32,
    adv_lon: i32,
    raw: serde_json::Value,
}

/// APPSTART frame, same bytes as meshcore_py. Firmware requires len >= 8 and
/// ignores bytes 1–7; bytes 8+ are the app name it logs.
const APPSTART_FRAME: [u8; 13] = [
    1, // CMD_APP_START
    3, b' ', b' ', b' ', b' ', b' ', b' ', // reserved (meshcore_py shape)
    b'm', b'c', b'c', b'l', b'i',
];

fn appstart_timeout(transport: &str) -> Duration {
    if transport == "ble" {
        // One attempt only on Windows — a long double-timeout was holding GATT
        // open long enough to reset some companions.
        if cfg!(target_os = "windows") {
            Duration::from_secs(12)
        } else {
            Duration::from_secs(10)
        }
    } else {
        Duration::from_secs(8)
    }
}

fn clarify_appstart_timeout(transport: &str, raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    if !(lower.contains("timeout")
        && (lower.contains("selfinfo")
            || lower.contains("self info")
            || lower.contains("response")
            || lower.contains("appstart")))
    {
        return raw.to_string();
    }
    if transport == "ble" {
        return format!(
            "{raw} Bluetooth linked, but the companion never answered APPSTART. \
Close the phone MeshCore app so it is not holding the radio. If the radio was re-flashed or its PIN \
changed, remove it from the system Bluetooth settings, then click Pair and enter the PIN. \
Wait ~10s, then Connect once. Avoid running HQ elevated (admin)."
        );
    }
    clarify_usb_session_error(raw)
}

async fn companion_appstart_once(
    commands: &meshcore_rs::commands::CommandHandler,
    dispatcher: &meshcore_rs::events::EventDispatcher,
    timeout: Duration,
) -> Result<meshcore_rs::events::SelfInfo, String> {
    let event = request_listening_first(
        commands,
        dispatcher,
        &APPSTART_FRAME,
        &[EventType::SelfInfo],
        timeout,
    )
    .await?;
    match event.payload {
        EventPayload::SelfInfo(info) => Ok(info),
        _ => Err("Unexpected response to APPSTART.".into()),
    }
}

/// APPSTART with listen-before-write. meshcore-rs `send_appstart` subscribes only
/// after the write, so a fast SelfInfo is dropped and we sit until timeout —
/// common on Windows BLE right after GATT open.
///
/// Windows: single attempt only. A second APPSTART while GATT is wedged held the
/// link open for ~30s and some companions drop offline / stop advertising.
async fn companion_appstart(
    commands: &meshcore_rs::commands::CommandHandler,
    dispatcher: &meshcore_rs::events::EventDispatcher,
    transport: &str,
) -> Result<meshcore_rs::events::SelfInfo, String> {
    let timeout = appstart_timeout(transport);
    match companion_appstart_once(commands, dispatcher, timeout).await {
        Ok(info) => Ok(info),
        Err(err)
            if transport == "ble"
                && !cfg!(target_os = "windows")
                && !err.contains(meshcore_rs::BLE_NOT_PAIRED) =>
        {
            hq_log!(warn, "appstart: {err}; settling and retrying once");
            tokio::time::sleep(Duration::from_millis(600)).await;
            companion_appstart_once(commands, dispatcher, timeout)
                .await
                .map_err(|e| clarify_appstart_timeout(transport, &e))
        }
        Err(err) => Err(clarify_appstart_timeout(transport, &err)),
    }
}

/// Device query with listen-before-write (same race as APPSTART).
async fn companion_device_query(
    commands: &meshcore_rs::commands::CommandHandler,
    dispatcher: &meshcore_rs::events::EventDispatcher,
    transport: &str,
) -> Result<meshcore_rs::events::DeviceInfoData, String> {
    let data = [22u8, 8]; // CMD_DEVICE_QUERY + protocol version
    let timeout = appstart_timeout(transport);
    let event = request_listening_first(
        commands,
        dispatcher,
        &data,
        &[EventType::DeviceInfo],
        timeout,
    )
    .await
    .map_err(|err| {
        if transport == "ble" {
            format!(
                "{err} Device query timed out after identity. Retry Connect; close other Bluetooth clients first."
            )
        } else {
            err
        }
    })?;
    match event.payload {
        EventPayload::DeviceInfo(info) => Ok(info),
        _ => Err("Unexpected response to device query.".into()),
    }
}

fn looks_like_bt_address(value: &str) -> bool {
    let trimmed = value.trim();
    let parts: Vec<_> = trimmed.split([':', '-']).collect();
    parts.len() == 6
        && parts.iter().all(|part| {
            part.len() == 2 && part.chars().all(|ch| ch.is_ascii_hexdigit())
        })
}

/// Human-facing companion label for status / welcome rename.
///
/// Order: SelfInfo advert name → BLE scan GAP name → transport address.
fn companion_display_label(status: &Mutex<RadioStatus>, address: &str, self_name: &str) -> String {
    let self_name = self_name.trim();
    if !self_name.is_empty() && !looks_like_bt_address(self_name) {
        return self_name.to_string();
    }
    let scanned = {
        let guard = status.lock().unwrap_or_else(|err| err.into_inner());
        guard
            .devices
            .iter()
            .find(|device| device.address.eq_ignore_ascii_case(address))
            .map(|device| device.name.clone())
    };
    if let Some(name) = scanned {
        let name = name.trim();
        if !name.is_empty() && name != "Unnamed device" && !looks_like_bt_address(name) {
            return name.to_string();
        }
    }
    if !self_name.is_empty() {
        return self_name.to_string();
    }
    address.to_string()
}

/// Record this companion as the HQ node and site settings. Primary companion only.
fn apply_hq_identity(db: &Db, ident: &SelfIdentity) -> Result<(), String> {
    db.save_hq(&ident.pubkey, &ident.name, ident.adv_lat, ident.adv_lon, &ident.raw)
        .map_err(|err| err.to_string())?;
    contact_qr::enqueue_contact_qr(&ident.pubkey);
    if !ident.name.trim().is_empty() {
        let _ = db.set_setting("hq_name", ident.name.trim());
    }
    let (lat, lon) = crate::db::coords_from_micro(ident.adv_lat, ident.adv_lon);
    if let (Some(lat), Some(lon)) = (lat, lon) {
        let _ = db.set_setting("hq_lat", &format!("{lat:.6}"));
        let _ = db.set_setting("hq_lon", &format!("{lon:.6}"));
    }
    Ok(())
}

/// Channel-table work only the primary companion does: read its slots, push the
/// HQ channels, and wipe slots the operator soft-deleted. Errors that mean the
/// link is dead are returned; other failures are logged.
async fn primary_channel_setup(
    db: &Db,
    secrets: &Secrets,
    commands: &meshcore_rs::commands::CommandHandler,
    max_channels: u8,
    status: &Mutex<RadioStatus>,
) -> Result<(), String> {
    write_sync(
        status,
        SyncPhase::Channels,
        "Syncing channels…",
        Some((0, max_channels as u32)),
    );
    // Channel sync failure (including first-slot timeout on a dead link)
    // must not fall through to "Companion ready".
    if let Err(err) = sync_channels(db, secrets, commands, max_channels, status).await {
        hq_log!(warn, "channel sync: {err}");
        return Err(err);
    }
    if let Err(err) = push_channel(db, secrets, commands, max_channels).await {
        hq_log!(warn, "channel setup: {err}");
        // After a long BLE contact dump, meshcore-rs often returns a plain
        // timeout while logging "BLE TX error: Not connected" on the side.
        // Do not fall through to Companion ready with a dead GATT link.
        if is_link_error(&err) || is_channel_query_timeout(&err) {
            return Err(format!(
                "Channel push failed — radio link likely dropped: {err}"
            ));
        }
    }
    // Soft-delete is local; companion still holds the slot until we blank it.
    if let Err(err) = clear_soft_deleted_radio_slots(db, commands).await {
        hq_log!(warn, "clear soft-deleted channels: {err}");
        if is_link_error(&err) || is_channel_query_timeout(&err) {
            return Err(format!(
                "Channel cleanup failed — radio link likely dropped: {err}"
            ));
        }
    }
    Ok(())
}

async fn run_session(
    ctx: &SlotCtx,
    rx: &mut UnboundedReceiver<RadioCmd>,
    live: &Mutex<Option<Arc<MeshCore>>>,
) -> SessionEnd {
    let db = &ctx.pool.db;
    let secrets: &Secrets = &ctx.pool.secrets;
    let status = &ctx.status;
    let open_path = if ctx.transport == "usb" {
        match resolve_and_store_usb_path(ctx).await {
            Ok(path) => path,
            Err(err) => {
                let detail = if err.to_ascii_lowercase().contains("not found") {
                    "USB companion not found; waiting for the same device…".to_string()
                } else {
                    err
                };
                return SessionEnd::Lost(detail);
            }
        }
    } else {
        ctx.address
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone()
    };
    let name = open_path.as_str();
    let radio = ctx.id;
    let scan_cancel = &ctx.scan_cancel;
    let meshcore = match connect_transport(&ctx.transport, name).await {
        Ok(meshcore) => Arc::new(meshcore),
        Err(err) => return SessionEnd::Lost(err),
    };
    // Shared so an operator Disconnect can tear the link down even while this
    // session is stuck mid-sync or waiting on the command lock.
    *live.lock().unwrap_or_else(|err| err.into_inner()) = Some(meshcore.clone());

    // Windows BLE: notifications often lag 0.5–2s after subscribe; give the
    // stack time before APPSTART or the reply never arrives.
    if ctx.transport == "ble" {
        let ms = if cfg!(target_os = "windows") { 3500 } else { 500 };
        write_sync(
            status,
            SyncPhase::AppStart,
            "Bluetooth linked — settling…",
            None,
        );
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }

    let identity: Option<SelfIdentity>;
    let channel_setup_done = Arc::new(AtomicBool::new(false));
    let channel_setup_inflight = Arc::new(AtomicBool::new(false));
    let max_channels;
    {
        let dispatcher = meshcore.dispatcher();
        let commands = meshcore.commands().lock().await;
        write_sync(status, SyncPhase::AppStart, "Reading companion identity…", None);
        let info = match companion_appstart(&commands, dispatcher, &ctx.transport).await {
            Ok(info) => info,
            Err(err) => return SessionEnd::Lost(err),
        };
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
        // Prefer the MeshCore advert name from SelfInfo. Fall back to the BLE
        // GAP name from the last scan before using the transport address — UI
        // surfaces (`connected_name`, welcome rename) should not show a MAC.
        let label = companion_display_label(status, name, info.name.trim());
        let _ = db.remember_radio(&ctx.transport, name, &label);
        let (lat, lon) = crate::db::coords_from_micro(info.adv_lat, info.adv_lon);
        {
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
            guard.public_key = pubkey.clone();
        }
        let ident = SelfIdentity {
            pubkey,
            name: info.name.clone(),
            adv_lat: info.adv_lat,
            adv_lon: info.adv_lon,
            raw,
        };
        // The first connected companion becomes primary when none is set.
        ctx.set_link(true);
        if ctx.is_primary() {
            if let Err(err) = apply_hq_identity(db, &ident) {
                return SessionEnd::Lost(err);
            }
        }
        identity = Some(ident);
        write_sync(status, SyncPhase::AppStart, "Querying device info…", None);
        max_channels = match companion_device_query(&commands, dispatcher, &ctx.transport).await {
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
                guard.protocol_ver = info.fw_version_code;
                guard.model = model;
                if let Some(repeat) = info.repeat {
                    guard.client_repeat = repeat;
                }
                max_channels
            }
            Err(err) => return SessionEnd::Lost(err),
        };
        let _ = commands.set_time(unix_now() as u32).await;
        {
            let mut guard = status.lock().unwrap_or_else(|err| err.into_inner());
            guard.connected_name = label.clone();
        }
        if let Ok(battery) = commands.get_bat().await {
            apply_battery(db, status, &battery);
        }
        // Stay on "connecting" until sync finishes. Marking connected here used
        // to enable Send while this lock was still held for minutes — and a
        // nested lock below deadlocked the session before the live loop.
        write_status(
            status,
            "connecting",
            format!("Linked to {label}. Syncing companion…"),
        );
        write_sync(
            status,
            SyncPhase::Contacts,
            "Fetching contacts (BLE can take a few minutes)…",
            None,
        );
        // Contact lists over BLE can outlast the default 5s command timeout.
        // Slow/empty dumps can retry from the live loop; a dead link must not.
        if let Err(err) =
            sync_contacts(db, &commands, meshcore.dispatcher(), Some(status), radio).await
        {
            hq_log!(warn, "contact sync: {err}");
            if is_hard_gatt_drop(&err) || is_link_error(&err) {
                return SessionEnd::Lost(err);
            }
        }
        // After a long BLE contact dump the GATT link is often soft-dead for a
        // moment; give it air before channel queries (Windows needs longer).
        if ctx.transport == "ble" {
            let ms = if cfg!(target_os = "windows") { 2000 } else { 600 };
            write_sync(
                status,
                SyncPhase::Channels,
                "Contacts synced — settling before channels…",
                None,
            );
            tokio::time::sleep(Duration::from_millis(ms)).await;
        }
        // Slot numbers are per radio. Only the primary reads its channel table into
        // the database and receives the HQ channels; a secondary waits for promotion.
        // Soft channel-setup failures must NOT drop the session — that reconnect
        // loop (especially on Windows BLE after a big contact sync) never recovers.
        if ctx.is_primary() {
            match primary_channel_setup(db, secrets, &commands, max_channels, status).await {
                Ok(()) => channel_setup_done.store(true, Ordering::SeqCst),
                Err(err) => {
                    hq_log!(warn, "channel setup deferred: {err}");
                    if is_hard_gatt_drop(&err) {
                        return SessionEnd::Lost(err);
                    }
                    write_sync(
                        status,
                        SyncPhase::Channels,
                        format!("Channel sync deferred — staying linked. ({err})"),
                        None,
                    );
                    channel_setup_done.store(false, Ordering::SeqCst);
                }
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
            // Listen-before-write (same as live drain) — USB CDC can reply before
            // meshcore-rs get_msg attaches its waiter.
            match get_msg_listening_first(
                &commands,
                meshcore.dispatcher(),
                Duration::from_secs(4),
            )
            .await
            {
                Ok(Some(event)) => {
                    drained = drained.saturating_add(1);
                    write_sync(
                        status,
                        SyncPhase::Messages,
                        format!("Draining queued messages… ({drained})"),
                        Some((drained, (drained + 8).max(1))),
                    );
                    // History/drain only — do not re-run METIS-INFO bot commands.
                    if let Err(err) =
                        handle_event(db, &meshcore, event, radio, false, Some(status)).await
                    {
                        hq_log!(warn, "message: {err}");
                    }
                }
                Ok(None) => break,
                Err(err) => {
                    if is_link_error(&err) {
                        return SessionEnd::Lost(err);
                    }
                    // No reply / soft error — treat as empty and move on to live mode.
                    hq_log!(debug, "connect msg drain: {err}");
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
        // Use the lock we already hold — a second commands().lock() here
        // deadlocked the session forever (tokio Mutex is not reentrant).
        if let Err(err) = refresh_custom_vars(&commands, status).await {
            hq_log!(debug, "custom vars: {err}");
            if is_link_error(&err) {
                return SessionEnd::Lost(err);
            }
        }
        write_status(status, "connected", connected_detail(status, name));
        // Drop the held commands lock before background status DMs compete for it.
        drop(commands);
        {
            let meshcore = meshcore.clone();
            let db = Arc::clone(db);
            let status = Arc::clone(status);
            tokio::spawn(async move {
                // Let the session loop subscribe / settle before airtime probes.
                tokio::time::sleep(Duration::from_secs(3)).await;
                run_claimed_infra_status_sweep(db, meshcore, status).await;
            });
        }
        // Re-acquire for the rest of session setup that expects `commands` in scope…
        // (session continues with a fresh lock below via the event loop)
    }

    let (event_tx, mut event_rx) = unbounded_channel();
    let mut subscriptions = Vec::new();
    let kinds = [
        EventType::ContactMsgRecv,
        EventType::ChannelMsgRecv,
        EventType::Advertisement,
        EventType::PathUpdate,
        EventType::Ack,
        EventType::Disconnected,
        EventType::MessagesWaiting,
        EventType::LoginSuccess,
        EventType::LoginFailed,
    ];
    for kind in kinds {
        let event_tx = event_tx.clone();
        let subscription = meshcore
            .subscribe(kind, HashMap::new(), move |event| {
                let _ = event_tx.send(event);
            })
            .await;
        subscriptions.push(subscription);
    }
    let event_tx_late = event_tx.clone();
    drop(event_tx);
    // LogData: every overheard frame. Only while map upload and/or CLI RX log wants it —
    // otherwise BLE floods with noise. Held separately so we can unsubscribe mid-session.
    let mut logdata_sub: Option<Subscription> = None;
    {
        let need_logdata = crate::services::map_upload::upload_enabled(db)
            || ctx.pool.rx_log_wanted.load(Ordering::SeqCst);
        if need_logdata {
            let event_tx = event_tx_late.clone();
            logdata_sub = Some(
                meshcore
                    .subscribe(EventType::LogData, HashMap::new(), move |event| {
                        let _ = event_tx.send(event);
                    })
                    .await,
            );
            hq_log!(info, "LogData subscribed (map upload / RX log)");
        }
    }
    // Do NOT use meshcore-rs start_auto_message_fetching(): it spawns tasks that
    // hold the command mutex across get_msg and starve channel sends / CLI.
    // Live RX uses MessagesWaiting + tick drain with lock yield between batches (O5).

    let mut replies: HashMap<String, Instant> = HashMap::new();
    let mut pushed_generation = 0i64;
    let mut was_primary = ctx.is_primary();
    let mut last_battery = Instant::now();
    // Consecutive battery/channel TX failures while UI still says "connected".
    // meshcore-rs often logs "BLE TX error: Not connected" beside a plain timeout.
    let mut link_fail_streak: u8 = 0;
    // Periodic claimed-infra status sweep (Settings interval; 0 = connect-only).
    let mut last_infra_sweep = Instant::now();
    // Contacts were just synced during connect.
    let mut last_advert_sync = Instant::now();
    let mut contact_retry_at = Instant::now() + Duration::from_secs(15);
    // Older builds could finish a dump and still leave contacts_since at 0,
    // which re-triggers a multi-minute BLE sync on every connect and blocks sends.
    // Watermark is per-radio so a second companion does not inherit the primary's.
    if db.contacts_since_for_radio(radio).unwrap_or(0) == 0 {
        if db.list_nodes().map(|n| n.len()).unwrap_or(0) > 0 {
            let _ = db.set_contacts_since_for_radio(radio, 1);
        }
    }
    let mut need_contact_retry = db.contacts_since_for_radio(radio).unwrap_or(0) == 0;
    // Soft-deferred channel setup (BLE timeout after contact dump) retries here.
    let mut channel_setup_retry_at = if channel_setup_done.load(Ordering::SeqCst) {
        Instant::now() + Duration::from_secs(3600)
    } else {
        Instant::now() + Duration::from_secs(8)
    };
    // Persistent interval — recreating `sleep(1s)` inside select! starves battery
    // polls whenever mesh events keep arriving (auto-fetch / advertisements).
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Live message queue: drain on MessagesWaiting, retry if lock-busy, and poll
    // periodically (MeshCore docs: do not rely on 0x83 alone — it coalesces).
    let mut need_msg_drain = false;
    let mut last_msg_poll = Instant::now();
    const MSG_POLL_EVERY: Duration = Duration::from_secs(2);
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
                    Some(cmd) if cmd.needs_primary() && !ctx.is_primary() => {
                        reject_cmd(cmd, status, NOT_PRIMARY_REASON);
                    }
                    Some(RadioCmd::ApplyName(advert)) => {
                        let advert = advert.trim().to_string();
                        if !advert.is_empty() {
                            let commands = meshcore.commands().lock().await;
                            if let Err(err) = commands.set_name(&advert).await {
                                write_status(status, "connected", err.to_string());
                            } else {
                                let _ = db.set_setting("hq_name", &advert);
                                let hq_key = {
                                    let mut guard =
                                        status.lock().unwrap_or_else(|err| err.into_inner());
                                    guard.connected_name = advert.clone();
                                    guard.public_key.clone()
                                };
                                if !hq_key.is_empty() {
                                    let _ = db.update_node_name(&hq_key, &advert);
                                    contact_qr::enqueue_contact_qr(&hq_key);
                                }
                                let addr = ctx
                                    .address
                                    .lock()
                                    .unwrap_or_else(|err| err.into_inner())
                                    .clone();
                                let _ = db.remember_radio(&ctx.transport, &addr, &advert);
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
                    Some(RadioCmd::ClearChannel { slot, reply }) => {
                        let commands = meshcore.commands().lock().await;
                        let result = clear_radio_channel_slot(&commands, slot).await;
                        match &result {
                            Ok(()) => write_status(
                                status,
                                "connected",
                                format!("Cleared radio channel slot {slot}."),
                            ),
                            Err(err) => write_status(status, "connected", err.clone()),
                        }
                        if let Some(reply) = reply {
                            let _ = reply.send(result);
                        }
                    }
                    Some(RadioCmd::SendChannel {
                        slot,
                        text,
                        bot_activity,
                        reply,
                    }) => {
                        hq_log!(debug, "channel send: handling slot {slot}");
                        let result = transmit_channel_message(
                            db,
                            &meshcore,
                            status,
                            name,
                            radio,
                            slot,
                            &text,
                            ctx.is_primary(),
                            bot_activity,
                        )
                        .await;
                        if let Err(err) = &result {
                            hq_log!(error, "channel send (slot {slot}): {err}");
                        } else {
                            hq_log!(info, "channel send: ok slot {slot}");
                        }
                        if let Some(reply) = reply {
                            let _ = reply.send(result);
                        }
                    }
                    Some(RadioCmd::RefreshDeviceSettings) => {
                        let commands = meshcore.commands().lock().await;
                        match refresh_custom_vars(&commands, status).await {
                            Ok(()) => write_status(
                                status,
                                "connected",
                                "Device settings refreshed from the companion.",
                            ),
                            Err(err) => {
                                hq_log!(warn, "custom vars refresh: {err}");
                                write_status(
                                    status,
                                    "connected",
                                    format!("Could not refresh device settings: {err}"),
                                );
                            }
                        }
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
                                let _ = db.set_setting("hq_lat", &format!("{lat:.6}"));
                                let _ = db.set_setting("hq_lon", &format!("{lon:.6}"));
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
                    Some(RadioCmd::SetDefaultFloodScope(scope)) => {
                        let commands = meshcore.commands().lock().await;
                        match apply_default_flood_scope(&commands, scope.as_deref()).await {
                            Ok(applied) => {
                                {
                                    let mut guard =
                                        status.lock().unwrap_or_else(|err| err.into_inner());
                                    guard.flood_scope = applied.clone();
                                }
                                let detail = match applied.as_deref() {
                                    Some(name) => format!("Default flood scope set to {name}."),
                                    None => "Default flood scope cleared.".into(),
                                };
                                write_status(status, "connected", detail);
                            }
                            Err(err) => write_status(status, "connected", err),
                        }
                    }
                    Some(RadioCmd::DiscoverRegions { reply }) => {
                        scan_cancel.store(false, Ordering::SeqCst);
                        let result =
                            run_region_discover(&db, &meshcore, status, scan_cancel).await;
                        let _ = reply.send(result);
                    }
                    Some(RadioCmd::DiscoverNodes { filter, reply }) => {
                        scan_cancel.store(false, Ordering::SeqCst);
                        let result = run_node_discover(
                            &db,
                            &meshcore,
                            status,
                            filter,
                            scan_cancel,
                            radio,
                        )
                        .await;
                        let _ = reply.send(result);
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
                    Some(RadioCmd::RunCli {
                        contact_hex,
                        text,
                        infra,
                    }) => {
                        let text = text.trim().to_string();
                        if text.is_empty() {
                            push_cli_line(status, "Empty command.");
                            continue;
                        }
                        let protocol_ver = status
                            .lock()
                            .unwrap_or_else(|err| err.into_inner())
                            .protocol_ver;
                        let txt_type = if infra { 1u8 } else { 3u8 };
                        let result = if contact_hex.trim().is_empty() {
                            apply_local_cli(&meshcore, &text, protocol_ver).await
                        } else {
                            match parse_pubkey_prefix(&contact_hex) {
                                Ok(prefix) => {
                                    match lock_commands_for_operator(&meshcore).await {
                                        Ok(commands) => {
                                            apply_remote_cli(&commands, prefix, &text, txt_type)
                                                .await
                                        }
                                        Err(err) => Err(err),
                                    }
                                }
                                Err(err) => Err(err),
                            }
                        };
                        match result {
                            Ok(reply) => {
                                // UI already echoes `> cmd`. Remote CLI returns empty here —
                                // the mesh reply is pushed when ContactMessage (txt_type 1) arrives.
                                let reply = reply.trim();
                                if reply.is_empty() {
                                    write_status(
                                        status,
                                        "connected",
                                        "CLI command sent — waiting for mesh reply.",
                                    );
                                } else {
                                    push_cli_line(status, reply);
                                    write_status(status, "connected", "CLI command completed.");
                                }
                            }
                            Err(err) => {
                                push_cli_line(status, format!("error: {err}"));
                                write_status(status, "connected", err);
                            }
                        }
                    }
                    Some(RadioCmd::SendLogin {
                        pubkey_hex,
                        password,
                        reply,
                    }) => {
                        // Never put the password in status/cli/audit.
                        write_status(status, "connected", "Verifying infrastructure login…");
                        let outcome = apply_infra_login(&meshcore, &pubkey_hex, &password).await;
                        match &outcome {
                            Ok(()) => {
                                push_cli_line(status, "login ok");
                                write_status(status, "connected", "Infrastructure login ok.");
                            }
                            Err(err) => {
                                push_cli_line(status, format!("login failed: {err}"));
                                write_status(status, "connected", err.clone());
                            }
                        }
                        let _ = reply.send(outcome);
                    }
                    Some(RadioCmd::ProbeNoise) => {
                        // Run off the session select arm. Holding commands across
                        // multi-minute status timeouts starved SendChannel (UI 30s
                        // timeout while the frame sat in the cmd queue).
                        let meshcore = meshcore.clone();
                        let db = Arc::clone(db);
                        let status = Arc::clone(status);
                        tokio::spawn(async move {
                            run_noise_probe(db, meshcore, status).await;
                        });
                    }
                    Some(RadioCmd::FetchAdvertPath { pubkey_hex, reply }) => {
                        let meshcore = meshcore.clone();
                        let db = Arc::clone(db);
                        let status = Arc::clone(status);
                        tokio::spawn(async move {
                            let outcome =
                                run_operator_path_fetch(db.as_ref(), &meshcore, &pubkey_hex).await;
                            match &outcome {
                                Ok(fetch) => {
                                    write_status(
                                        status.as_ref(),
                                        "connected",
                                        format!(
                                            "Path trace ok · {} hop(s) ({})",
                                            fetch.hop_count, fetch.source
                                        ),
                                    );
                                }
                                Err(err) => {
                                    write_status(status.as_ref(), "connected", err.clone());
                                }
                            }
                            let _ = reply.send(outcome);
                        });
                    }
                    Some(RadioCmd::RefreshContact { pubkey_hex, reply }) => {
                        let meshcore = meshcore.clone();
                        let db = Arc::clone(db);
                        let status = Arc::clone(status);
                        let radio_id = radio;
                        tokio::spawn(async move {
                            let outcome = run_contact_refresh(
                                db.as_ref(),
                                &meshcore,
                                status.as_ref(),
                                radio_id,
                                &pubkey_hex,
                            )
                            .await;
                            match &outcome {
                                Ok(result) => {
                                    write_status(
                                        status.as_ref(),
                                        "connected",
                                        result.summary.clone(),
                                    );
                                }
                                Err(err) => {
                                    write_status(status.as_ref(), "connected", err.clone());
                                }
                            }
                            let _ = reply.send(outcome);
                        });
                    }
                    Some(RadioCmd::ProbeInfraStatus { pubkey_hex, reply }) => {
                        let meshcore = meshcore.clone();
                        let status = Arc::clone(status);
                        let db = Arc::clone(db);
                        tokio::spawn(async move {
                            let outcome = run_infra_status_probe(
                                meshcore.clone(),
                                status,
                                &pubkey_hex,
                            )
                            .await;
                            if outcome.is_ok() {
                                tokio::task::yield_now().await;
                                probe_and_persist_neighbours(&db, &meshcore, &pubkey_hex).await;
                            }
                            let _ = reply.send(outcome);
                        });
                    }
                    Some(RadioCmd::PingNode { pubkey_hex, reply }) => {
                        let meshcore = meshcore.clone();
                        let status = Arc::clone(status);
                        let db = Arc::clone(db);
                        tokio::spawn(async move {
                            let outcome =
                                run_node_ping(db.as_ref(), meshcore, status, &pubkey_hex)
                                    .await;
                            let _ = reply.send(outcome);
                        });
                    }
                    Some(RadioCmd::SignBytes { data, reply }) => {
                        let commands = meshcore.commands().lock().await;
                        let outcome = match commands.sign(&data, 128).await {
                            Ok(sig) => Ok(sig),
                            Err(err) => Err(err.to_string()),
                        };
                        let _ = reply.send(outcome);
                    }
                    Some(RadioCmd::ExportPrivateKey { reply }) => {
                        let commands = meshcore.commands().lock().await;
                        let outcome = match commands.export_private_key().await {
                            Ok(key) => Ok(key),
                            Err(err) => Err(err.to_string()),
                        };
                        let _ = reply.send(outcome);
                    }
                    Some(RadioCmd::Reboot) => {
                        // Prefer the out-of-band reboot path (stop + reboot_pending).
                        // If we get here instead, still send the frame and leave.
                        write_status(status, "reconnecting", "Rebooting companion…");
                        match send_companion_reboot(&meshcore).await {
                            Ok(()) => {
                                let _ = meshcore.disconnect().await;
                                return SessionEnd::Lost("Companion rebooting.".into());
                            }
                            Err(err) => write_status(status, "connected", err),
                        }
                    }
                }
            }
            event = event_rx.recv() => {
                match event {
                    None => return SessionEnd::Lost("Radio event stream ended.".into()),
                    Some(event) if event.event_type == EventType::Disconnected => {
                        let link = if ctx.transport == "usb" { "USB" } else { "Bluetooth" };
                        return SessionEnd::Lost(format!("{link} link closed."));
                    }
                    Some(event) if event.event_type == EventType::MessagesWaiting => {
                        // Coalescing push — must drain (or keep retrying) until
                        // NoMoreMessages, or further arrivals may not re-signal.
                        hq_log!(info, "radio {radio}: MessagesWaiting");
                        let result = drain_pending_messages(&meshcore, radio, true).await;
                        need_msg_drain = need_msg_drain_after(result);
                        match result {
                            MsgDrainResult::Busy => set_receiving_busy_hint(status, true),
                            MsgDrainResult::Cleared => set_receiving_busy_hint(status, false),
                            MsgDrainResult::MorePending => {}
                        }
                    }
                    Some(event) => {
                        if event.event_type == EventType::Advertisement
                            && last_advert_sync.elapsed() >= Duration::from_secs(300)
                        {
                            last_advert_sync = Instant::now();
                            need_contact_retry = true;
                            contact_retry_at = Instant::now();
                        }
                        // Map uploader + optional CLI RX log from LogData.
                        if let EventPayload::LogData(ref log) = event.payload {
                            if ctx.is_primary() {
                                push_rx_log_line(&ctx.pool, log);
                                let guard = status.lock().unwrap_or_else(|err| err.into_inner());
                                let params = crate::services::map_upload::radio_params_from_mhz(
                                    guard.radio_freq_mhz,
                                    guard.radio_bw_khz,
                                    guard.radio_sf,
                                    guard.radio_cr,
                                );
                                drop(guard);
                                crate::services::map_upload::try_enqueue(db, log, params);
                            }
                        }
                        // Infra CLI_DATA replies → CLI transcript (not chat enrollment).
                        if let EventPayload::ContactMessage(ref msg) = event.payload {
                            if msg.txt_type == 1 && !msg.text.trim().is_empty() {
                                push_cli_line(status, msg.text.trim());
                                write_status(status, "connected", "CLI command completed.");
                            }
                        }
                        // Bot replies only on the primary; every radio still ingests history.
                        let live_bot = ctx.is_primary();
                        if let Err(err) =
                            handle_event(db, &meshcore, event, radio, live_bot, Some(status)).await
                        {
                            hq_log!(warn, "event: {err}");
                        }
                    }
                }
            }
            _ = tick.tick() => {
                let _ = db.fail_stale_tx_messages(90);
                // Tick order (do not reorder): (1) message drain, (2) LogData
                // subscribe, (3) outbound/broadcast, (4) channel push, (5) battery,
                // (6) infra/contacts. Drain must win the mutex over background.
                //
                // Cap+yield: each drain_pending_messages takes/releases the lock;
                // MorePending retries next tick. Do NOT use start_auto_message_fetching().
                let poll_due = last_msg_poll.elapsed() >= MSG_POLL_EVERY;
                if need_msg_drain || poll_due {
                    let from_waiting = need_msg_drain;
                    match drain_pending_messages(&meshcore, radio, from_waiting).await {
                        MsgDrainResult::Cleared => {
                            need_msg_drain = false;
                            last_msg_poll = Instant::now();
                            set_receiving_busy_hint(status, false);
                        }
                        MsgDrainResult::MorePending => {
                            need_msg_drain = true;
                            last_msg_poll = Instant::now();
                            // Still draining across ticks — soft status, not an alarm.
                            set_receiving_busy_hint(status, true);
                        }
                        MsgDrainResult::Busy => {
                            // Keep need_msg_drain so the next tick retries; for a
                            // pure poll, back off until MSG_POLL_EVERY again.
                            if from_waiting {
                                set_receiving_busy_hint(status, true);
                            }
                            if !need_msg_drain {
                                last_msg_poll = Instant::now();
                            }
                        }
                    }
                }
                // Mid-session LogData: subscribe while map upload or CLI RX log wants it;
                // unsubscribe when neither needs the flood of every overheard frame.
                let need_logdata = crate::services::map_upload::upload_enabled(db)
                    || ctx.pool.rx_log_wanted.load(Ordering::SeqCst);
                if need_logdata && logdata_sub.is_none() {
                    let event_tx = event_tx_late.clone();
                    logdata_sub = Some(
                        meshcore
                            .subscribe(EventType::LogData, HashMap::new(), move |event| {
                                let _ = event_tx.send(event);
                            })
                            .await,
                    );
                    hq_log!(info, "LogData subscribed (map upload / RX log)");
                } else if !need_logdata {
                    if let Some(sub) = logdata_sub.take() {
                        sub.unsubscribe().await;
                        hq_log!(info, "LogData unsubscribed");
                    }
                }
                let primary = ctx.is_primary();
                if primary {
                    if !was_primary {
                        hq_log!(info, "radio {radio} took over as primary companion");
                        if let Some(ident) = identity.as_ref() {
                            if let Err(err) = apply_hq_identity(db, ident) {
                                hq_log!(warn, "HQ identity: {err}");
                            }
                        }
                        // Force a channel setup attempt after promotion.
                        channel_setup_done.store(false, Ordering::SeqCst);
                        channel_setup_retry_at = Instant::now();
                    }
                    if !channel_setup_done.load(Ordering::SeqCst)
                        && !channel_setup_inflight.load(Ordering::SeqCst)
                        && Instant::now() >= channel_setup_retry_at
                    {
                        channel_setup_retry_at = Instant::now() + Duration::from_secs(45);
                        channel_setup_inflight.store(true, Ordering::SeqCst);
                        pushed_generation = db.channel_generation().unwrap_or(0);
                        let db = Arc::clone(db);
                        let secrets = Arc::clone(&ctx.pool.secrets);
                        let meshcore = meshcore.clone();
                        let status = Arc::clone(status);
                        let done = Arc::clone(&channel_setup_done);
                        let inflight = Arc::clone(&channel_setup_inflight);
                        tokio::spawn(async move {
                            let outcome = async {
                                let Ok(commands) = tokio::time::timeout(
                                    SEND_LOCK_WAIT,
                                    meshcore.commands().lock(),
                                )
                                .await
                                else {
                                    return Err("radio busy".into());
                                };
                                primary_channel_setup(
                                    &db, &secrets, &commands, max_channels, &status,
                                )
                                .await
                            }
                            .await;
                            match outcome {
                                Ok(()) => {
                                    done.store(true, Ordering::SeqCst);
                                    write_sync(&status, SyncPhase::Ready, "Companion ready", None);
                                }
                                Err(err) => {
                                    hq_log!(warn, "deferred channel setup: {err}");
                                    write_sync(
                                        &status,
                                        SyncPhase::Ready,
                                        "Companion ready (channel sync still pending)",
                                        None,
                                    );
                                }
                            }
                            inflight.store(false, Ordering::SeqCst);
                        });
                    }
                    if let Err(err) = flush_one_outbound(db, &meshcore, radio, &mut replies).await {
                        hq_log!(warn, "send: {err}");
                    }
                    if primary {
                        if let Err(err) =
                            flush_one_broadcast_leg(db, &meshcore, status, name, radio).await
                        {
                            hq_log!(warn, "broadcast: {err}");
                        }
                    }
                }
                was_primary = primary;
                let generation = db.channel_generation().unwrap_or(0);
                if primary && generation != pushed_generation {
                    if let Some(commands) = lock_commands_brief(&meshcore).await {
                        match push_channel(db, secrets, &commands, max_channels).await {
                            Ok(true) => {
                                pushed_generation = generation;
                                link_fail_streak = 0;
                            }
                            Ok(false) => {}
                            Err(err) => {
                                hq_log!(warn, "channel: {err}");
                                if is_link_error(&err) || is_channel_query_timeout(&err) {
                                    link_fail_streak = link_fail_streak.saturating_add(1);
                                }
                            }
                        }
                    }
                }
                // O4: when RX drain still needs the mutex, skip one background
                // brief-lock job this tick so drain retries win.
                if !need_msg_drain && last_battery.elapsed() >= Duration::from_secs(15) {
                    if let Some(commands) = lock_commands_brief(&meshcore).await {
                        match commands.get_bat().await {
                            Ok(battery) => {
                                apply_battery(db, status, &battery);
                                link_fail_streak = 0;
                            }
                            Err(err) => {
                                let msg = err.to_string();
                                hq_log!(debug, "battery poll: {msg}");
                                if is_link_error(&msg) || is_channel_query_timeout(&msg) {
                                    link_fail_streak = link_fail_streak.saturating_add(1);
                                }
                            }
                        }
                        last_battery = Instant::now();
                    }
                }
                // GATT died under us: stop pretending Connected and reconnect.
                if link_fail_streak >= 3 {
                    return SessionEnd::Lost(
                        "Companion link dropped (radio not responding). Reconnecting…".into(),
                    );
                }
                // Periodic claimed repeater/room/sensor status (primary only).
                if ctx.is_primary() {
                    let interval_mins = db.infra_probe_interval_mins();
                    if interval_mins > 0 {
                        let every = Duration::from_secs((interval_mins as u64).saturating_mul(60));
                        if last_infra_sweep.elapsed() >= every {
                            last_infra_sweep = Instant::now();
                            let meshcore = meshcore.clone();
                            let db = Arc::clone(&ctx.pool.db);
                            let status = Arc::clone(status);
                            tokio::spawn(async move {
                                run_claimed_infra_status_sweep(db, meshcore, status).await;
                            });
                        }
                    }
                }
                // Never run a multi-minute contact dump on the live loop — that
                // held the select arm and made SendChannel look dead.
                // Incremental get_contacts can still need ~15–25s on a busy radio;
                // an 8s budget timed out every attempt and left need_contact_retry
                // stuck true (retry spam every 2 minutes). O5: channel setup is
                // spawned; contact retry stays on the tick arm but is capped at
                // 25s and skipped while need_msg_drain (O4).
                if !need_msg_drain
                    && need_contact_retry
                    && Instant::now() >= contact_retry_at
                {
                    contact_retry_at = Instant::now() + Duration::from_secs(120);
                    if let Some(commands) = lock_commands_brief(&meshcore).await {
                        write_sync(
                            status,
                            SyncPhase::Contacts,
                            "Retrying contact sync…",
                            None,
                        );
                        let sync = tokio::time::timeout(
                            Duration::from_secs(25),
                            sync_contacts(
                                db,
                                &commands,
                                meshcore.dispatcher(),
                                Some(status),
                                radio,
                            ),
                        )
                        .await;
                        match sync {
                            Ok(Ok(_n)) => {
                                need_contact_retry = false;
                                last_advert_sync = Instant::now();
                                write_sync(status, SyncPhase::Ready, "Companion ready", None);
                                write_status(status, "connected", connected_detail(status, name));
                            }
                            Ok(Err(err)) => {
                                // Don't spin forever on a hard error — wait for a
                                // fresh advert (5 min throttle) to re-arm.
                                need_contact_retry = false;
                                hq_log!(warn, "contact sync retry: {err}");
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
                            Err(_) => {
                                // Give up this pass; next advert can re-arm.
                                need_contact_retry = false;
                                hq_log!(
                                    debug,
                                    "contact sync retry: timed out (will retry after next advert)"
                                );
                                write_sync(status, SyncPhase::Ready, "Companion ready", None);
                                write_status(status, "connected", connected_detail(status, name));
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
    dispatcher: &meshcore_rs::events::EventDispatcher,
    status: Option<&Mutex<RadioStatus>>,
    radio_id: i64,
) -> Result<u32, String> {
    let since: u32 = db
        .contacts_since_for_radio(radio_id)
        .map_err(|err| err.to_string())?;
    // Full dumps of hundreds of contacts over BLE can take a minute or two;
    // incremental replies are small and arrive almost immediately.
    let timeout = if since == 0 {
        Duration::from_secs(120)
    } else {
        Duration::from_secs(30)
    };
    // CMD_GET_CONTACTS = 4, followed by the lastmod watermark.
    let mut data = vec![4u8];
    data.extend_from_slice(&since.to_le_bytes());
    let event = request_listening_first(
        commands,
        dispatcher,
        &data,
        &[EventType::Contacts, EventType::Error],
        timeout,
    )
    .await?;
    let contacts = match event.payload {
        EventPayload::Contacts(contacts) => contacts,
        EventPayload::String(err) => return Err(err),
        _ if event.event_type == EventType::Error => {
            return Err("Companion rejected the contact request.".into())
        }
        _ => return Err("Unexpected reply to the contact request.".into()),
    };
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
            true, // bootstrap contact sync → stay noticed
            Some(radio_id),
        )
        .map_err(|err| err.to_string())?;
        contact_qr::enqueue_contact_qr(&pubkey);
        if let Some(status) = status {
            write_sync(
                status,
                SyncPhase::Contacts,
                "Syncing contacts…",
                Some((idx as u32 + 1, total.max(1))),
            );
        }
    }
    // Channel bubbles only carry a display name — attach roster keys once names uniquely match.
    if let Err(err) = db.backfill_channel_sender_pubkeys() {
        hq_log!(debug, "channel sender pubkey backfill: {err}");
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
    // A finished sync that leaves the watermark at 0 (empty dump, or every
    // contact still at timestamp 0) would retry the full BLE dump forever and
    // hold the command lock for minutes — channel sends sit in the queue and
    // look dead in the UI. Advance at least to 1 so the next pass is incremental.
    let marker = newest.max(since).max(1);
    db.set_contacts_since_for_radio(radio_id, marker)
        .map_err(|err| err.to_string())?;
    Ok(total)
}

fn is_link_error(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    lower.contains("not connected")
        || lower.contains("disconnected")
        || lower.contains("ble tx")
        || lower.contains("link closed")
        || lower.contains("unresponsive")
        || lower.contains("link lost")
        // USB serial drops (unplugged cable, reset companion).
        || lower.contains("broken pipe")
        || lower.contains("input/output error")
        || lower.contains("no such device")
        || lower.contains("port closed")
}

fn is_channel_query_timeout(err: &str) -> bool {
    err.to_ascii_lowercase().contains("timeout")
}

/// Hard GATT/serial drop — session must end. Soft timeouts (esp. after a long
/// BLE contact dump) should not reconnect-loop the whole companion.
fn is_hard_gatt_drop(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    (lower.contains("not connected")
        || lower.contains("ble tx")
        || lower.contains("disconnected")
        || lower.contains("link closed")
        || lower.contains("link lost")
        || lower.contains("broken pipe")
        || lower.contains("port closed"))
        && !lower.contains("unresponsive during channel")
}

/// Blank a companion channel slot (`set_channel` with empty name + zero secret).
async fn clear_radio_channel_slot(
    commands: &meshcore_rs::commands::CommandHandler,
    slot: u8,
) -> Result<(), String> {
    let blank = [0u8; 16];
    commands
        .set_channel(slot, "", &blank)
        .await
        .map_err(|err| err.to_string())
}

/// Wipe companion slots that still hold channels the operator soft-deleted in HQ,
/// unless another active HQ channel now owns that slot.
async fn clear_soft_deleted_radio_slots(
    db: &Db,
    commands: &meshcore_rs::commands::CommandHandler,
) -> Result<(), String> {
    let Ok(deleted) = db.list_soft_deleted_channels() else {
        return Ok(());
    };
    for (slot, name) in deleted {
        if slot < 0 || slot > u8::MAX as i64 {
            continue;
        }
        if matches!(db.active_channel_on_slot(slot), Ok(Some(_))) {
            continue;
        }
        let idx = slot as u8;
        match commands.get_channel(idx).await {
            Ok(info) if info.name.trim().eq_ignore_ascii_case(&name) => {
                if let Err(err) = clear_radio_channel_slot(commands, idx).await {
                    let msg = err.to_string();
                    hq_log!(warn, "clear soft-deleted `{name}` slot {idx}: {msg}");
                    if is_link_error(&msg) {
                        return Err(msg);
                    }
                }
            }
            Ok(_) => {}
            Err(err) => {
                let msg = err.to_string();
                hq_log!(debug, "get_channel({idx}) while clearing soft-deleted: {msg}");
                if is_link_error(&msg) || is_channel_query_timeout(&msg) {
                    return Err(msg);
                }
            }
        }
    }
    Ok(())
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
                    match db.upsert_channel_from_radio(name, idx as i64, &info.secret, secrets) {
                        Ok(false) => {
                            // Operator deleted this channel in HQ — wipe the companion
                            // slot so the next open does not resurrect it.
                            if let Err(err) = clear_radio_channel_slot(commands, idx).await {
                                hq_log!(warn, "clear soft-deleted channel slot {idx}: {err}");
                            }
                        }
                        Ok(true) => {}
                        Err(err) => hq_log!(warn, "upsert channel slot {idx}: {err}"),
                    }
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
                hq_log!(debug, "get_channel({idx}): {msg}");
                if is_link_error(&msg) {
                    return Err(format!("Channel sync stopped at slot {idx}: {msg}"));
                }
                // meshcore-rs often surfaces a dead GATT link as a plain timeout
                // ("Timeout waiting for Some(ChannelInfo)") while logging
                // "BLE TX error: Not connected" separately. A timeout before any
                // channel is read means the link is dead — do not pretend sync OK.
                // Mid-table timeouts are ambiguous (empty higher slots vs dead
                // link); keep partial results and let push_channel confirm the
                // link — a push timeout aborts setup instead of false Ready.
                if is_channel_query_timeout(&msg) {
                    if completed == 0 {
                        // One settle+retry — common after a multi-minute BLE contact dump.
                        hq_log!(
                            warn,
                            "get_channel({idx}) timed out with no slots yet; settling and retrying once"
                        );
                        tokio::time::sleep(Duration::from_millis(
                            if cfg!(target_os = "windows") {
                                1500
                            } else {
                                700
                            },
                        ))
                        .await;
                        match commands.get_channel(idx).await {
                            Ok(info) => {
                                completed = step as u32 + 1;
                                let name = info.name.trim();
                                if !name.is_empty() {
                                    match db.upsert_channel_from_radio(
                                        name,
                                        idx as i64,
                                        &info.secret,
                                        secrets,
                                    ) {
                                        Ok(false) => {
                                            if let Err(err) =
                                                clear_radio_channel_slot(commands, idx).await
                                            {
                                                hq_log!(
                                                    warn,
                                                    "clear soft-deleted channel slot {idx}: {err}"
                                                );
                                            }
                                        }
                                        Ok(true) => {}
                                        Err(err) => {
                                            hq_log!(warn, "upsert channel slot {idx}: {err}")
                                        }
                                    }
                                }
                                write_sync(
                                    status,
                                    SyncPhase::Channels,
                                    "Syncing channels…",
                                    Some((completed, total.max(1))),
                                );
                                continue;
                            }
                            Err(retry_err) => {
                                let retry_msg = retry_err.to_string();
                                hq_log!(debug, "get_channel({idx}) retry: {retry_msg}");
                                return Err(format!(
                                    "Radio link unresponsive during channel sync (slot {idx}): {msg}"
                                ));
                            }
                        }
                    }
                    hq_log!(
                        warn,
                        "get_channel({idx}) timed out after {completed} slot(s); keeping partial sync and verifying link on channel push"
                    );
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
    let info_name = db
        .info_channel_name()
        .map_err(|err| err.to_string())?;
    let net_name = db.net_channel_name().map_err(|err| err.to_string())?;
    let info_secret = db
        .ensure_channel_secret(secrets)
        .map_err(|err| err.to_string())?;
    let net_secret = db
        .ensure_net_channel_secret(secrets)
        .map_err(|err| err.to_string())?;

    let rows = db.list_channels().map_err(|err| err.to_string())?;
    let info_slot = rows
        .iter()
        .find(|row| row.name.eq_ignore_ascii_case(&info_name))
        .map(|row| row.slot as u8)
        .unwrap_or(if max_channels > 1 { 1 } else { 0 });
    let net_slot = rows
        .iter()
        .find(|row| row.name.eq_ignore_ascii_case(&net_name))
        .map(|row| row.slot as u8)
        .unwrap_or(if max_channels > 2 { 2 } else { info_slot.saturating_add(1) });
    let info_slot = info_slot.min(max_channels.saturating_sub(1));
    let mut net_slot = net_slot.min(max_channels.saturating_sub(1));
    if net_slot == info_slot && max_channels > 2 {
        net_slot = if info_slot == 1 { 2 } else { 1 };
    }

    // Prefer existing HQ slots. Never stomp another channel's slot
    // (that used to leave two DB rows on one slot and break private sends).
    commands
        .set_channel(info_slot, &info_name, &info_secret)
        .await
        .map_err(|err| err.to_string())?;
    let _ = db.ensure_channel_row(&info_name, info_slot as i64, "private", "info");
    db.set_setting("channel_slot", &info_slot.to_string())
        .map_err(|err| err.to_string())?;

    if max_channels > 1 && net_slot != info_slot {
        commands
            .set_channel(net_slot, &net_name, &net_secret)
            .await
            .map_err(|err| err.to_string())?;
        let _ = db.ensure_channel_row(&net_name, net_slot as i64, "private", "net");
    }
    Ok(true)
}

async fn push_named_channel(
    db: &Db,
    secrets: &Secrets,
    commands: &meshcore_rs::commands::CommandHandler,
    slot: u8,
    name: &str,
) -> Result<(), String> {
    let secret = if db.is_info_channel(name) {
        db.ensure_channel_secret(secrets)
            .map_err(|err| err.to_string())?
    } else if db.is_net_channel(name) {
        db.ensure_net_channel_secret(secrets)
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

/// `live_bot` false during connect queue drain — ingest history but do not re-run bots.
/// When `status` is set, bump `rx_seq` on a newly inserted channel/DM so the UI can refresh.
async fn handle_event(
    db: &Arc<Db>,
    meshcore: &MeshCore,
    event: MeshCoreEvent,
    radio_id: i64,
    live_bot: bool,
    status: Option<&Mutex<RadioStatus>>,
) -> Result<(), String> {
    match event.payload {
        EventPayload::ContactMessage(message) => {
            let inserted = on_direct(db, meshcore, message, radio_id).await?;
            if inserted {
                if let Some(status) = status {
                    bump_rx_seq(status);
                }
            }
        }
        EventPayload::ChannelMessage(message) => {
            let hops = hop_count_from_path_len(message.path_len).map(i64::from);
            let (sender_name, body) = channel_sender_and_body(&message.text);
            let sender_prefix = sender_name
                .clone()
                .unwrap_or_else(|| format!("ch{}", message.channel_idx));
            // Channel frames have no pubkey — bind only when the display name
            // matches exactly one roster node (never guess on collisions).
            let resolved = sender_name.as_ref().and_then(|name| {
                db.unique_node_by_display_name(name)
                    .ok()
                    .flatten()
                    .or_else(|| db.unique_node_by_name(name).ok().flatten())
            });
            let sender_pubkey = resolved.as_ref().map(|n| n.pubkey.clone());
            if let (Some(name), Some(pubkey)) = (sender_name.as_deref(), sender_pubkey.as_deref()) {
                let _ = db.promote_name_block_to_pubkey(pubkey, name);
                let _ = db.promote_name_pin_to_pubkey(pubkey, name);
            }
            if db
                .is_channel_sender_blocked(sender_name.as_deref(), sender_pubkey.as_deref())
                .unwrap_or(false)
            {
                // Name-only blocks / pins already promoted above when resolvable.
                hq_log!(
                    debug,
                    "dropping channel RX from blocked sender {:?}",
                    sender_name.as_deref().or(sender_pubkey.as_deref())
                );
                return Ok(());
            }
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
                "resolved_pubkey": sender_pubkey,
            });
            let inserted = db
                .insert_message(
                    "channel",
                    "rx",
                    sender_pubkey.as_deref(),
                    &sender_prefix,
                    Some(message.channel_idx as i64),
                    hops,
                    message.sender_timestamp,
                    &message.text,
                    &display,
                    message.snr.map(f64::from),
                    None,
                    &raw,
                    "received", Some(radio_id), "local",
                )
                .map_err(|err| err.to_string())?;
            if inserted.is_some() {
                if let Some(status) = status {
                    bump_rx_seq(status);
                }
            }
            if let (Some(message_id), Some(pubkey)) = (inserted, sender_pubkey.as_deref()) {
                let _ = db.touch_node_radio(pubkey, radio_id);
                // Zero-hop channel RX = direct RF; mark a unicast-capable route.
                // Multi-hop channel frames do not include reverse path bytes.
                if message.path_len == 0 {
                    let _ = db.set_route(pubkey, &[]);
                    let _ = db.save_path(Some(message_id), Some(pubkey), "direct", &[], 1);
                } else {
                    remember_route(db, meshcore, pubkey, message_id).await;
                }
            }
            // MeshCore-style session speakers under the channel in the rail —
            // live RX only (not connect-queue drain / history replay).
            if live_bot && inserted.is_some() {
                if let Some(name) = sender_name.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty())
                {
                    let hq_name = db.hq_advert_name().unwrap_or_default();
                    let from_self =
                        !hq_name.is_empty() && name.eq_ignore_ascii_case(&hq_name);
                    if !from_self {
                        if let Ok(Some(channel_name)) =
                            db.active_channel_on_slot(message.channel_idx as i64)
                        {
                            crate::channel_speakers::note(
                                &channel_name,
                                name,
                                sender_pubkey.as_deref(),
                            );
                        }
                    }
                }
            }
            // Bot commands live on METIS-INFO (private, enrolled secret), not DMs.
            // Skip during connect drain, skip our own advert name (local TX path
            // already answered), and claim so reconnect/redelivery cannot loop.
            if live_bot
                && inserted.is_some()
                && crate::services::is_metis_info_slot(db, message.channel_idx)
            {
                let hq_name = db.hq_advert_name().unwrap_or_default();
                let from_self = sender_name
                    .as_ref()
                    .is_some_and(|n| !hq_name.is_empty() && n.eq_ignore_ascii_case(&hq_name));
                if !from_self {
                    if let Some(cmd) = crate::services::parse_bot_command(&body) {
                        let claim = bot_command_claim_id(
                            message.channel_idx,
                            message.sender_timestamp,
                            &body,
                        );
                        match db.claim_bot_command(&claim) {
                            Ok(true) => handle_bot_channel(
                                db,
                                message.channel_idx,
                                resolved.as_ref(),
                                cmd,
                                false,
                            ),
                            Ok(false) => hq_log!(
                                debug,
                                "METIS-INFO cmd already handled ({claim})"
                            ),
                            Err(err) => hq_log!(warn, "bot claim: {err}"),
                        }
                    }
                }
            }
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

/// Returns true when a new message row was inserted (not a dedupe ignore).
async fn on_direct(
    db: &Arc<Db>,
    meshcore: &MeshCore,
    message: ContactMessage,
    radio_id: i64,
) -> Result<bool, String> {
    let prefix = message.sender_prefix_hex();
    let matches = db.nodes_by_prefix(&prefix).map_err(|err| err.to_string())?;
    let unique = matches.len() == 1;
    let current = matches
        .first()
        .map(|node| node.enrollment)
        .unwrap_or(crate::signup::Enrollment::Heard);
    // CLI_DATA (txt_type 1) is remote admin/stats reply — not signup chat.
    let is_cli_data = message.txt_type == 1;
    let decision = if is_cli_data {
        crate::signup::ReplyDecision {
            enrollment: current,
            requested_name: None,
            reply: None,
        }
    } else {
        let command = parse_command(&message.text);
        decide(&command, current, unique)
    };
    let pubkey = unique.then(|| matches[0].pubkey.clone());
    if let Some(ref key) = pubkey {
        if db.is_blocked(key).unwrap_or(false) {
            hq_log!(debug, "dropping direct RX from blocked {key}");
            return Ok(false);
        }
    }
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
            if is_cli_data { "cli" } else { "direct" },
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
            "received", Some(radio_id), "local",
        )
        .map_err(|err| err.to_string())?;
    let Some(message_id) = inserted else {
        return Ok(false);
    };
    if let Some(ref pubkey) = pubkey {
        if !is_cli_data {
            db.set_enrollment(pubkey, decision.enrollment, decision.requested_name.clone())
                .map_err(|err| err.to_string())?;
        }
        let _ = db.touch_node_radio(pubkey, radio_id);
        remember_route(db, meshcore, pubkey, message_id).await;
    }
    if let Some(reply) = decision.reply {
        if let Some(pubkey) = matches.first().map(|node| node.pubkey.clone()) {
            // Same gap as channel bot — stops reconnect/drain from double-queuing
            // identical enrollment replies within a minute.
            if crate::services::bot_rate_ok(&format!("dm-enroll:{pubkey}")) {
                db.queue_text(&pubkey, &reply, &reply)
                    .map_err(|err| err.to_string())?;
            } else {
                hq_log!(debug, "enrollment reply rate-limited for {pubkey}");
            }
        }
    }
    Ok(true)
}

/// METIS-INFO channel bot. Replies go back on the same channel slot.
///
/// `local_operator` is true when HQ typed the command in the Messages UI — the
/// companion never echoes our own TX as ChannelMsgRecv.
fn handle_bot_channel(
    db: &Arc<Db>,
    slot: u8,
    sender: Option<&crate::db::NodeRow>,
    command: crate::services::BotCommand,
    local_operator: bool,
) {
    let (rate_key, node_lat, node_lon, node_label) = if let Some(node) = sender {
        if !local_operator && !crate::services::is_enrolled(node.enrollment) {
            enqueue_channel_text(
                slot,
                crate::signup::fit_mesh("Not enrolled. DM #signup to this HQ first."),
            );
            return;
        }
        let label = if node.name.trim().is_empty() {
            node.prefix.clone()
        } else {
            node.name.clone()
        };
        (node.pubkey.clone(), node.lat, node.lon, label)
    } else if local_operator {
        (
            "local-hq".to_string(),
            None,
            None,
            db.setting("hq_name")
                .ok()
                .flatten()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "HQ".into()),
        )
    } else {
        hq_log!(debug, "METIS-INFO cmd ignored — sender name not unique on roster");
        return;
    };

    // Rate-limit remote enrolled radios only — HQ Messages UI must not fight itself.
    if !local_operator && !crate::services::bot_rate_ok(&rate_key) {
        enqueue_channel_text(slot, crate::services::bot_rate_wait_line());
        return;
    }

    match command {
        crate::services::BotCommand::Help => {
            let reply = crate::services::help_line();
            let _ = db.audit("svc_help", &rate_key, "channel");
            enqueue_channel_text(slot, reply);
        }
        crate::services::BotCommand::Wx { place, detail } => {
            if !crate::services::setting_bool(db, "svc_weather_enabled", true) {
                enqueue_channel_text(
                    slot,
                    crate::signup::fit_mesh("Weather service is off at this HQ."),
                );
                return;
            }
            let hq_lat = crate::services::setting_f64(db, "hq_lat", f64::NAN);
            let hq_lon = crate::services::setting_f64(db, "hq_lon", f64::NAN);
            let hq_lat = hq_lat.is_finite().then_some(hq_lat);
            let hq_lon = hq_lon.is_finite().then_some(hq_lon);
            let hq_label = db
                .setting("hq_name")
                .ok()
                .flatten()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "HQ".into());
            let fahrenheit = crate::services::weather_unit_f(db);
            let db = db.clone();
            let bare_place = place
                .as_ref()
                .map(|p| p.trim())
                .filter(|p| !p.is_empty())
                .is_none();
            let zip_hint = place.as_ref().and_then(|p| {
                let digits: String = p.chars().filter(|c| c.is_ascii_digit()).collect();
                (digits.len() == 5).then_some(digits)
            });
            tokio::task::spawn_blocking(move || {
                // Bare #weather / #weather+: prefer fresh local MQTT station.
                let station_line = if bare_place {
                    crate::services::try_station_wx_line(&db, fahrenheit)
                } else {
                    None
                };
                if !detail {
                    if let Some(line) = station_line {
                        let _ = db.audit("svc_wx", &rate_key, &line);
                        enqueue_channel_frames(slot, vec![line]);
                        return;
                    }
                }

                let resolved = if let Some(place) = place.as_deref().filter(|p| !p.trim().is_empty())
                {
                    match crate::map_tiles::geocode_place(place) {
                        Ok((lat, lon, label)) => {
                            let label =
                                crate::services::short_wx_label(&label, zip_hint.as_deref());
                            Some((lat, lon, label))
                        }
                        Err(err) => {
                            enqueue_channel_text(
                                slot,
                                crate::signup::fit_mesh(&format!("Place lookup failed: {err}")),
                            );
                            return;
                        }
                    }
                } else {
                    crate::services::resolve_weather_coords(
                        &db, node_lat, node_lon, hq_lat, hq_lon,
                    )
                    .map(|(lat, lon, src)| {
                        let label = if src == "node" {
                            crate::services::short_wx_label(&node_label, None)
                        } else {
                            crate::services::short_wx_label(&hq_label, None)
                        };
                        (lat, lon, label)
                    })
                };

                let frames = if detail {
                    match resolved {
                        Some((lat, lon, label)) => {
                            let query = crate::services::WeatherQuery {
                                lat,
                                lon,
                                label,
                                fahrenheit,
                            };
                            match crate::services::fetch_weather_detail(&query) {
                                Ok(mut frames) => {
                                    if let Some(station) = station_line {
                                        if frames.is_empty() {
                                            frames.push(station);
                                        } else {
                                            frames[0] = station;
                                        }
                                    }
                                    frames
                                }
                                Err(err) => {
                                    hq_log!(warn, "wx+: {err}");
                                    if let Some(station) = station_line {
                                        vec![station]
                                    } else {
                                        vec![crate::signup::fit_mesh(
                                            "Weather unavailable right now.",
                                        )]
                                    }
                                }
                            }
                        }
                        None => {
                            if let Some(station) = station_line {
                                vec![station]
                            } else {
                                enqueue_channel_text(
                                    slot,
                                    crate::signup::fit_mesh(
                                        "No position. Use #weather <zip> or set HQ site pin.",
                                    ),
                                );
                                return;
                            }
                        }
                    }
                } else {
                    let Some((lat, lon, label)) = resolved else {
                        enqueue_channel_text(
                            slot,
                            crate::signup::fit_mesh(
                                "No position. Use #weather <zip> or set HQ site pin.",
                            ),
                        );
                        return;
                    };
                    let query = crate::services::WeatherQuery {
                        lat,
                        lon,
                        label,
                        fahrenheit,
                    };
                    match crate::services::fetch_weather_line(&query) {
                        Ok(line) => vec![line],
                        Err(err) => {
                            hq_log!(warn, "wx: {err}");
                            vec![crate::signup::fit_mesh("Weather unavailable right now.")]
                        }
                    }
                };
                let audit = frames.first().cloned().unwrap_or_default();
                let _ = db.audit(if detail { "svc_wx+" } else { "svc_wx" }, &rate_key, &audit);
                enqueue_channel_frames(slot, frames);
            });
        }
        crate::services::BotCommand::Quake => {
            if !crate::services::setting_bool(db, "svc_quake_enabled", false) {
                enqueue_channel_text(
                    slot,
                    crate::signup::fit_mesh("Earthquake watch is off at this HQ."),
                );
                return;
            }
            let (min_lat, max_lat, min_lon, max_lon) = crate::services::quake_bbox(db);
            let min_mag = crate::services::quake_min_magnitude(db);
            let db = db.clone();
            tokio::task::spawn_blocking(move || {
                let reply = match crate::services::fetch_quakes(
                    min_lat, max_lat, min_lon, max_lon, min_mag, 24 * 60,
                ) {
                    Ok(events) => {
                        if let Some(event) = events.first() {
                            crate::services::format_quake(&db, event)
                        } else {
                            crate::signup::fit_mesh(&format!(
                                "USGS: no M{min_mag:.1}+ in HQ box last 24h"
                            ))
                        }
                    }
                    Err(err) => {
                        hq_log!(warn, "quake: {err}");
                        crate::signup::fit_mesh("USGS unavailable right now.")
                    }
                };
                let _ = db.audit("svc_quake", &rate_key, &reply);
                enqueue_channel_text(slot, reply);
            });
        }
        crate::services::BotCommand::Amber { detail } => {
            if !crate::services::setting_bool(db, "svc_amber_enabled", false) {
                enqueue_channel_text(
                    slot,
                    crate::signup::fit_mesh("Amber watch is off at this HQ."),
                );
                return;
            }
            let state = crate::services::emerg_state(db);
            let db = db.clone();
            tokio::task::spawn_blocking(move || {
                let frames = match crate::services::fetch_emergency_alerts(
                    &state,
                    crate::services::AMBER_CODES,
                ) {
                    Ok(alerts) => {
                        if let Some(alert) = alerts.first() {
                            let short = crate::services::shorten_url(&alert.url);
                            if detail {
                                crate::services::format_nws_detail(alert, short.as_deref())
                            } else {
                                vec![crate::services::format_nws_alert(alert, short.as_deref())]
                            }
                        } else {
                            vec![crate::signup::fit_mesh(&format!(
                                "AMBER: none active in {state}"
                            ))]
                        }
                    }
                    Err(err) => {
                        hq_log!(warn, "amber: {err}");
                        vec![crate::signup::fit_mesh("Amber feed unavailable right now.")]
                    }
                };
                let audit = frames.first().cloned().unwrap_or_default();
                let _ = db.audit(
                    if detail { "svc_amber+" } else { "svc_amber" },
                    &rate_key,
                    &audit,
                );
                enqueue_channel_frames(slot, frames);
            });
        }
        crate::services::BotCommand::Alert { detail } => {
            if !crate::services::setting_bool(db, "svc_emerg_enabled", false)
                && !crate::services::setting_bool(db, "svc_amber_enabled", false)
            {
                enqueue_channel_text(
                    slot,
                    crate::signup::fit_mesh("Emergency alert watch is off at this HQ."),
                );
                return;
            }
            let state = crate::services::emerg_state(db);
            let codes = if crate::services::setting_bool(db, "svc_emerg_enabled", false) {
                crate::services::EMERG_CODES
            } else {
                crate::services::AMBER_CODES
            };
            let db = db.clone();
            tokio::task::spawn_blocking(move || {
                let frames = match crate::services::fetch_emergency_alerts(&state, codes) {
                    Ok(alerts) => {
                        if let Some(alert) = alerts.first() {
                            let short = crate::services::shorten_url(&alert.url);
                            if detail {
                                crate::services::format_nws_detail(alert, short.as_deref())
                            } else {
                                vec![crate::services::format_nws_alert(alert, short.as_deref())]
                            }
                        } else {
                            vec![crate::signup::fit_mesh(&format!(
                                "ALERT: none active in {state}"
                            ))]
                        }
                    }
                    Err(err) => {
                        hq_log!(warn, "alert: {err}");
                        vec![crate::signup::fit_mesh("Alert feed unavailable right now.")]
                    }
                };
                let audit = frames.first().cloned().unwrap_or_default();
                let _ = db.audit(
                    if detail { "svc_alert+" } else { "svc_alert" },
                    &rate_key,
                    &audit,
                );
                enqueue_channel_frames(slot, frames);
            });
        }
    }
}

const TRACE_COOLDOWN: Duration = Duration::from_secs(8);

fn trace_rate_allows(pubkey_hex: &str) -> Result<(), String> {
    static LAST: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    let map = LAST.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = map.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    if let Some(prev) = guard.get(pubkey_hex) {
        let wait = TRACE_COOLDOWN.saturating_sub(now.saturating_duration_since(*prev));
        if !wait.is_zero() {
            return Err(format!(
                "Rate-limited — wait {}s before probing again.",
                wait.as_secs().max(1)
            ));
        }
    }
    guard.insert(pubkey_hex.to_string(), now);
    // Bound map size.
    if guard.len() > 200 {
        let cutoff = now.checked_sub(Duration::from_secs(600)).unwrap_or(now);
        guard.retain(|_, t| *t >= cutoff);
    }
    Ok(())
}

fn persist_path_source(persist_source: &str, kind: &str) -> String {
    if persist_source == "operator" && kind == "advert" {
        "operator".into()
    } else {
        kind.into()
    }
}

fn advert_fetch_from_path(
    path: Vec<u8>,
    hash_size: usize,
    source: String,
) -> AdvertPathFetch {
    let hop_count = if hash_size == 0 {
        0
    } else {
        (path.len() / hash_size) as u32
    };
    AdvertPathFetch {
        path,
        hash_size,
        source,
        hop_count,
    }
}

fn path_fetch_from_event(
    event: &meshcore_rs::events::MeshCoreEvent,
    pubkey_prefix: &[u8; 6],
) -> Option<(Vec<u8>, usize, &'static str)> {
    match &event.payload {
        // PUSH 0x81 — path_len < 0 means unknown / cleared (not a usable route).
        EventPayload::PathUpdate(update) if &update.prefix == pubkey_prefix => {
            if update.path_len < 0 {
                return None;
            }
            let hash_size = 1usize;
            let take = (update.path_len as usize).saturating_mul(hash_size);
            let path = update.path.iter().take(take).copied().collect::<Vec<_>>();
            let kind = if update.path_len == 0 {
                "direct"
            } else {
                "path_update"
            };
            Some((path, hash_size, kind))
        }
        // RESP_CODE_ADVERT_PATH (22 / 0x16) arrives as Unknown + Bytes in meshcore-rs.
        EventPayload::Bytes(raw) => parse_advert_path(raw).map(|(p, hs)| (p, hs, "advert")),
        _ => None,
    }
}

/// DB / contact fallbacks when the live 0x2A round-trip did not yield a path.
fn path_fetch_fallbacks(
    db: &Db,
    pubkey_hex: &str,
    message_id: Option<i64>,
    persist_source: &str,
) -> Result<AdvertPathFetch, String> {
    if let Ok(Some(stored)) = db.latest_path_for_pubkey(pubkey_hex) {
        if !stored.path.is_empty() || stored.hash_size > 0 {
            return Ok(advert_fetch_from_path(
                stored.path,
                stored.hash_size.max(1) as usize,
                persist_path_source(persist_source, &stored.source),
            ));
        }
    }
    if let Ok(nodes) = db.nodes_by_prefix(&pubkey_hex[..12.min(pubkey_hex.len())]) {
        if let Some(node) = nodes.into_iter().find(|node| node.pubkey == pubkey_hex) {
            if !node.hop_bytes.is_empty() {
                let source = persist_path_source(persist_source, "out_path");
                let _ = db.save_path(
                    message_id,
                    Some(pubkey_hex),
                    &source,
                    &node.hop_bytes,
                    1,
                );
                return Ok(advert_fetch_from_path(node.hop_bytes, 1, source));
            }
            // Companion contact with path_len 0 = direct (no mid hops). Still drawable.
            if node.path_len == 0 {
                let source = persist_path_source(persist_source, "direct");
                let _ = db.save_path(message_id, Some(pubkey_hex), &source, &[], 1);
                return Ok(AdvertPathFetch {
                    path: Vec::new(),
                    hash_size: 1,
                    source,
                    hop_count: 0,
                });
            }
        }
    }
    Err(
        "No path yet — companion has no cached advert route for this node (and no contact out_path). Hear its advert on-air first, or Trace a radio contact that already has a route."
            .into(),
    )
}

/// Fetch advert path via `CMD_GET_ADVERT_PATH` (0x2A).
///
/// Success reply is `RESP_CODE_ADVERT_PATH` (22) — meshcore-rs surfaces that as
/// `EventType::Unknown` + raw bytes. Cache miss is `PACKET_ERROR`. `PathUpdate`
/// is a separate push when a contact route changes; we still accept a matching
/// one if it races in.
async fn fetch_advert_path(
    db: &Db,
    meshcore: &MeshCore,
    pubkey_hex: &str,
    message_id: Option<i64>,
    persist_source: &str,
) -> Result<AdvertPathFetch, String> {
    let bytes = hex::decode(pubkey_hex).map_err(|_| "Invalid pubkey.".to_string())?;
    if bytes.len() != 32 {
        return Err("Pubkey must be 32 bytes.".into());
    }
    let mut pubkey_prefix = [0u8; 6];
    pubkey_prefix.copy_from_slice(&bytes[..6]);

    let mut frame = Vec::with_capacity(34);
    frame.push(0x2A);
    frame.push(0x00);
    frame.extend_from_slice(&bytes);

    let operator = persist_source == "operator";
    let send_wait = if operator {
        Duration::from_secs(3)
    } else {
        Duration::from_secs(2)
    };
    // Only wait for a late Unknown/PathUpdate when the companion did not already
    // answer with Error (cache miss) or a parseable path frame.
    let late_wait = if operator {
        Duration::from_secs(4)
    } else {
        Duration::from_secs(1)
    };

    // Subscribe before write so a late PathUpdate / Error is not missed while
    // we wait on the first reply (same listen-first helper as get_msg / channel TX).
    let mut late_rx = meshcore.dispatcher().receiver();
    let expected = [
        EventType::Unknown,
        EventType::Error,
        EventType::PathUpdate,
    ];

    let send_outcome = {
        let Some(commands) = lock_commands_brief(meshcore).await else {
            return path_fetch_fallbacks(db, pubkey_hex, message_id, persist_source);
        };
        request_listening_first(
            &commands,
            meshcore.dispatcher(),
            &frame,
            &expected,
            send_wait,
        )
        .await
    };

    if let Ok(ref event) = send_outcome {
        if event.event_type == EventType::Error {
            // ERR_NOT_FOUND (no advert-path cache entry) — skip late wait.
            hq_log!(
                debug,
                "path fetch: companion has no advert-path cache entry for {pubkey_hex}"
            );
            return path_fetch_fallbacks(db, pubkey_hex, message_id, persist_source);
        }
        if let Some((path, hash_size, kind)) = path_fetch_from_event(event, &pubkey_prefix) {
            let source = persist_path_source(persist_source, kind);
            let _ = db.save_path(
                message_id,
                Some(pubkey_hex),
                &source,
                &path,
                hash_size as i64,
            );
            // Advert-path cache is a usable reverse route for Status/DM — keep it.
            let _ = db.set_route(pubkey_hex, &path);
            return Ok(advert_fetch_from_path(path, hash_size, source));
        }
        // Unrelated Unknown/PathUpdate won the race — keep listening briefly.
    }

    let deadline = Instant::now() + late_wait;
    while Instant::now() < deadline {
        let left = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, late_rx.recv()).await {
            Ok(Ok(event)) => {
                if event.event_type == EventType::Error {
                    hq_log!(
                        debug,
                        "path fetch (late): no advert-path cache for {pubkey_hex}"
                    );
                    break;
                }
                if let Some((path, hash_size, kind)) =
                    path_fetch_from_event(&event, &pubkey_prefix)
                {
                    let source = persist_path_source(persist_source, kind);
                    let _ = db.save_path(
                        message_id,
                        Some(pubkey_hex),
                        &source,
                        &path,
                        hash_size as i64,
                    );
                    let _ = db.set_route(pubkey_hex, &path);
                    return Ok(advert_fetch_from_path(path, hash_size, source));
                }
            }
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => {}
            Ok(Err(_)) => break,
            Err(_) => break,
        }
    }

    path_fetch_fallbacks(db, pubkey_hex, message_id, persist_source)
}

async fn run_operator_path_fetch(
    db: &Db,
    meshcore: &MeshCore,
    pubkey_hex: &str,
) -> Result<AdvertPathFetch, String> {
    trace_rate_allows(pubkey_hex)?;
    fetch_advert_path(db, meshcore, pubkey_hex, None, "operator").await
}

const CONTACT_REFRESH_STATUS_TIMEOUT: Duration = Duration::from_secs(25);
const CONTACT_REFRESH_TELEM_TIMEOUT: Duration = Duration::from_secs(20);

/// Push/update this node on the companion contact table so unicast Status/DM
/// can route. Only push when HQ has a real route (`hop_bytes`) or direct
/// (`path_len == 0`). Never push flood (`path_len == -1`): MeshCore cannot
/// flood binary Status/Telemetry, and overwriting the companion with 0xFF
/// wipes any out_path it already had.
async fn ensure_radio_contact_for_probe(
    db: &Db,
    commands: &meshcore_rs::commands::CommandHandler,
    pubkey_hex: &str,
) -> Result<&'static str, String> {
    let key = parse_pubkey_full(pubkey_hex)?;
    let node = db
        .get_node(pubkey_hex)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Node not in HQ roster.".to_string())?;
    let name = {
        let n = node.name.trim();
        if n.is_empty() {
            pubkey_hex.chars().take(12).collect::<String>()
        } else {
            n.chars().take(32).collect()
        }
    };
    let (path_len, out_path, mode): (i8, Vec<u8>, &'static str) =
        if !node.hop_bytes.is_empty() {
            let len = node.hop_bytes.len().min(64) as i8;
            (len, node.hop_bytes.iter().take(64).copied().collect(), "route")
        } else if node.path_len == 0 {
            (0, Vec::new(), "direct")
        } else {
            // Leave the companion contact alone — no HQ route to apply.
            return Ok("unchanged");
        };
    let adv_lat = node
        .lat
        .filter(|v| v.abs() > 0.000_01)
        .map(|v| (v * 1_000_000.0).round() as i32)
        .unwrap_or(0);
    let adv_lon = node
        .lon
        .filter(|v| v.abs() > 0.000_01)
        .map(|v| (v * 1_000_000.0).round() as i32)
        .unwrap_or(0);
    let last_advert = node.last_advert.unwrap_or(0).clamp(0, u32::MAX as i64) as u32;
    let contact = Contact {
        public_key: key,
        contact_type: if matches!(node.contact_type, 1..=4) {
            node.contact_type
        } else {
            1
        },
        flags: 0,
        path_len,
        out_path,
        adv_name: name,
        last_advert,
        adv_lat,
        adv_lon,
        last_modification_timestamp: unix_now() as u32,
    };
    commands
        .add_contact(&contact)
        .await
        .map_err(|e| format!("Companion contact update failed: {e}"))?;
    Ok(mode)
}

/// Operator “Refresh contact”: pull companion contact row, path cache, then OTA status/telemetry.
async fn run_contact_refresh(
    db: &Db,
    meshcore: &MeshCore,
    status: &Mutex<RadioStatus>,
    radio_id: i64,
    pubkey_hex: &str,
) -> Result<ContactRefreshResult, String> {
    let key_hex = pubkey_hex.trim().to_ascii_lowercase();
    if key_hex.len() != 64 || !key_hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("Contact key must be 64 hex characters.".into());
    }
    trace_rate_allows(&key_hex)?;
    let key = parse_pubkey_full(&key_hex)?;

    write_status(
        status,
        "connected",
        format!(
            "Refreshing contact {}…",
            &key_hex.chars().take(8).collect::<String>()
        ),
    );

    // 1) Sync companion contact table so name / lat / lon / out_path match last heard advert.
    // Do not pass `status` into sync_contacts — that flips sync_phase to Contacts and
    // disables Messages compose until Ready is restored (easy to leave stuck).
    {
        let commands = match tokio::time::timeout(
            Duration::from_secs(8),
            meshcore.commands().lock(),
        )
        .await
        {
            Ok(guard) => guard,
            Err(_) => {
                write_sync(status, SyncPhase::Ready, "Companion ready", None);
                return Err("Contact refresh skipped — radio busy.".into());
            }
        };
        if let Err(err) = sync_contacts(
            db,
            &commands,
            meshcore.dispatcher(),
            None,
            radio_id,
        )
        .await
        {
            hq_log!(debug, "contact refresh sync: {err}");
        }
    }
    tokio::task::yield_now().await;

    let node = db.get_node(&key_hex).ok().flatten();
    let label = node
        .as_ref()
        .map(|n| {
            let name = n.name.trim();
            if name.is_empty() {
                key_hex.chars().take(8).collect::<String>()
            } else {
                name.to_string()
            }
        })
        .unwrap_or_else(|| key_hex.chars().take(8).collect());

    let mut parts: Vec<String> = Vec::new();
    if let Some(ref n) = node {
        if let (Some(lat), Some(lon)) = (n.lat, n.lon) {
            if lat.abs() > 0.000_01 || lon.abs() > 0.000_01 {
                parts.push(format!("GPS {lat:.4},{lon:.4}"));
            }
        }
        if let Some(adv) = n.last_advert.filter(|t| *t > 0) {
            parts.push(format!("last advert {adv}"));
        }
    }

    // 2) Path from companion advert-path cache / contact out_path (local — no mesh TX).
    // Channel chat is flood — hearing someone on a channel does not create a
    // unicast advert-path. "no path yet" is normal until their advert is cached.
    match fetch_advert_path(db, meshcore, &key_hex, None, "operator").await {
        Ok(fetch) => {
            let hops = if fetch.hop_count == 0 && fetch.source == "direct" {
                "direct".into()
            } else {
                format!("{} hop(s)", fetch.hop_count)
            };
            parts.push(format!("path {hops} ({})", fetch.source));
        }
        Err(_) => parts.push(
            "no unicast path yet (channel flood ≠ route — need advert/contact path)".into(),
        ),
    }
    tokio::task::yield_now().await;

    // 3) Ensure companion contact row (flood if no path), then OTA status.
    let prior_tx = db
        .get_infra_status_latest(&key_hex)
        .ok()
        .flatten()
        .and_then(|r| r.tx_power_dbm);
    let is_claimed = db
        .claimed_pubkey_set()
        .ok()
        .map(|s| s.contains(&key_hex))
        .unwrap_or(false);
    {
        let mut commands = match tokio::time::timeout(SEND_LOCK_WAIT, meshcore.commands().lock())
            .await
        {
            Ok(guard) => guard,
            Err(_) => {
                parts.push("status skipped (radio busy)".into());
                write_sync(status, SyncPhase::Ready, "Companion ready", None);
                return Ok(ContactRefreshResult {
                    summary: format!("{label}: {}", parts.join(" · ")),
                });
            }
        };
        let route_mode = match ensure_radio_contact_for_probe(db, &commands, &key_hex).await {
            Ok(mode) => {
                parts.push(format!("radio contact ({mode})"));
                mode
            }
            Err(err) => {
                hq_log!(debug, "contact refresh ensure contact: {err}");
                parts.push("radio contact update failed".into());
                "unknown"
            }
        };
        commands.set_default_timeout(PING_MSG_SENT_TIMEOUT);
        let status_result = commands
            .request_status_with_timeout(
                Destination::Bytes(key.to_vec()),
                CONTACT_REFRESH_STATUS_TIMEOUT,
            )
            .await;
        commands.set_default_timeout(Duration::from_secs(5));
        drop(commands);
        match status_result {
            Ok(info) => {
                persist_infra_status_probe(db, &key_hex, &info, prior_tx);
                parts.push(format!(
                    "status {} mV · RSSI {} · SNR {:.1}",
                    info.battery_mv, info.last_rssi, info.snr
                ));
            }
            Err(_) => {
                if route_mode == "unchanged" {
                    parts.push(
                        "status: no reply (no cached route — Status cannot flood)"
                            .into(),
                    );
                } else {
                    parts.push("status: no reply".into());
                }
            }
        }
    }
    tokio::task::yield_now().await;

    // 3b) Neighbours only for claimed infra (chat contacts rarely answer; timeouts look like bugs).
    if is_claimed {
        probe_and_persist_neighbours(db, meshcore, &key_hex).await;
        if let Ok(links) = db.list_infra_neighbor_links() {
            let n = links
                .iter()
                .filter(|l| l.from_pubkey.eq_ignore_ascii_case(&key_hex))
                .count();
            if n > 0 {
                parts.push(format!("{n} neighbour(s)"));
            }
        }
        tokio::task::yield_now().await;
    }

    // 4) Over-the-air telemetry (CayenneLPP when supported).
    {
        let commands = match tokio::time::timeout(
            Duration::from_secs(8),
            meshcore.commands().lock(),
        )
        .await
        {
            Ok(guard) => guard,
            Err(_) => {
                parts.push("telemetry skipped (radio busy)".into());
                write_sync(status, SyncPhase::Ready, "Companion ready", None);
                return Ok(ContactRefreshResult {
                    summary: format!("{label}: {}", parts.join(" · ")),
                });
            }
        };
        match commands
            .request_telemetry_with_timeout(
                Destination::Bytes(key.to_vec()),
                CONTACT_REFRESH_TELEM_TIMEOUT,
            )
            .await
        {
            Ok(bytes) => {
                let _ = db.audit(
                    "contact_telemetry",
                    &key_hex,
                    &format!("{} bytes", bytes.len()),
                );
                // Keep a coarse sample so Overview can show activity; Cayenne decode is later.
                let _ = db.insert_telemetry_sample(
                    "contact_telem",
                    bytes.len() as f64,
                    0.0,
                );
                parts.push(format!("telemetry {} bytes", bytes.len()));
            }
            Err(_) => parts.push("telemetry: no reply".into()),
        }
    }

    let summary = format!("{label}: {}", parts.join(" · "));
    let _ = db.audit("contact_refresh", &key_hex, &summary);
    // Always restore Ready so Messages compose stays usable after a refresh.
    write_sync(status, SyncPhase::Ready, "Companion ready", None);
    let companion_name = {
        let guard = status.lock().unwrap_or_else(|err| err.into_inner());
        if guard.connected_name.is_empty() {
            "companion".into()
        } else {
            guard.connected_name.clone()
        }
    };
    write_status(status, "connected", connected_detail(status, &companion_name));
    Ok(ContactRefreshResult { summary })
}

async fn remember_route(db: &Db, meshcore: &MeshCore, pubkey_hex: &str, message_id: i64) {
    let _ = fetch_advert_path(db, meshcore, pubkey_hex, Some(message_id), "advert").await;
}

/// `CMD_SEND_TXT_MSG` (2) to a contact, listening for `MsgSent` before the write.
async fn send_direct_text(
    commands: &meshcore_rs::commands::CommandHandler,
    dispatcher: &meshcore_rs::events::EventDispatcher,
    pubkey_hex: &str,
    body: &str,
) -> Result<meshcore_rs::events::MsgSentInfo, String> {
    let prefix = Destination::Hex(pubkey_hex.to_string())
        .prefix()
        .map_err(|err| err.to_string())?;
    // TXT_TYPE_PLAIN = 0, attempt = 0
    let mut data = vec![0x02u8, 0x00, 0x00];
    data.extend_from_slice(&(unix_now() as u32).to_le_bytes());
    data.extend_from_slice(&prefix);
    data.extend_from_slice(body.as_bytes());
    let event = request_listening_first(
        commands,
        dispatcher,
        &data,
        &[EventType::MsgSent, EventType::Error],
        Duration::from_secs(10),
    )
    .await?;
    match event.payload {
        EventPayload::MsgSent(info) => Ok(info),
        EventPayload::String(err) => Err(err),
        _ => Err("Companion rejected the direct message.".into()),
    }
}

/// Drain one queued broadcast leg (contact DM or channel flood). Bypasses
/// [`REPLY_GAP`] — operator blasts are intentional, not automatic bot replies.
async fn flush_one_broadcast_leg(
    db: &Arc<Db>,
    meshcore: &MeshCore,
    status: &Mutex<RadioStatus>,
    connected_name: &str,
    radio_id: i64,
) -> Result<(), String> {
    let Some(leg) = db.next_broadcast_leg().map_err(|err| err.to_string())? else {
        return Ok(());
    };
    match leg.group_kind {
        BroadcastGroupKind::Contact => {
            let key = leg.member_key.trim().to_ascii_lowercase();
            if !is_broadcast_pubkey(&key) {
                let _ = db.mark_broadcast_leg(leg.id, "skipped", "invalid public key");
                return Ok(());
            }
            let Some(commands) = tokio::time::timeout(SEND_LOCK_WAIT, meshcore.commands().lock())
                .await
                .ok()
            else {
                // Leave leg queued for the next tick (do not mark failed).
                return Ok(());
            };
            match send_direct_text(&commands, meshcore.dispatcher(), &key, &leg.body).await {
                Ok(info) => {
                    let ack = hex::encode(info.expected_ack);
                    let prefix = key.chars().take(12).collect::<String>();
                    let raw = json!({
                        "expected_ack": ack,
                        "message_type": info.message_type,
                        "timeout_ms": info.suggested_timeout,
                        "broadcast_leg": leg.id,
                        "broadcast_job": leg.job_id,
                    });
                    let _ = db.insert_message(
                        "direct",
                        "tx",
                        Some(&key),
                        &prefix,
                        None,
                        None,
                        unix_now() as u32,
                        &leg.body,
                        &leg.body,
                        None,
                        None,
                        &raw,
                        "sent",
                        Some(radio_id),
                        "local",
                    );
                    let _ = db.mark_broadcast_leg(leg.id, "sent", "");
                }
                Err(err) => {
                    let _ = db.mark_broadcast_leg(leg.id, "failed", &err);
                }
            }
        }
        BroadcastGroupKind::Channel => {
            let slot = match db
                .active_channel_slot_by_name(&leg.member_key)
                .map_err(|err| err.to_string())?
            {
                Some(slot) => slot,
                None => {
                    let _ = db.mark_broadcast_leg(
                        leg.id,
                        "skipped",
                        "channel missing or inactive",
                    );
                    return Ok(());
                }
            };
            match transmit_channel_message(
                db,
                meshcore,
                status,
                connected_name,
                radio_id,
                slot,
                &leg.body,
                true,
                false,
            )
            .await
            {
                Ok(()) => {
                    let _ = db.mark_broadcast_leg(leg.id, "sent", "");
                }
                Err(err) => {
                    let _ = db.mark_broadcast_leg(leg.id, "failed", &err);
                }
            }
        }
    }
    tokio::time::sleep(BROADCAST_LEG_GAP).await;
    Ok(())
}

async fn flush_one_outbound(
    db: &Arc<Db>,
    meshcore: &MeshCore,
    radio_id: i64,
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
    let Some(commands) = tokio::time::timeout(SEND_LOCK_WAIT, meshcore.commands().lock())
        .await
        .ok()
    else {
        return Err("Radio is busy (sync in progress). Try again in a moment.".into());
    };
    match send_direct_text(&commands, meshcore.dispatcher(), &outbound.pubkey, &outbound.body).await
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
                "sent", Some(radio_id), "local",
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
pub fn prefix_channel_sender(name: &str, text: &str) -> String {
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

    #[test]
    fn need_msg_drain_after_cleared_stops_retry() {
        assert!(!need_msg_drain_after(MsgDrainResult::Cleared));
    }

    #[test]
    fn need_msg_drain_after_busy_or_more_keeps_retry() {
        assert!(need_msg_drain_after(MsgDrainResult::Busy));
        assert!(need_msg_drain_after(MsgDrainResult::MorePending));
    }

    #[test]
    fn bump_rx_seq_increments_for_ui_invalidate() {
        let status = Mutex::new(RadioStatus::default());
        bump_rx_seq(&status);
        bump_rx_seq(&status);
        let guard = status.lock().unwrap();
        assert_eq!(guard.rx_seq, 2);
    }

    #[test]
    fn receiving_busy_hint_sets_and_clears_connected_detail() {
        let status = Mutex::new(RadioStatus {
            state: "connected".into(),
            connected_name: "HQ-1".into(),
            detail: "Connected to HQ-1.".into(),
            ..RadioStatus::default()
        });
        set_receiving_busy_hint(&status, true);
        {
            let guard = status.lock().unwrap();
            assert!(guard.detail.starts_with("Receiving…"), "{}", guard.detail);
            assert_eq!(guard.state, "connected");
        }
        set_receiving_busy_hint(&status, false);
        let guard = status.lock().unwrap();
        assert!(!guard.detail.starts_with("Receiving…"), "{}", guard.detail);
        assert!(guard.detail.contains("HQ-1"), "{}", guard.detail);
    }

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
    fn status_probe_needs_full_32_byte_key_not_prefix() {
        let hex64 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let full = parse_pubkey_full(hex64).expect("full key");
        assert_eq!(full.len(), 32);
        assert!(parse_pubkey_full(&hex64[..12]).is_err());
        let prefix = parse_pubkey_prefix(hex64).expect("prefix");
        assert_eq!(prefix.len(), 6);
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

    #[test]
    fn primary_prefers_the_saved_choice_when_connected() {
        assert_eq!(pick_primary(Some(7), &[3, 7, 9]), Some(7));
    }

    #[test]
    fn primary_promotes_lowest_connected_when_choice_drops() {
        // Saved primary 2 is not connected: lowest id among the rest wins.
        assert_eq!(pick_primary(Some(2), &[9, 4, 6]), Some(4));
        assert_eq!(pick_primary(None, &[5, 3]), Some(3));
    }

    #[test]
    fn primary_is_none_without_connected_companions() {
        assert_eq!(pick_primary(Some(1), &[]), None);
        assert_eq!(pick_primary(None, &[]), None);
    }

    #[test]
    fn only_disconnect_skips_the_primary_gate() {
        assert!(!RadioCmd::Disconnect.needs_primary());
        assert!(RadioCmd::SendAdvert { flood: false }.needs_primary());
        assert!(RadioCmd::Reboot.needs_primary());
        assert!(RadioCmd::SendChannel {
            slot: 1,
            text: "hi".into(),
            bot_activity: false,
            reply: None,
        }
        .needs_primary());
    }

    #[test]
    fn secondary_refuses_channel_send_with_a_reason() {
        let status = Mutex::new(RadioStatus::default());
        let (tx, mut rx) = oneshot::channel();
        reject_cmd(
            RadioCmd::SendChannel {
                slot: 1,
                text: "hello".into(),
                bot_activity: false,
                reply: Some(tx),
            },
            &status,
            NOT_PRIMARY_REASON,
        );
        let err = rx.try_recv().expect("reply sent").expect_err("must refuse");
        assert!(err.contains("not the primary"), "{err}");
        let guard = status.lock().unwrap();
        assert_eq!(guard.send_note, NOT_PRIMARY_REASON);
        assert_eq!(guard.send_seq, 1);
    }

    #[test]
    fn no_primary_message_is_command_specific() {
        let send = RadioCmd::SendChannel {
            slot: 0,
            text: String::new(),
            bot_activity: false,
            reply: None,
        };
        assert!(no_primary_reason(&send).contains("not transmitted"));
        assert!(no_primary_reason(&RadioCmd::Reboot).contains("device settings"));
    }

    #[test]
    fn usb_label_falls_back_to_the_path_leaf() {
        assert_eq!(usb_fallback_label("/dev/ttyACM0"), "ttyACM0");
        assert_eq!(usb_fallback_label("COM3"), "COM3");
    }

    #[test]
    fn usb_permission_denied_is_fatal_with_os_hint() {
        let msg = clarify_usb_connect_error("Failed to open serial port: Permission denied");
        assert!(is_fatal_usb_session_error(&msg));
        assert!(
            msg.contains("dialout")
                || msg.contains("USB accessories")
                || msg.contains("COM port")
                || msg.contains("serial port"),
            "{msg}"
        );
        assert!(!is_fatal_usb_session_error("Connection reset by peer"));
    }

    #[test]
    fn usb_selfinfo_timeout_is_fatal_with_companion_hint() {
        let msg = clarify_usb_session_error("Timeout waiting for Some(SelfInfo)");
        assert!(is_fatal_usb_session_error(&msg));
        assert!(msg.to_ascii_lowercase().contains("companion"), "{msg}");
    }

    #[test]
    fn usb_missing_device_is_recoverable_for_sticky_rematch() {
        let msg = clarify_usb_connect_error("No such file or directory (os error 2)");
        assert!(!is_fatal_usb_session_error(&msg), "{msg}");
        assert!(msg.to_ascii_lowercase().contains("rematch"), "{msg}");
    }

    #[test]
    fn ble_already_connected_is_explicit_busy() {
        let raw = "Bluetooth error: Already connected";
        assert!(is_explicit_companion_busy_error(raw));
        let msg = clarify_ble_connect_error(raw);
        assert!(msg.to_ascii_lowercase().contains("companion busy"), "{msg}");
        assert!(msg.to_ascii_lowercase().contains("phone"), "{msg}");
    }

    #[test]
    fn early_ble_not_connected_looks_held() {
        assert!(looks_like_held_companion(
            "ble",
            "Channel push failed — radio link likely dropped: BLE TX error: Not connected"
        ));
        assert!(looks_like_held_companion(
            "ble",
            "Timeout waiting for Some(SelfInfo)"
        ));
        assert!(!is_explicit_companion_busy_error(
            "Timeout waiting for Some(SelfInfo)"
        ));
    }

    #[test]
    fn ble_appstart_timeout_hints_phone_and_elevation() {
        let msg = clarify_appstart_timeout("ble", "Timeout waiting for Some(SelfInfo)");
        let lower = msg.to_ascii_lowercase();
        assert!(lower.contains("appstart"), "{msg}");
        assert!(lower.contains("phone"), "{msg}");
        assert!(lower.contains("elevated") || lower.contains("admin"), "{msg}");
    }

    #[test]
    fn ble_handshake_failures_detected_for_stop_reconnect() {
        assert!(is_ble_handshake_failure("Timeout waiting for Some(SelfInfo)"));
        assert!(is_ble_handshake_failure(
            "Timeout waiting for response Bluetooth linked, but the companion never answered APPSTART."
        ));
        assert!(is_ble_handshake_failure("MeshCore device not found"));
        assert!(is_ble_handshake_failure(&format!(
            "Connection error: {}: the system refused access",
            meshcore_rs::BLE_NOT_PAIRED
        )));
        assert!(!is_ble_handshake_failure("Radio event stream ended."));
    }

    #[test]
    fn ble_not_paired_event_ends_handshake_wait() {
        let msg = format!("{}: refused", meshcore_rs::BLE_NOT_PAIRED);
        let event = MeshCoreEvent::error(msg.clone());
        assert_eq!(ble_not_paired_error(&event), Some(msg));
        assert_eq!(ble_not_paired_error(&MeshCoreEvent::error("ERR 2")), None);
        assert_eq!(ble_not_paired_error(&MeshCoreEvent::ok()), None);
    }

    #[test]
    fn held_disconnect_keeps_existing_busy_copy() {
        let existing = companion_held_message("ble");
        assert_eq!(format_held_disconnect("ble", existing), existing);
    }

    #[test]
    fn parses_discover_resp_with_push_envelope() {
        let tag = [1u8, 2, 3, 4];
        let mut pubkey = [0u8; 32];
        pubkey[0] = 0xAB;
        pubkey[31] = 0xCD;
        // PUSH body: snr, rssi, path_len=0, then 0x92|type, snr, tag, pubkey
        let mut payload = vec![10i8 as u8, 20i8 as u8, 0];
        payload.push(0x92); // DISCOVER_RESP | repeater
        payload.push(10);
        payload.extend_from_slice(&tag);
        payload.extend_from_slice(&pubkey);
        let (ty, key) = parse_discover_resp_payload(&payload, tag).expect("parse");
        assert_eq!(ty, 2);
        assert_eq!(key, pubkey);
        assert!(parse_discover_resp_payload(&payload, [9, 9, 9, 9]).is_none());
    }

    #[test]
    fn rx_log_line_omits_payload_bytes() {
        let log = LogData {
            snr: 8.5,
            rssi: -91,
            header: None,
            advertisement: None,
            payload: vec![0xDE, 0xAD],
        };
        let line = format_rx_log_line(&log);
        assert!(line.contains("SNR +8.5"), "{line}");
        assert!(line.contains("RSSI -91 dBm"), "{line}");
        assert!(!line.contains("dead") && !line.contains("DE AD"), "{line}");
    }
}
