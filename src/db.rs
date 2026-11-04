//! Local history. WAL SQLite, one file for every radio.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

use crate::mesh_text::text_mentions_name;
use crate::messages_appearance::{
    parse_appearance_json, MessagesAppearance, APPEARANCE_SETTING,
};
use crate::path::dedupe_key;
use crate::secrets::{
    SecretStoreKind, Secrets, CHANNEL_NAME, MAX_CHANNEL_NAME_BYTES, NET_CHANNEL_NAME,
};
use crate::signup::{approval_message, Enrollment};

const REDACTED_SECRET: &str = "[secret sent]";

/// Which HQ-owned private channel a rename targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemChannel {
    Info,
    Net,
}

impl SystemChannel {
    fn setting_key(self) -> &'static str {
        match self {
            Self::Info => "channel_info_name",
            Self::Net => "channel_net_name",
        }
    }

    fn default_name(self) -> &'static str {
        match self {
            Self::Info => CHANNEL_NAME,
            Self::Net => NET_CHANNEL_NAME,
        }
    }

    fn role(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Net => "net",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Info => "bot home",
            Self::Net => "peer net",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("{0}")]
    Msg(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// WAL + durability pragmas for file-backed HQ databases.
///
/// Call after `Db::open` / restore. Fails closed if the filesystem cannot run WAL
/// (some network mounts reject it). In-memory connections stay `journal_mode=memory`
/// (SQLite cannot WAL them) — used only by unit tests via `Db::from_conn`.
fn apply_runtime_pragmas(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    let mode: String = conn.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
    let mode_l = mode.to_ascii_lowercase();
    if mode_l == "memory" {
        // In-memory DB — WAL is impossible; keep busy timeout + sync for tests.
        conn.busy_timeout(std::time::Duration::from_millis(5000))?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        return Ok(());
    }
    if mode_l != "wal" {
        return Err(Error::Msg(format!(
            "SQLite journal_mode is '{mode}', expected WAL. Use a local disk for the HQ data folder."
        )));
    }
    // Covers Backup API's second connection and any future multi-conn readers.
    conn.busy_timeout(std::time::Duration::from_millis(5000))?;
    // NORMAL is the usual durable pairing with WAL (faster than FULL).
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct NodeRow {
    pub pubkey: String,
    pub name: String,
    pub requested_name: Option<String>,
    pub contact_type: u8,
    pub enrollment: Enrollment,
    pub last_heard: Option<i64>,
    pub last_advert: Option<i64>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub has_gps: bool,
    pub prefix: String,
    pub hop_bytes: Vec<u8>,
    pub path_len: i64,
    /// Relative file under `data/qr/` when a contact QR has been generated.
    pub qr_file: Option<String>,
    /// Exact `meshcore://contact/add…` URI the file encodes (stale if name/type changes).
    pub qr_uri: Option<String>,
    /// False until the operator opens the node on Roster (live-heard only).
    pub noticed: bool,
    /// HQ radio that most recently heard this node (`radios.id`).
    pub last_radio_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DmThreadRow {
    pub pubkey: String,
    pub name: String,
    pub last_text: Option<String>,
    pub last_message_at: Option<i64>,
    pub message_count: i64,
    pub unread_count: i64,
    /// Unread RX messages that @mention the HQ companion.
    pub unread_mentions: i64,
}

#[derive(Debug, Clone)]
pub struct MessageRow {
    pub id: i64,
    pub direction: String,
    #[allow(dead_code)]
    pub kind: String,
    pub sender_name: String,
    pub sender_pubkey: Option<String>,
    #[allow(dead_code)]
    pub channel_idx: Option<i64>,
    pub hop_count: Option<i64>,
    pub text: String,
    #[allow(dead_code)]
    pub delivery: String,
    pub received_at: i64,
    #[allow(dead_code)]
    pub route_source: Option<String>,
    pub path: Vec<u8>,
    pub hash_size: i64,
    pub snr: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct RfSampleRow {
    #[allow(dead_code)]
    pub pubkey: String,
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    pub noise_floor: i64,
    #[allow(dead_code)]
    pub last_rssi: i64,
    #[allow(dead_code)]
    pub snr: f64,
    #[allow(dead_code)]
    pub at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelRow {
    pub name: String,
    pub slot: i64,
    pub visibility: String,
    pub role: String,
    pub active: bool,
    pub message_count: i64,
    /// Received (`rx`) messages newer than `channel_last_read_{slot}`; 0 if never opened.
    /// Outbound HQ/bot posts do not count.
    pub unread_count: i64,
    /// Unread RX messages that @mention the HQ companion.
    pub unread_mentions: i64,
    pub last_message_at: Option<i64>,
    pub last_text: Option<String>,
    /// Per-channel override: `1h`…`forever`, or `None` to use the global default.
    pub message_keep: Option<String>,
    /// True when this channel has a local appearance override JSON.
    pub has_appearance_override: bool,
}

#[derive(Debug, Clone)]
pub struct OutboundRow {
    pub id: i64,
    pub pubkey: String,
    pub body: String,
}

#[derive(Debug, Clone)]
pub struct ServiceEventRow {
    pub id: String,
    pub kind: String,
    pub body: String,
    #[allow(dead_code)]
    pub detail: String,
    #[allow(dead_code)]
    pub status: String,
    pub created_at: i64,
    #[allow(dead_code)]
    pub decided_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct PurgePreview {
    pub messages: i64,
    pub paths: i64,
    pub positions: i64,
    pub telemetry: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HourlyTraffic {
    pub hour_start: i64,
    pub rx: i64,
    pub tx: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct KindCount {
    pub kind: String,
    pub count: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TelemetryPoint {
    pub at: i64,
    pub value_pct: f64,
    pub value_mv: f64,
}

/// Operator-facing role for a saved HQ radio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadioRole {
    Companion,
    Room,
    Repeater,
    Other,
}

impl RadioRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Companion => "companion",
            Self::Room => "room",
            Self::Repeater => "repeater",
            Self::Other => "other",
        }
    }

    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "room" | "room_server" | "room-server" => Self::Room,
            "repeater" => Self::Repeater,
            "other" => Self::Other,
            _ => Self::Companion,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Companion => "Companion",
            Self::Room => "Room server",
            Self::Repeater => "Repeater",
            Self::Other => "Other",
        }
    }
}

#[derive(Debug, Clone)]
pub struct RadioRow {
    pub id: i64,
    pub transport: String,
    pub address: String,
    pub label: String,
    pub role: RadioRole,
    pub last_error: String,
    pub last_connected: Option<i64>,
    /// Sticky USB identity (`vid:…:pid:…[:sn:…]`); empty for BLE/TCP or unknown USB.
    pub usb_identity: String,
}

pub struct Db {
    conn: Mutex<Connection>,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| Error::Msg(err.to_string()))?;
        }
        let conn = Connection::open(path)?;
        let db = Self::from_conn(conn)?;
        apply_runtime_pragmas(&db.lock())?;
        Ok(db)
    }

    pub fn from_conn(conn: Connection) -> Result<Self> {
        conn.execute_batch(SCHEMA)?;
        // Older databases created before last_connected existed.
        let _ = conn.execute(
            "ALTER TABLE radios ADD COLUMN last_connected INTEGER",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE radios ADD COLUMN role TEXT NOT NULL DEFAULT 'companion'",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE channels ADD COLUMN visibility TEXT NOT NULL DEFAULT 'private'",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE channels ADD COLUMN role TEXT NOT NULL DEFAULT 'general'",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE channels ADD COLUMN active INTEGER NOT NULL DEFAULT 1",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE channels ADD COLUMN created_at INTEGER",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE channels ADD COLUMN message_keep TEXT",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE channels ADD COLUMN appearance_json TEXT",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE nodes ADD COLUMN qr_file TEXT",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE nodes ADD COLUMN qr_uri TEXT",
            [],
        );
        // Existing rows stay noticed; only live inserts set noticed=0.
        let _ = conn.execute(
            "ALTER TABLE nodes ADD COLUMN noticed INTEGER NOT NULL DEFAULT 1",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE nodes ADD COLUMN last_radio_id INTEGER",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE radios ADD COLUMN usb_identity TEXT NOT NULL DEFAULT ''",
            [],
        );
        let _ = conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS node_radios (
                pubkey TEXT NOT NULL,
                radio_id INTEGER NOT NULL,
                last_heard INTEGER NOT NULL,
                PRIMARY KEY (pubkey, radio_id)
            );
            CREATE INDEX IF NOT EXISTS node_radios_radio ON node_radios(radio_id);",
        );
        let _ = conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS rf_samples (
                id INTEGER PRIMARY KEY,
                pubkey TEXT NOT NULL,
                noise_floor INTEGER NOT NULL,
                last_rssi INTEGER NOT NULL,
                snr REAL NOT NULL DEFAULT 0,
                at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS rf_samples_time ON rf_samples(at);",
        );
        let _ = conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS tracked_nodes (
                pubkey TEXT PRIMARY KEY,
                label TEXT NOT NULL DEFAULT '',
                notes TEXT NOT NULL DEFAULT '',
                added_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS blocked_nodes (
                pubkey TEXT PRIMARY KEY,
                label TEXT NOT NULL DEFAULT '',
                added_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS positions_pubkey_time ON positions(pubkey, at);",
        );
        let _ = conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS claimed_infra (
                pubkey TEXT PRIMARY KEY,
                kind TEXT NOT NULL DEFAULT 'repeater',
                label TEXT NOT NULL DEFAULT '',
                notes TEXT NOT NULL DEFAULT '',
                added_at INTEGER NOT NULL,
                verified_at INTEGER NOT NULL DEFAULT 0
            );",
        );
        let _ = conn.execute(
            "ALTER TABLE claimed_infra ADD COLUMN verified_at INTEGER NOT NULL DEFAULT 0",
            [],
        );
        let _ = conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS service_events (
                id TEXT PRIMARY KEY,
                kind TEXT NOT NULL,
                body TEXT NOT NULL,
                detail TEXT NOT NULL DEFAULT '{}',
                status TEXT NOT NULL DEFAULT 'pending',
                created_at INTEGER NOT NULL,
                decided_at INTEGER
            );
            CREATE INDEX IF NOT EXISTS service_events_status ON service_events(status, created_at);",
        );
        let _ = conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS bot_handled (
                id TEXT PRIMARY KEY,
                at INTEGER NOT NULL
            );",
        );
        let _ = conn.execute(
            "ALTER TABLE messages ADD COLUMN mentions_hq INTEGER NOT NULL DEFAULT 0",
            [],
        );
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|err| err.into_inner())
    }

    /// Private channel names (secret inventory for backup), including inactive.
    pub fn private_channel_names(&self) -> Result<Vec<String>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT name FROM channels
             WHERE lower(COALESCE(visibility, 'private')) != 'public'
             ORDER BY name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Error::from)
    }

    /// Online SQLite backup into a byte snapshot (WAL-safe).
    pub fn sqlite_snapshot_bytes(&self) -> Result<Vec<u8>> {
        use rusqlite::backup::Backup;
        use std::time::Duration;

        let path = unique_temp_db_path("metis-snap");
        {
            let src = self.lock();
            let mut dst = Connection::open(&path)?;
            {
                let backup = Backup::new(&src, &mut dst)?;
                backup
                    .run_to_completion(64, Duration::from_millis(1), None)
                    .map_err(|e| Error::Msg(e.to_string()))?;
            }
        }
        let bytes = std::fs::read(&path).map_err(|e| Error::Msg(e.to_string()))?;
        let _ = std::fs::remove_file(&path);
        Ok(bytes)
    }

    /// Replace this connection's database contents from a snapshot.
    pub fn restore_sqlite_bytes(&self, bytes: &[u8]) -> Result<()> {
        use rusqlite::backup::Backup;
        use std::time::Duration;

        let path = unique_temp_db_path("metis-restore");
        std::fs::write(&path, bytes).map_err(|e| Error::Msg(e.to_string()))?;
        {
            let src = Connection::open(&path)?;
            let mut dst = self.lock();
            {
                let backup = Backup::new(&src, &mut dst)?;
                backup
                    .run_to_completion(64, Duration::from_millis(1), None)
                    .map_err(|e| Error::Msg(e.to_string()))?;
            }
            apply_runtime_pragmas(&dst)?;
        }
        let _ = std::fs::remove_file(&path);
        // Re-apply additive migrations in case the snapshot is from an older build.
        {
            let conn = self.lock();
            // from_conn migrations are not re-run; open a side connection on the
            // same in-memory DB is impossible — re-run the ALTER/CREATE IF NOT EXISTS
            // batch against the live connection.
            drop(conn);
        }
        Self::apply_migrations(&self.lock())?;
        Ok(())
    }

    fn apply_migrations(conn: &Connection) -> Result<()> {
        let _ = conn.execute(
            "ALTER TABLE radios ADD COLUMN last_connected INTEGER",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE radios ADD COLUMN role TEXT NOT NULL DEFAULT 'companion'",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE channels ADD COLUMN visibility TEXT NOT NULL DEFAULT 'private'",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE channels ADD COLUMN role TEXT NOT NULL DEFAULT 'general'",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE channels ADD COLUMN active INTEGER NOT NULL DEFAULT 1",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE channels ADD COLUMN created_at INTEGER",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE channels ADD COLUMN message_keep TEXT",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE channels ADD COLUMN appearance_json TEXT",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE nodes ADD COLUMN qr_file TEXT",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE nodes ADD COLUMN qr_uri TEXT",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE nodes ADD COLUMN noticed INTEGER NOT NULL DEFAULT 1",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE radios ADD COLUMN usb_identity TEXT NOT NULL DEFAULT ''",
            [],
        );
        let _ = conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS rf_samples (
                id INTEGER PRIMARY KEY,
                pubkey TEXT NOT NULL,
                noise_floor INTEGER NOT NULL,
                last_rssi INTEGER NOT NULL,
                snr REAL NOT NULL DEFAULT 0,
                at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS rf_samples_time ON rf_samples(at);
            CREATE TABLE IF NOT EXISTS tracked_nodes (
                pubkey TEXT PRIMARY KEY,
                label TEXT NOT NULL DEFAULT '',
                notes TEXT NOT NULL DEFAULT '',
                added_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS claimed_infra (
                pubkey TEXT PRIMARY KEY,
                kind TEXT NOT NULL DEFAULT 'repeater',
                label TEXT NOT NULL DEFAULT '',
                notes TEXT NOT NULL DEFAULT '',
                added_at INTEGER NOT NULL,
                verified_at INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS service_events (
                id TEXT PRIMARY KEY,
                kind TEXT NOT NULL,
                body TEXT NOT NULL,
                detail TEXT NOT NULL DEFAULT '{}',
                status TEXT NOT NULL DEFAULT 'pending',
                created_at INTEGER NOT NULL,
                decided_at INTEGER
            );
            CREATE TABLE IF NOT EXISTS bot_handled (
                id TEXT PRIMARY KEY,
                at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS blocked_nodes (
                pubkey TEXT PRIMARY KEY,
                label TEXT NOT NULL DEFAULT '',
                added_at INTEGER NOT NULL
            );",
        );
        let _ = conn.execute(
            "ALTER TABLE messages ADD COLUMN mentions_hq INTEGER NOT NULL DEFAULT 0",
            [],
        );
        Ok(())
    }

    /// Counts inside a SQLite snapshot (for restore preview).
    pub fn count_snapshot(bytes: &[u8]) -> Result<(i64, i64, i64)> {
        let path = std::env::temp_dir().join(format!(
            "metis-count-{}-{}.db",
            std::process::id(),
            unix_now()
        ));
        std::fs::write(&path, bytes).map_err(|e| Error::Msg(e.to_string()))?;
        let conn = Connection::open(&path)?;
        let nodes: i64 = conn
            .query_row("SELECT COUNT(*) FROM nodes", [], |r| r.get(0))
            .unwrap_or(0);
        let messages: i64 = conn
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
            .unwrap_or(0);
        let channels: i64 = conn
            .query_row("SELECT COUNT(*) FROM channels", [], |r| r.get(0))
            .unwrap_or(0);
        let _ = std::fs::remove_file(&path);
        Ok((nodes, messages, channels))
    }

    /// Merge a roster contact from bulk import. Never pushes to the radio.
    /// Returns `"new"`, `"updated"`, or `"skipped"`.
    pub fn import_roster_contact(
        &self,
        pubkey: &str,
        name: &str,
        contact_type: u8,
        lat: Option<f64>,
        lon: Option<f64>,
        enrollment: Enrollment,
        last_heard: Option<i64>,
    ) -> Result<&'static str> {
        let key = pubkey.trim().to_ascii_lowercase();
        if key.len() != 64 || !key.chars().all(|c| c.is_ascii_hexdigit()) {
            return Ok("skipped");
        }
        let ty = match contact_type {
            1..=4 => contact_type,
            _ => 1,
        };
        let (lat, lon) = match (lat, lon) {
            (Some(la), Some(lo)) if la.abs() > 0.000_001 || lo.abs() > 0.000_001 => {
                (Some(la), Some(lo))
            }
            _ => (None, None),
        };
        let prefix: String = key.chars().take(12).collect();
        let conn = self.lock();
        let existing: Option<(String, Option<f64>, Option<f64>)> = conn
            .query_row(
                "SELECT enrollment, lat, lon FROM nodes WHERE pubkey = ?1",
                params![key],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let is_new = existing.is_none();
        let current = existing
            .as_ref()
            .map(|(e, _, _)| Enrollment::parse(e))
            .unwrap_or(Enrollment::Heard);
        // Never invent HQ from import; never demote HQ.
        let merged = if current == Enrollment::Hq {
            Enrollment::Hq
        } else if enrollment == Enrollment::Hq {
            current
        } else {
            merge_enrollment_rank(current, enrollment)
        };
        let (store_lat, store_lon) = match (lat, lon) {
            (Some(la), Some(lo)) => (Some(la), Some(lo)),
            _ => existing
                .as_ref()
                .map(|(_, a, b)| (*a, *b))
                .unwrap_or((None, None)),
        };
        let heard = last_heard.unwrap_or_else(unix_now);
        let label = if name.trim().is_empty() {
            prefix.clone()
        } else {
            name.trim().chars().take(32).collect()
        };
        // Bulk import is operator-driven — do not flood Roster with NEW pills.
        let noticed = 1i64;
        conn.execute(
            "INSERT INTO nodes (
                pubkey, name, contact_type, flags, path_len, hop_bytes, last_advert,
                lat, lon, lastmod, enrollment, prefix, last_heard, raw_json, noticed
             ) VALUES (?1, ?2, ?3, 0, -1, x'', NULL, ?4, ?5, ?6, ?7, ?8, ?6, '{}', ?9)
             ON CONFLICT(pubkey) DO UPDATE SET
                name = excluded.name,
                contact_type = excluded.contact_type,
                lat = COALESCE(excluded.lat, nodes.lat),
                lon = COALESCE(excluded.lon, nodes.lon),
                enrollment = excluded.enrollment,
                last_heard = MAX(COALESCE(nodes.last_heard, 0), excluded.last_heard),
                prefix = excluded.prefix",
            params![
                key,
                label,
                ty,
                store_lat,
                store_lon,
                heard,
                merged.as_str(),
                prefix,
                noticed
            ],
        )?;
        Ok(if is_new { "new" } else { "updated" })
    }

    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        let conn = self.lock();
        let mut stmt = conn.prepare("SELECT value FROM settings WHERE key = ?1")?;
        Ok(stmt
            .query_row(params![key], |row| row.get(0))
            .optional()?)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.lock().execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    fn channel_last_read_key(slot: i64) -> String {
        format!("channel_last_read_{slot}")
    }

    pub fn channel_last_read(&self, slot: i64) -> Result<Option<i64>> {
        Ok(self
            .setting(&Self::channel_last_read_key(slot))?
            .and_then(|value| value.parse().ok()))
    }

    pub fn set_channel_last_read(&self, slot: i64, message_id: i64) -> Result<()> {
        self.set_setting(&Self::channel_last_read_key(slot), &message_id.to_string())
    }

    /// Mark every message currently in the channel as read.
    pub fn mark_channel_caught_up(&self, slot: i64) -> Result<()> {
        let latest: Option<i64> = self
            .lock()
            .query_row(
                "SELECT id FROM messages
                 WHERE kind = 'channel' AND channel_idx = ?1
                 ORDER BY received_at DESC, id DESC
                 LIMIT 1",
                params![slot],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(id) = latest {
            self.set_channel_last_read(slot, id)?;
        }
        Ok(())
    }

    pub fn mark_dm_caught_up(&self, pubkey: &str) -> Result<()> {
        let latest: Option<i64> = self
            .lock()
            .query_row(
                "SELECT id FROM messages
                 WHERE kind = 'direct' AND sender_pubkey = ?1
                 ORDER BY received_at DESC, id DESC
                 LIMIT 1",
                params![pubkey],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(id) = latest {
            self.set_dm_last_read(pubkey, id)?;
        }
        Ok(())
    }

    pub fn ensure_defaults(&self) -> Result<()> {
        if self.setting("hq_name")?.is_none() {
            self.set_setting("hq_name", "Metis HQ")?;
        }
        if self.setting("retention")?.is_none() {
            self.set_setting("retention", "forever")?;
        }
        if self.setting("retention_days")?.is_none() {
            self.set_setting("retention_days", "30")?;
        }
        if self.setting("message_keep")?.is_none() {
            // Prefer forever; map older day-based retention when present.
            let migrated = match self.setting("retention")?.as_deref() {
                Some("30") => "30d",
                Some("90") | Some("365") => "30d",
                Some("forever") | None => "forever",
                Some("custom") => {
                    let days = self
                        .setting("retention_days")?
                        .and_then(|v| v.parse::<i64>().ok())
                        .unwrap_or(30);
                    if days <= 1 {
                        "24h"
                    } else if days <= 7 {
                        "7d"
                    } else {
                        "30d"
                    }
                }
                Some(_) => "forever",
            };
            self.set_setting("message_keep", migrated)?;
        }
        if self.setting("channel_generation")?.is_none() {
            self.set_setting("channel_generation", "0")?;
        }
        if self.setting("time_format")?.is_none() {
            self.set_setting("time_format", "12h")?;
        }
        if self.setting("date_format")?.is_none() {
            self.set_setting("date_format", "md_short")?;
        }
        if self.setting("svc_weather_enabled")?.is_none() {
            self.set_setting("svc_weather_enabled", "1")?;
        }
        if self.setting("svc_weather_unit")?.is_none() {
            self.set_setting("svc_weather_unit", "F")?;
        }
        if self.setting("svc_nws_enabled")?.is_none() {
            self.set_setting("svc_nws_enabled", "0")?;
        }
        if self.setting("svc_nws_auto_relay")?.is_none() {
            self.set_setting("svc_nws_auto_relay", "0")?;
        }
        if self.setting("svc_quake_enabled")?.is_none() {
            self.set_setting("svc_quake_enabled", "0")?;
        }
        if self.setting("svc_quake_min_mag")?.is_none() {
            self.set_setting("svc_quake_min_mag", "3.0")?;
        }
        if self.setting("channel_info_name")?.is_none() {
            self.set_setting("channel_info_name", CHANNEL_NAME)?;
        }
        if self.setting("channel_net_name")?.is_none() {
            self.set_setting("channel_net_name", NET_CHANNEL_NAME)?;
        }
        if self.setting("svc_weather_hourly")?.is_none() {
            self.set_setting("svc_weather_hourly", "0")?;
        }
        if self.setting("svc_weather_zips")?.is_none() {
            self.set_setting("svc_weather_zips", "")?;
        }
        if self.setting("svc_weather_interval_mins")?.is_none() {
            self.set_setting("svc_weather_interval_mins", "60")?;
        }
        if self.setting("svc_weather_mqtt_enabled")?.is_none() {
            self.set_setting("svc_weather_mqtt_enabled", "0")?;
        }
        if self.setting("svc_weather_mqtt_broker")?.is_none() {
            self.set_setting("svc_weather_mqtt_broker", "")?;
        }
        if self.setting("svc_weather_mqtt_port")?.is_none() {
            self.set_setting("svc_weather_mqtt_port", "1883")?;
        }
        if self.setting("svc_weather_mqtt_tls")?.is_none() {
            self.set_setting("svc_weather_mqtt_tls", "0")?;
        }
        if self.setting("svc_weather_mqtt_username")?.is_none() {
            self.set_setting("svc_weather_mqtt_username", "")?;
        }
        if self.setting("svc_weather_mqtt_topic")?.is_none() {
            self.set_setting("svc_weather_mqtt_topic", "")?;
        }
        if self.setting("svc_weather_mqtt_mode")?.is_none() {
            self.set_setting("svc_weather_mqtt_mode", "json_template")?;
        }
        if self.setting("svc_weather_mqtt_template")?.is_none() {
            self.set_setting("svc_weather_mqtt_template", "")?;
        }
        if self.setting("svc_weather_mqtt_stale_secs")?.is_none() {
            self.set_setting("svc_weather_mqtt_stale_secs", "3600")?;
        }
        if self.setting("svc_weather_mqtt_label")?.is_none() {
            self.set_setting("svc_weather_mqtt_label", "")?;
        }
        if self.setting("svc_weather_station_stale_secs")?.is_none() {
            // Prefer shared stale; seed from MQTT key when present.
            let seed = self
                .setting("svc_weather_mqtt_stale_secs")?
                .unwrap_or_else(|| "3600".into());
            self.set_setting("svc_weather_station_stale_secs", &seed)?;
        }
        if self.setting("svc_weather_ecowitt_push_enabled")?.is_none() {
            self.set_setting("svc_weather_ecowitt_push_enabled", "0")?;
        }
        if self.setting("svc_weather_ecowitt_push_port")?.is_none() {
            self.set_setting("svc_weather_ecowitt_push_port", "4199")?;
        }
        if self.setting("svc_weather_ecowitt_push_path")?.is_none() {
            self.set_setting("svc_weather_ecowitt_push_path", "/data/report")?;
        }
        if self.setting("svc_weather_ecowitt_passkey")?.is_none() {
            self.set_setting("svc_weather_ecowitt_passkey", "")?;
        }
        if self.setting("svc_weather_ecowitt_pull_enabled")?.is_none() {
            self.set_setting("svc_weather_ecowitt_pull_enabled", "0")?;
        }
        if self.setting("svc_weather_ecowitt_pull_host")?.is_none() {
            self.set_setting("svc_weather_ecowitt_pull_host", "")?;
        }
        if self.setting("svc_weather_ecowitt_pull_interval_secs")?.is_none() {
            self.set_setting("svc_weather_ecowitt_pull_interval_secs", "30")?;
        }
        if self.setting("svc_amber_enabled")?.is_none() {
            self.set_setting("svc_amber_enabled", "0")?;
        }
        if self.setting("svc_amber_auto_relay")?.is_none() {
            self.set_setting("svc_amber_auto_relay", "0")?;
        }
        if self.setting("svc_emerg_enabled")?.is_none() {
            self.set_setting("svc_emerg_enabled", "0")?;
        }
        if self.setting("svc_emerg_auto_relay")?.is_none() {
            self.set_setting("svc_emerg_auto_relay", "0")?;
        }
        if self.setting("svc_emerg_state")?.is_none() {
            self.set_setting("svc_emerg_state", "CA")?;
        }
        if self.setting("svc_map_upload_enabled")?.is_none() {
            self.set_setting("svc_map_upload_enabled", "0")?;
        }
        if self.setting("svc_map_upload_authorized")?.is_none() {
            self.set_setting("svc_map_upload_authorized", "0")?;
        }
        if self.setting("svc_map_upload_api_url")?.is_none() {
            self.set_setting(
                "svc_map_upload_api_url",
                "https://map.meshcore.dev/api/v1/uploader/node",
            )?;
        }
        if self.setting("svc_map_upload_interval_secs")?.is_none() {
            self.set_setting("svc_map_upload_interval_secs", "3600")?;
        }
        if self.setting("svc_map_upload_dry_run")?.is_none() {
            self.set_setting("svc_map_upload_dry_run", "0")?;
        }
        if self.setting("backup_schedule")?.is_none() {
            self.set_setting("backup_schedule", "off")?;
        }
        if self.setting("backup_last_at")?.is_none() {
            self.set_setting("backup_last_at", "0")?;
        }
        if self.setting("mesh_regions")?.is_none() {
            self.set_setting("mesh_regions", "[]")?;
        }
        if self.setting("mesh_region_active")?.is_none() {
            self.set_setting("mesh_region_active", "")?;
        }
        if let Some(name) = self.setting("radio_name")? {
            if !name.trim().is_empty() && !self.is_saved_radio(&name)? {
                let _ = self.upsert_radio("ble", name.trim(), "Companion");
            }
        }
        self.ensure_default_channels()?;
        self.repair_duplicate_channel_slots()?;
        // A send still queued when the app closed never reached the radio.
        self.lock().execute(
            "UPDATE messages SET delivery = 'failed'
             WHERE direction = 'tx' AND delivery IN ('queued', 'sending')",
            [],
        )?;
        // Unstick older DBs that never advanced the contact watermark past 0
        // (full BLE contact dumps then block the radio lock and starve sends).
        if self
            .setting("contacts_since")?
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(0)
            == 0
        {
            let nodes = self
                .lock()
                .query_row("SELECT COUNT(*) FROM nodes", [], |row| row.get::<_, i64>(0))
                .unwrap_or(0);
            if nodes > 0 {
                self.set_setting("contacts_since", "1")?;
            }
        }
        Ok(())
    }

    fn ensure_default_channels(&self) -> Result<()> {
        let now = unix_now();
        // Public is always mesh slot 0. Vacate anything else that landed there
        // during a radio sync, then pin Public.
        self.vacate_channel_slot(0, "Public")?;
        self.lock().execute(
            "INSERT INTO channels (name, slot, secret_account, visibility, role, active, created_at)
             VALUES ('Public', 0, 'Public', 'public', 'general', 1, ?1)
             ON CONFLICT(name) DO UPDATE SET
               slot = 0,
               visibility = COALESCE(NULLIF(channels.visibility, ''), 'public'),
               role = COALESCE(NULLIF(channels.role, ''), 'general'),
               active = 1",
            params![now],
        )?;
        let info_name = self.info_channel_name()?;
        let net_name = self.net_channel_name()?;
        let info_slot = self.allocate_system_channel_slot(&info_name, 1)?;
        self.vacate_channel_slot(info_slot, &info_name)?;
        self.lock().execute(
            "INSERT INTO channels (name, slot, secret_account, visibility, role, active, created_at)
             VALUES (?1, ?2, ?1, 'private', 'info', 1, ?3)
             ON CONFLICT(name) DO UPDATE SET
               slot = excluded.slot,
               visibility = 'private',
               role = 'info',
               active = 1",
            params![info_name, info_slot, now],
        )?;
        let net_slot = self.allocate_system_channel_slot(&net_name, 2)?;
        self.vacate_channel_slot(net_slot, &net_name)?;
        self.lock().execute(
            "INSERT INTO channels (name, slot, secret_account, visibility, role, active, created_at)
             VALUES (?1, ?2, ?1, 'private', 'net', 1, ?3)
             ON CONFLICT(name) DO UPDATE SET
               slot = excluded.slot,
               visibility = 'private',
               role = 'net',
               active = 1",
            params![net_name, net_slot, now],
        )?;
        Ok(())
    }

    /// Prefer `prefer` when free and non-zero; otherwise first free private slot.
    fn allocate_system_channel_slot(&self, name: &str, prefer: i64) -> Result<i64> {
        let conn = self.lock();
        let existing: Option<i64> = conn
            .query_row(
                "SELECT slot FROM channels WHERE name = ?1 COLLATE NOCASE",
                params![name],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(slot) = existing.filter(|s| *s != 0) {
            return Ok(slot);
        }
        let taken: Vec<i64> = conn
            .prepare("SELECT slot FROM channels WHERE COALESCE(active, 1) = 1")?
            .query_map([], |row| row.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if prefer != 0 && !taken.contains(&prefer) {
            return Ok(prefer);
        }
        Ok((0..32)
            .find(|slot| *slot != 0 && !taken.contains(slot))
            .unwrap_or(prefer.max(1)))
    }

    pub fn info_channel_name(&self) -> Result<String> {
        self.system_channel_name(SystemChannel::Info)
    }

    pub fn net_channel_name(&self) -> Result<String> {
        self.system_channel_name(SystemChannel::Net)
    }

    fn system_channel_name(&self, which: SystemChannel) -> Result<String> {
        let raw = self
            .setting(which.setting_key())?
            .unwrap_or_else(|| which.default_name().to_string());
        let name = normalize_channel_name(&raw).unwrap_or_else(|| which.default_name().to_string());
        Ok(name)
    }

    pub fn is_info_channel(&self, name: &str) -> bool {
        self.info_channel_name()
            .ok()
            .is_some_and(|n| n.eq_ignore_ascii_case(name.trim()))
    }

    pub fn is_net_channel(&self, name: &str) -> bool {
        self.net_channel_name()
            .ok()
            .is_some_and(|n| n.eq_ignore_ascii_case(name.trim()))
    }

    pub fn is_reserved_private(&self, name: &str) -> bool {
        self.is_info_channel(name) || self.is_net_channel(name)
    }

    /// Move every active channel except `keep_name` off `slot` onto a free slot.
    fn vacate_channel_slot(&self, slot: i64, keep_name: &str) -> Result<()> {
        let conn = self.lock();
        let others: Vec<String> = {
            let mut stmt = conn.prepare(
                "SELECT name FROM channels
                 WHERE slot = ?1
                   AND COALESCE(active, 1) = 1
                   AND name != ?2 COLLATE NOCASE",
            )?;
            let mapped = stmt.query_map(params![slot, keep_name], |row| row.get(0))?;
            mapped.collect::<std::result::Result<Vec<_>, _>>()?
        };
        if others.is_empty() {
            return Ok(());
        }
        let mut used: std::collections::HashSet<i64> = conn
            .prepare("SELECT slot FROM channels WHERE COALESCE(active, 1) = 1")?
            .query_map([], |row| row.get::<_, i64>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .collect();
        // Slot is being claimed; treat it as taken even if only `keep_name` will remain.
        used.insert(slot);
        for name in others {
            let Some(free) = (0..32).find(|candidate| !used.contains(candidate)) else {
                break;
            };
            conn.execute(
                "UPDATE channels SET slot = ?2 WHERE name = ?1",
                params![name, free],
            )?;
            used.insert(free);
        }
        Ok(())
    }

    /// Public on 0, one name per slot. Radio sync used to leave METIS-INFO and
    /// Public both on slot 1, which made the UI open the emergency channel when
    /// the operator thought they clicked Public.
    fn repair_duplicate_channel_slots(&self) -> Result<()> {
        // Pin reserved channels first.
        self.vacate_channel_slot(0, "Public")?;
        self.lock().execute(
            "UPDATE channels SET slot = 0 WHERE name = 'Public' COLLATE NOCASE",
            [],
        )?;
        let info_name = self.info_channel_name()?;
        let net_name = self.net_channel_name()?;
        for reserved in [&info_name, &net_name] {
            let current: Option<i64> = self
                .lock()
                .query_row(
                    "SELECT slot FROM channels
                     WHERE name = ?1 COLLATE NOCASE AND COALESCE(active, 1) = 1",
                    params![reserved],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(current) = current {
                let slot = if current == 0 { 1 } else { current };
                self.vacate_channel_slot(slot, reserved)?;
                self.lock().execute(
                    "UPDATE channels SET slot = ?2 WHERE name = ?1 COLLATE NOCASE",
                    params![reserved, slot],
                )?;
            }
        }

        let rows = self.list_channels()?;
        let mut by_slot: std::collections::BTreeMap<i64, Vec<String>> =
            std::collections::BTreeMap::new();
        for row in &rows {
            by_slot.entry(row.slot).or_default().push(row.name.clone());
        }
        let mut used: std::collections::HashSet<i64> = rows.iter().map(|r| r.slot).collect();
        for (_slot, names) in by_slot {
            if names.len() < 2 {
                continue;
            }
            let keep = if names.iter().any(|n| n.eq_ignore_ascii_case("Public")) {
                "Public".to_string()
            } else if names.iter().any(|n| n.eq_ignore_ascii_case(&info_name)) {
                info_name.clone()
            } else if names.iter().any(|n| n.eq_ignore_ascii_case(&net_name)) {
                net_name.clone()
            } else {
                names[0].clone()
            };
            for name in names {
                if name.eq_ignore_ascii_case(&keep) {
                    continue;
                }
                let Some(free) = (0..32).find(|candidate| !used.contains(candidate)) else {
                    break;
                };
                self.lock().execute(
                    "UPDATE channels SET slot = ?2 WHERE name = ?1",
                    params![name, free],
                )?;
                used.insert(free);
            }
        }
        Ok(())
    }

    /// How long to keep channel/DM messages: `1h` | `6h` | `24h` | `7d` | `30d` | `forever`.
    pub fn message_keep(&self) -> Result<String> {
        Ok(self
            .setting("message_keep")?
            .unwrap_or_else(|| "forever".into()))
    }

    pub fn set_message_keep(&self, keep: &str) -> Result<()> {
        let normalized = match keep {
            "1h" | "6h" | "24h" | "7d" | "30d" | "forever" => keep,
            _ => "forever",
        };
        self.set_setting("message_keep", normalized)?;
        // Keep legacy Settings fields roughly in sync for older UI paths.
        match normalized {
            "forever" => {
                self.set_setting("retention", "forever")?;
            }
            "30d" => {
                self.set_setting("retention", "30")?;
                self.set_setting("retention_days", "30")?;
            }
            "7d" => {
                self.set_setting("retention", "custom")?;
                self.set_setting("retention_days", "7")?;
            }
            "24h" | "6h" | "1h" => {
                self.set_setting("retention", "custom")?;
                self.set_setting("retention_days", "1")?;
            }
            _ => {}
        }
        Ok(())
    }

    pub fn message_keep_seconds(keep: &str) -> Option<i64> {
        match keep {
            "1h" => Some(60 * 60),
            "6h" => Some(6 * 60 * 60),
            "24h" => Some(24 * 60 * 60),
            "7d" => Some(7 * 24 * 60 * 60),
            "30d" => Some(30 * 24 * 60 * 60),
            _ => None,
        }
    }

    pub fn message_keep_cutoff(keep: &str) -> Option<i64> {
        Self::message_keep_seconds(keep).map(|secs| unix_now() - secs)
    }

    /// Normalize a keep token; returns `None` for "use global default".
    pub fn normalize_message_keep(keep: &str) -> Option<&'static str> {
        match keep {
            "1h" => Some("1h"),
            "6h" => Some("6h"),
            "24h" => Some("24h"),
            "7d" => Some("7d"),
            "30d" => Some("30d"),
            "forever" => Some("forever"),
            _ => None,
        }
    }

    pub fn set_channel_message_keep(&self, name: &str, keep: Option<&str>) -> Result<()> {
        let value = keep.and_then(Self::normalize_message_keep);
        self.lock().execute(
            "UPDATE channels SET message_keep = ?2 WHERE name = ?1",
            params![name, value],
        )?;
        Ok(())
    }

    /// Delete messages older than each channel's effective keep (override or global).
    /// Non-channel messages (DMs) use the global default only.
    pub fn apply_message_keep(&self) -> Result<u64> {
        let global = self.message_keep()?;
        let conn = self.lock();
        let mut channel_keeps: Vec<(i64, Option<String>)> = {
            let mut stmt = conn.prepare(
                "SELECT slot, NULLIF(TRIM(COALESCE(message_keep, '')), '') FROM channels",
            )?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        };
        let mut deleted: u64 = 0;

        for (slot, keep_raw) in channel_keeps.drain(..) {
            let effective = keep_raw
                .as_deref()
                .and_then(Self::normalize_message_keep)
                .unwrap_or(global.as_str());
            let Some(cutoff) = Self::message_keep_cutoff(effective) else {
                continue;
            };
            deleted += conn.execute(
                "DELETE FROM messages
                 WHERE kind = 'channel' AND channel_idx = ?1 AND received_at < ?2",
                params![slot, cutoff],
            )? as u64;
        }

        if let Some(cutoff) = Self::message_keep_cutoff(&global) {
            // Orphan channel_idx rows plus DMs / other kinds.
            deleted += conn.execute(
                "DELETE FROM messages
                 WHERE received_at < ?1
                   AND (
                     kind IS NULL OR kind != 'channel' OR channel_idx IS NULL
                     OR channel_idx NOT IN (SELECT slot FROM channels)
                   )",
                params![cutoff],
            )? as u64;
        }

        if deleted > 0 {
            conn.execute(
                "DELETE FROM paths WHERE message_id IS NOT NULL AND message_id NOT IN (SELECT id FROM messages)",
                [],
            )?;
        }
        Ok(deleted)
    }

    pub fn upsert_radio(&self, transport: &str, address: &str, label: &str) -> Result<i64> {
        self.upsert_radio_with_role(transport, address, label, RadioRole::Companion)
    }

    pub fn upsert_radio_with_role(
        &self,
        transport: &str,
        address: &str,
        label: &str,
        role: RadioRole,
    ) -> Result<i64> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO radios (transport, address, label, role, last_error, last_connected)
             VALUES (?1, ?2, ?3, ?4, '', NULL)
             ON CONFLICT(transport, address) DO UPDATE SET
               label = CASE
                 WHEN excluded.label = '' OR excluded.label = 'Companion' THEN radios.label
                 ELSE excluded.label
               END",
            params![transport, address, label, role.as_str()],
        )?;
        let id = conn.query_row(
            "SELECT id FROM radios WHERE transport = ?1 AND address = ?2",
            params![transport, address],
            |row| row.get(0),
        )?;
        Ok(id)
    }

    pub fn remember_radio(&self, transport: &str, address: &str, label: &str) -> Result<i64> {
        self.remember_radio_with_identity(transport, address, label, "")
    }

    /// Save / refresh a radio row. `usb_identity` is stored for USB sticky rematch; pass `""` for BLE/TCP.
    pub fn remember_radio_with_identity(
        &self,
        transport: &str,
        address: &str,
        label: &str,
        usb_identity: &str,
    ) -> Result<i64> {
        let id = self.upsert_radio(transport, address, label)?;
        let identity = usb_identity.trim();
        self.lock().execute(
            "UPDATE radios SET last_connected = ?2, last_error = '',
               label = CASE
                 WHEN ?3 = '' OR ?3 = 'Companion' THEN label
                 ELSE ?3
               END,
               usb_identity = CASE
                 WHEN ?4 = '' THEN usb_identity
                 ELSE ?4
               END
             WHERE id = ?1",
            params![id, unix_now(), label, identity],
        )?;
        // Keep last-used address for header picker fallback; primary_radio_id is authoritative.
        self.set_setting("radio_name", address)?;
        Ok(id)
    }

    pub fn set_radio_usb_identity(&self, id: i64, identity: &str) -> Result<()> {
        let changed = self.lock().execute(
            "UPDATE radios SET usb_identity = ?2 WHERE id = ?1",
            params![id, identity.trim()],
        )?;
        if changed == 0 {
            return Err(Error::Msg("Radio not found.".into()));
        }
        Ok(())
    }

    pub fn radio_usb_identity(&self, id: i64) -> Result<String> {
        let raw: String = self.lock().query_row(
            "SELECT COALESCE(usb_identity, '') FROM radios WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )?;
        Ok(raw)
    }

    pub fn radio_address(&self, id: i64) -> Result<String> {
        let raw: String = self.lock().query_row(
            "SELECT address FROM radios WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )?;
        Ok(raw)
    }

    /// Rewrite the open path for a saved radio without changing its id (F4 radio_id stays stable).
    ///
    /// If another USB row already owns `new_address`, that duplicate is forgotten and this id wins.
    pub fn remap_radio_address(&self, id: i64, new_address: &str) -> Result<()> {
        let new_address = new_address.trim();
        if new_address.is_empty() {
            return Err(Error::Msg("USB address is empty.".into()));
        }
        let (transport, old_address): (String, String) = {
            let conn = self.lock();
            conn.query_row(
                "SELECT transport, address FROM radios WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|_| Error::Msg("Radio not found.".into()))?
        };
        if old_address == new_address {
            return Ok(());
        }
        let conflict: Option<i64> = {
            let conn = self.lock();
            conn.query_row(
                "SELECT id FROM radios WHERE transport = ?1 AND address = ?2 AND id != ?3",
                params![transport, new_address, id],
                |row| row.get(0),
            )
            .optional()?
        };
        if let Some(dup_id) = conflict {
            // Fold heard-on rows onto the live slot before dropping the duplicate.
            // Do not call forget_radio: that would clear radio_name when it already
            // matches new_address (the address we are about to claim).
            {
                let conn = self.lock();
                let _ = conn.execute(
                    "INSERT INTO node_radios (pubkey, radio_id, last_heard)
                     SELECT pubkey, ?1, last_heard FROM node_radios WHERE radio_id = ?2
                     ON CONFLICT(pubkey, radio_id) DO UPDATE SET
                       last_heard = CASE
                         WHEN excluded.last_heard > node_radios.last_heard THEN excluded.last_heard
                         ELSE node_radios.last_heard
                       END",
                    params![id, dup_id],
                );
                let _ = conn.execute("DELETE FROM node_radios WHERE radio_id = ?1", params![dup_id]);
                let _ = conn.execute(
                    "UPDATE nodes SET last_radio_id = ?1 WHERE last_radio_id = ?2",
                    params![id, dup_id],
                );
                conn.execute("DELETE FROM radios WHERE id = ?1", params![dup_id])?;
            }
            if self.primary_radio_id()? == Some(dup_id) {
                if self.radio_role(id)? == RadioRole::Companion {
                    let _ = self.set_primary_radio_id(id);
                } else {
                    let _ = self.clear_primary_radio_id();
                }
            }
        }
        {
            let conn = self.lock();
            conn.execute(
                "UPDATE radios SET address = ?2 WHERE id = ?1",
                params![id, new_address],
            )?;
        }
        // Keep the last-used picker in sync when this was the remembered address.
        if self
            .setting("radio_name")?
            .is_some_and(|name| {
                name.is_empty()
                    || name.eq_ignore_ascii_case(&old_address)
                    || name.eq_ignore_ascii_case(new_address)
            })
        {
            self.set_setting("radio_name", new_address)?;
        }
        Ok(())
    }

    pub fn set_radio_role(&self, id: i64, role: RadioRole) -> Result<()> {
        let changed = self.lock().execute(
            "UPDATE radios SET role = ?2 WHERE id = ?1",
            params![id, role.as_str()],
        )?;
        if changed == 0 {
            return Err(Error::Msg("Radio not found.".into()));
        }
        if role != RadioRole::Companion {
            if self.primary_radio_id()? == Some(id) {
                self.clear_primary_radio_id()?;
            }
        }
        Ok(())
    }

    pub fn radio_role(&self, id: i64) -> Result<RadioRole> {
        let raw: String = self.lock().query_row(
            "SELECT COALESCE(role, 'companion') FROM radios WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )?;
        Ok(RadioRole::parse(&raw))
    }

    pub fn primary_radio_id(&self) -> Result<Option<i64>> {
        Ok(self
            .setting("primary_radio_id")?
            .and_then(|v| v.parse::<i64>().ok())
            .filter(|id| *id > 0))
    }

    pub fn set_primary_radio_id(&self, id: i64) -> Result<()> {
        let role = self.radio_role(id)?;
        if role != RadioRole::Companion {
            return Err(Error::Msg(
                "Only a companion radio can be the primary HQ radio.".into(),
            ));
        }
        self.set_setting("primary_radio_id", &id.to_string())
    }

    pub fn clear_primary_radio_id(&self) -> Result<()> {
        self.set_setting("primary_radio_id", "")
    }

    /// Remove a saved radio from the pool. Clears primary / last-used if they pointed here.
    pub fn forget_radio(&self, id: i64) -> Result<()> {
        let row = {
            let conn = self.lock();
            conn.query_row(
                "SELECT address FROM radios WHERE id = ?1",
                params![id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        };
        let Some(address) = row else {
            return Err(Error::Msg("Radio not found.".into()));
        };
        if self.primary_radio_id()? == Some(id) {
            self.clear_primary_radio_id()?;
        }
        if self
            .setting("radio_name")?
            .is_some_and(|name| name.eq_ignore_ascii_case(&address))
        {
            self.set_setting("radio_name", "")?;
        }
        let changed = self
            .lock()
            .execute("DELETE FROM radios WHERE id = ?1", params![id])?;
        if changed == 0 {
            return Err(Error::Msg("Radio not found.".into()));
        }
        Ok(())
    }

    pub fn list_saved_radios(&self) -> Result<Vec<RadioRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, transport, address, label, COALESCE(role, 'companion'), last_error,
                    last_connected, COALESCE(usb_identity, '')
             FROM radios
             ORDER BY COALESCE(last_connected, 0) DESC, id DESC",
        )?;
        let rows = stmt
            .query_map([], |row| {
                let role_raw: String = row.get(4)?;
                Ok(RadioRow {
                    id: row.get(0)?,
                    transport: row.get(1)?,
                    address: row.get(2)?,
                    label: row.get(3)?,
                    role: RadioRole::parse(&role_raw),
                    last_error: row.get(5)?,
                    last_connected: row.get(6)?,
                    usb_identity: row.get(7)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn is_saved_radio(&self, address_or_name: &str) -> Result<bool> {
        let needle = address_or_name.trim();
        if needle.is_empty() {
            return Ok(false);
        }
        let conn = self.lock();
        let found: Option<i64> = conn
            .query_row(
                "SELECT id FROM radios
                 WHERE lower(address) = lower(?1) OR lower(label) = lower(?1)
                 LIMIT 1",
                params![needle],
                |row| row.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    pub fn set_radio_error(&self, id: i64, error: &str) -> Result<()> {
        self.lock().execute(
            "UPDATE radios SET last_error = ?2 WHERE id = ?1",
            params![id, error],
        )?;
        Ok(())
    }

    pub fn save_device_info(&self, id: i64, info: &Value) -> Result<()> {
        self.lock().execute(
            "UPDATE radios SET device_json = ?2, last_error = '' WHERE id = ?1",
            params![id, info.to_string()],
        )?;
        Ok(())
    }

    pub fn device_info_for_address(&self, address: &str) -> Result<Option<Value>> {
        let conn = self.lock();
        let raw: Option<String> = conn
            .query_row(
                "SELECT device_json FROM radios
                 WHERE lower(address) = lower(?1) OR lower(label) = lower(?1)
                 ORDER BY COALESCE(last_connected, 0) DESC
                 LIMIT 1",
                params![address.trim()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(raw.and_then(|text| serde_json::from_str(&text).ok()))
    }

    pub fn list_channels(&self) -> Result<Vec<ChannelRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT c.name, c.slot,
                    COALESCE(NULLIF(c.visibility, ''), 'private'),
                    COALESCE(NULLIF(c.role, ''), 'general'),
                    COALESCE(c.active, 1),
                    (SELECT COUNT(*) FROM messages m
                     WHERE m.kind = 'channel' AND m.channel_idx = c.slot),
                    CASE
                      WHEN EXISTS (
                        SELECT 1 FROM settings s
                        WHERE s.key = 'channel_last_read_' || c.slot
                      ) THEN (
                        SELECT COUNT(*) FROM messages m
                        WHERE m.kind = 'channel'
                          AND m.channel_idx = c.slot
                          AND m.direction = 'rx'
                          AND m.id > COALESCE(
                            (SELECT CAST(s.value AS INTEGER) FROM settings s
                             WHERE s.key = 'channel_last_read_' || c.slot),
                            0
                          )
                      )
                      ELSE 0
                    END,
                    CASE
                      WHEN EXISTS (
                        SELECT 1 FROM settings s
                        WHERE s.key = 'channel_last_read_' || c.slot
                      ) THEN (
                        SELECT COUNT(*) FROM messages m
                        WHERE m.kind = 'channel'
                          AND m.channel_idx = c.slot
                          AND m.direction = 'rx'
                          AND COALESCE(m.mentions_hq, 0) = 1
                          AND m.id > COALESCE(
                            (SELECT CAST(s.value AS INTEGER) FROM settings s
                             WHERE s.key = 'channel_last_read_' || c.slot),
                            0
                          )
                      )
                      ELSE 0
                    END,
                    (SELECT m.received_at FROM messages m
                     WHERE m.kind = 'channel' AND m.channel_idx = c.slot
                     ORDER BY m.received_at DESC, m.id DESC LIMIT 1),
                    (SELECT m.display_text FROM messages m
                     WHERE m.kind = 'channel' AND m.channel_idx = c.slot
                     ORDER BY m.received_at DESC, m.id DESC LIMIT 1),
                    NULLIF(TRIM(COALESCE(c.message_keep, '')), ''),
                    NULLIF(TRIM(COALESCE(c.appearance_json, '')), '')
             FROM channels c
             WHERE COALESCE(c.active, 1) = 1
             ORDER BY
               CASE
                 WHEN c.name = 'Public' COLLATE NOCASE THEN 0
                 WHEN lower(COALESCE(c.role, '')) = 'info' THEN 1
                 WHEN lower(COALESCE(c.role, '')) = 'net' THEN 2
                 WHEN lower(COALESCE(c.role, '')) = 'emergency' THEN 1
                 ELSE 3
               END,
               c.name COLLATE NOCASE",
        )?;
        let rows = stmt
            .query_map([], |row| {
                let keep_raw: Option<String> = row.get(10)?;
                let message_keep = keep_raw.and_then(|v| {
                    Self::normalize_message_keep(&v).map(str::to_string)
                });
                let appearance_raw: Option<String> = row.get(11)?;
                Ok(ChannelRow {
                    name: row.get(0)?,
                    slot: row.get(1)?,
                    visibility: row.get(2)?,
                    role: row.get(3)?,
                    active: {
                        let value: i64 = row.get(4)?;
                        value != 0
                    },
                    message_count: row.get(5)?,
                    unread_count: row.get(6)?,
                    unread_mentions: row.get(7)?,
                    last_message_at: row.get(8)?,
                    last_text: row.get(9)?,
                    message_keep,
                    has_appearance_override: appearance_raw.is_some(),
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn messages_appearance(&self) -> Result<MessagesAppearance> {
        Ok(self
            .setting(APPEARANCE_SETTING)?
            .map(|raw| parse_appearance_json(&raw))
            .unwrap_or_default())
    }

    pub fn set_messages_appearance(&self, appearance: &MessagesAppearance) -> Result<()> {
        let clamped = appearance.clone().clamp();
        let json = serde_json::to_string(&clamped).map_err(|e| Error::Msg(e.to_string()))?;
        self.set_setting(APPEARANCE_SETTING, &json)
    }

    pub fn channel_appearance(&self, name: &str) -> Result<Option<MessagesAppearance>> {
        let conn = self.lock();
        let raw: Option<Option<String>> = conn
            .query_row(
                "SELECT NULLIF(TRIM(COALESCE(appearance_json, '')), '') FROM channels WHERE name = ?1",
                params![name],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?;
        Ok(raw.flatten().map(|text| parse_appearance_json(&text)))
    }

    pub fn set_channel_appearance(
        &self,
        name: &str,
        appearance: Option<&MessagesAppearance>,
    ) -> Result<()> {
        let value = match appearance {
            Some(a) => {
                let json =
                    serde_json::to_string(&a.clone().clamp()).map_err(|e| Error::Msg(e.to_string()))?;
                Some(json)
            }
            None => None,
        };
        let changed = self.lock().execute(
            "UPDATE channels SET appearance_json = ?2 WHERE name = ?1",
            params![name, value],
        )?;
        if changed == 0 {
            return Err(Error::Msg(format!("Channel '{name}' not found.")));
        }
        Ok(())
    }

    /// Channel override when set; otherwise global Messages appearance.
    pub fn resolved_appearance(&self, channel_name: &str) -> Result<MessagesAppearance> {
        if let Some(over) = self.channel_appearance(channel_name)? {
            return Ok(over);
        }
        self.messages_appearance()
    }

    pub fn ensure_channel_row(
        &self,
        name: &str,
        slot: i64,
        visibility: &str,
        role: &str,
    ) -> Result<()> {
        // Public is immovable from mesh slot 0; reserved private stay off 0.
        let slot = if name.eq_ignore_ascii_case("Public") {
            0
        } else if self.is_reserved_private(name) && slot == 0 {
            1
        } else {
            slot
        };
        self.vacate_channel_slot(slot, name)?;
        let now = unix_now();
        self.lock().execute(
            "INSERT INTO channels (name, slot, secret_account, visibility, role, active, created_at)
             VALUES (?1, ?2, ?1, ?3, ?4, 1, ?5)
             ON CONFLICT(name) DO UPDATE SET
               slot = excluded.slot,
               visibility = excluded.visibility,
               role = CASE
                 WHEN channels.role = 'emergency' THEN channels.role
                 ELSE excluded.role
               END,
               active = 1",
            params![name, slot, visibility, role, now],
        )?;
        Ok(())
    }

    pub fn next_channel_slot(&self) -> Result<i64> {
        let conn = self.lock();
        let used: Vec<i64> = conn
            .prepare("SELECT slot FROM channels WHERE COALESCE(active, 1) = 1")?
            .query_map([], |row| row.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        for slot in 0..32 {
            if !used.contains(&slot) {
                return Ok(slot);
            }
        }
        Err(Error::Msg("No free channel slots (0–31).".into()))
    }

    pub fn create_channel(
        &self,
        name: &str,
        visibility: &str,
        secrets: &Secrets,
    ) -> Result<ChannelRow> {
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::Msg("Channel name is required.".into()));
        }
        if name.eq_ignore_ascii_case("Public") || self.is_reserved_private(name) {
            return Err(Error::Msg("That channel name is reserved.".into()));
        }
        if name.len() > MAX_CHANNEL_NAME_BYTES {
            return Err(Error::Msg(format!(
                "Channel name is limited to {MAX_CHANNEL_NAME_BYTES} characters."
            )));
        }
        let visibility = if visibility == "public" {
            "public"
        } else {
            "private"
        };
        let slot = self.next_channel_slot()?;
        if visibility == "private" {
            let secret = Secrets::generate();
            let kind = secrets.store(name, &secret).map_err(Error::Msg)?;
            self.set_setting("secret_store", kind.as_str())?;
        }
        self.ensure_channel_row(name, slot, visibility, "general")?;
        self.list_channels()?
            .into_iter()
            .find(|row| row.name == name)
            .ok_or_else(|| Error::Msg("Channel was created but could not be reloaded.".into()))
    }

    pub fn set_channel_visibility(&self, name: &str, visibility: &str) -> Result<()> {
        if name.eq_ignore_ascii_case("Public") {
            return Err(Error::Msg("Public is the default channel and stays public.".into()));
        }
        if self.is_reserved_private(name) {
            return Err(Error::Msg(format!(
                "{} stays private.",
                if self.is_info_channel(name) {
                    self.info_channel_name().unwrap_or_else(|_| CHANNEL_NAME.into())
                } else {
                    self.net_channel_name()
                        .unwrap_or_else(|_| NET_CHANNEL_NAME.into())
                }
            )));
        }
        let visibility = if visibility == "public" {
            "public"
        } else {
            "private"
        };
        let changed = self.lock().execute(
            "UPDATE channels SET visibility = ?2 WHERE name = ?1 AND COALESCE(active, 1) = 1",
            params![name, visibility],
        )?;
        if changed == 0 {
            return Err(Error::Msg("Channel not found.".into()));
        }
        Ok(())
    }

    pub fn soft_delete_channel(&self, name: &str) -> Result<i64> {
        if name.eq_ignore_ascii_case("Public") || self.is_reserved_private(name) {
            return Err(Error::Msg("System channels cannot be deleted.".into()));
        }
        let conn = self.lock();
        let slot: i64 = conn.query_row(
            "SELECT slot FROM channels WHERE name = ?1 AND COALESCE(active, 1) = 1",
            params![name],
            |row| row.get(0),
        )?;
        conn.execute(
            "UPDATE channels SET active = 0 WHERE name = ?1",
            params![name],
        )?;
        Ok(slot)
    }

    /// True when the operator deleted this channel in HQ (active=0).
    pub fn is_channel_soft_deleted(&self, name: &str) -> Result<bool> {
        let conn = self.lock();
        let active: Option<i64> = conn
            .query_row(
                "SELECT COALESCE(active, 1) FROM channels WHERE name = ?1 COLLATE NOCASE",
                params![name],
                |row| row.get(0),
            )
            .optional()?;
        Ok(matches!(active, Some(0)))
    }

    /// Soft-deleted channels and the slots they last occupied — used to wipe
    /// companion slots so a reconnect sync cannot resurrect them.
    pub fn list_soft_deleted_channels(&self) -> Result<Vec<(i64, String)>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT slot, name FROM channels
             WHERE COALESCE(active, 1) = 0
             ORDER BY slot ASC",
        )?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Slot currently claimed by an active channel, if any.
    pub fn active_channel_on_slot(&self, slot: i64) -> Result<Option<String>> {
        let conn = self.lock();
        conn.query_row(
            "SELECT name FROM channels
             WHERE slot = ?1 AND COALESCE(active, 1) = 1
             LIMIT 1",
            params![slot],
            |row| row.get(0),
        )
        .optional()
        .map_err(Error::from)
    }

    /// Import a channel discovered on the companion radio without clobbering
    /// the HQ bot-home / peer-net private channels.
    /// Returns `Ok(false)` when the operator soft-deleted this name — caller should
    /// clear the radio slot instead of resurrecting the row.
    pub fn upsert_channel_from_radio(
        &self,
        name: &str,
        slot: i64,
        secret: &[u8; 16],
        secrets: &Secrets,
    ) -> Result<bool> {
        let name = name.trim();
        if name.is_empty() {
            return Ok(true);
        }
        if self.is_channel_soft_deleted(name)? {
            return Ok(false);
        }
        if self.is_info_channel(name) {
            let info = self.info_channel_name()?;
            self.ensure_channel_row(&info, slot, "private", "info")?;
            return Ok(true);
        }
        if self.is_net_channel(name) {
            let net = self.net_channel_name()?;
            self.ensure_channel_row(&net, slot, "private", "net")?;
            return Ok(true);
        }
        let visibility = if name.eq_ignore_ascii_case("Public") || secret.iter().all(|b| *b == 0) {
            "public"
        } else {
            "private"
        };
        if visibility == "private" && secrets.load(name).map_err(Error::Msg)?.is_none() {
            let kind = secrets.store(name, secret).map_err(Error::Msg)?;
            self.set_setting("secret_store", kind.as_str())?;
        }
        let role = if name.eq_ignore_ascii_case("Public") {
            "general"
        } else {
            "general"
        };
        self.ensure_channel_row(name, slot, visibility, role)?;
        Ok(true)
    }

    /// Newest `limit` channel messages, ordered oldest → newest for the chat pane.
    /// Second value is true when older history exists beyond this page.
    pub fn list_channel_messages(
        &self,
        slot: i64,
        limit: i64,
    ) -> Result<(Vec<MessageRow>, bool)> {
        self.list_channel_messages_page(slot, None, limit)
    }

    /// Older channel messages strictly before `before_id` (newest-of-older page), ASC.
    pub fn list_channel_messages_before(
        &self,
        slot: i64,
        before_id: i64,
        limit: i64,
    ) -> Result<(Vec<MessageRow>, bool)> {
        self.list_channel_messages_page(slot, Some(before_id), limit)
    }

    fn list_channel_messages_page(
        &self,
        slot: i64,
        before_id: Option<i64>,
        limit: i64,
    ) -> Result<(Vec<MessageRow>, bool)> {
        let limit = limit.max(0);
        let fetch = limit + 1;
        let conn = self.lock();
        let sql = if before_id.is_some() {
            "SELECT id, direction, kind, sender_name, sender_pubkey, channel_idx, hop_count,
                    display_text, delivery, received_at, route_source, path, hash_size, snr
             FROM (
               SELECT m.id, m.direction, m.kind,
                      COALESCE(n.name, m.sender_prefix, 'channel') AS sender_name,
                      m.sender_pubkey, m.channel_idx, m.hop_count, m.display_text, m.delivery,
                      m.received_at, p.source AS route_source, COALESCE(p.path, x'') AS path,
                      COALESCE(p.hash_size, 1) AS hash_size, m.snr
               FROM messages m
               LEFT JOIN nodes n ON n.pubkey = m.sender_pubkey
               LEFT JOIN paths p ON p.id = (
                  SELECT id FROM paths WHERE message_id = m.id ORDER BY id DESC LIMIT 1
               )
               WHERE m.kind = 'channel' AND m.channel_idx = ?1 AND m.id < ?2
               ORDER BY m.received_at DESC, m.id DESC
               LIMIT ?3
             )
             ORDER BY received_at ASC, id ASC"
        } else {
            "SELECT id, direction, kind, sender_name, sender_pubkey, channel_idx, hop_count,
                    display_text, delivery, received_at, route_source, path, hash_size, snr
             FROM (
               SELECT m.id, m.direction, m.kind,
                      COALESCE(n.name, m.sender_prefix, 'channel') AS sender_name,
                      m.sender_pubkey, m.channel_idx, m.hop_count, m.display_text, m.delivery,
                      m.received_at, p.source AS route_source, COALESCE(p.path, x'') AS path,
                      COALESCE(p.hash_size, 1) AS hash_size, m.snr
               FROM messages m
               LEFT JOIN nodes n ON n.pubkey = m.sender_pubkey
               LEFT JOIN paths p ON p.id = (
                  SELECT id FROM paths WHERE message_id = m.id ORDER BY id DESC LIMIT 1
               )
               WHERE m.kind = 'channel' AND m.channel_idx = ?1
               ORDER BY m.received_at DESC, m.id DESC
               LIMIT ?2
             )
             ORDER BY received_at ASC, id ASC"
        };
        let mut stmt = conn.prepare(sql)?;
        let rows = if let Some(before) = before_id {
            stmt.query_map(params![slot, before, fetch], Self::map_message_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            stmt.query_map(params![slot, fetch], Self::map_message_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        Ok(trim_message_page(rows, limit))
    }

    /// Upsert a radio contact. When `quiet` is true (bootstrap contact sync), new
    /// rows stay noticed so Roster is not flooded. Live discover/advert paths pass
    /// `quiet = false` so NEW pills appear until the operator opens the node.
    pub fn upsert_contact(
        &self,
        pubkey: &str,
        name: &str,
        contact_type: u8,
        flags: u8,
        path_len: i64,
        hop_bytes: &[u8],
        last_advert: i64,
        lat_micro: i32,
        lon_micro: i32,
        lastmod: i64,
        raw: &Value,
        quiet: bool,
        radio_id: Option<i64>,
    ) -> Result<()> {
        let now = unix_now();
        let (lat, lon) = coords_from_micro(lat_micro, lon_micro);
        let prefix = pubkey.chars().take(12).collect::<String>();
        let conn = self.lock();
        let existing: Option<(String, Option<f64>, Option<f64>)> = conn
            .query_row(
                "SELECT enrollment, lat, lon FROM nodes WHERE pubkey = ?1",
                params![pubkey],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let is_new = existing.is_none();
        let enrollment = existing
            .as_ref()
            .map(|(value, _, _)| value.clone())
            .unwrap_or_else(|| "heard".into());
        let (store_lat, store_lon) = match (lat, lon) {
            (Some(lat), Some(lon)) => (Some(lat), Some(lon)),
            _ => existing
                .as_ref()
                .map(|(_, old_lat, old_lon)| (*old_lat, *old_lon))
                .unwrap_or((None, None)),
        };
        let noticed = if is_new && !quiet { 0i64 } else { 1i64 };
        conn.execute(
            "INSERT INTO nodes (
                pubkey, name, contact_type, flags, path_len, hop_bytes, last_advert,
                lat, lon, lastmod, enrollment, prefix, last_heard, raw_json, noticed,
                last_radio_id
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
             ON CONFLICT(pubkey) DO UPDATE SET
                name = excluded.name,
                contact_type = excluded.contact_type,
                flags = excluded.flags,
                path_len = excluded.path_len,
                hop_bytes = excluded.hop_bytes,
                last_advert = excluded.last_advert,
                lat = excluded.lat,
                lon = excluded.lon,
                lastmod = excluded.lastmod,
                prefix = excluded.prefix,
                last_heard = excluded.last_heard,
                raw_json = excluded.raw_json,
                last_radio_id = COALESCE(excluded.last_radio_id, nodes.last_radio_id)",
            params![
                pubkey,
                name,
                contact_type,
                flags,
                path_len,
                hop_bytes,
                last_advert,
                store_lat,
                store_lon,
                lastmod,
                enrollment,
                prefix,
                now,
                raw.to_string(),
                noticed,
                radio_id,
            ],
        )?;
        if let Some(rid) = radio_id {
            conn.execute(
                "INSERT INTO node_radios (pubkey, radio_id, last_heard)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(pubkey, radio_id) DO UPDATE SET last_heard = excluded.last_heard",
                params![pubkey, rid, now],
            )?;
        }
        if let (Some(lat), Some(lon)) = (lat, lon) {
            let changed = existing
                .as_ref()
                .and_then(|(_, old_lat, old_lon)| match (old_lat, old_lon) {
                    (Some(old_lat), Some(old_lon)) => Some((*old_lat, *old_lon) != (lat, lon)),
                    _ => Some(true),
                })
                .unwrap_or(true);
            if changed {
                conn.execute(
                    "INSERT INTO positions (pubkey, lat, lon, source, at) VALUES (?1, ?2, ?3, 'advert', ?4)",
                    params![pubkey, lat, lon, now],
                )?;
            }
        }
        drop(conn);
        let _ = self.promote_name_block_to_pubkey(pubkey, name);
        Ok(())
    }

    pub fn save_hq(&self, pubkey: &str, name: &str, lat_micro: i32, lon_micro: i32, raw: &Value) -> Result<()> {
        let (lat, lon) = coords_from_micro(lat_micro, lon_micro);
        let prefix = pubkey.chars().take(12).collect::<String>();
        self.lock().execute(
            "INSERT INTO nodes (
                pubkey, name, contact_type, flags, path_len, hop_bytes, last_advert,
                lat, lon, lastmod, enrollment, prefix, last_heard, raw_json, noticed
             ) VALUES (?1, ?2, 1, 0, -1, x'', ?3, ?4, ?5, ?3, 'hq', ?6, ?3, ?7, 1)
             ON CONFLICT(pubkey) DO UPDATE SET
                name = excluded.name,
                lat = COALESCE(excluded.lat, nodes.lat),
                lon = COALESCE(excluded.lon, nodes.lon),
                enrollment = 'hq',
                last_heard = excluded.last_heard,
                raw_json = excluded.raw_json",
            params![pubkey, name, unix_now(), lat, lon, prefix, raw.to_string()],
        )?;
        Ok(())
    }

    pub fn nodes_by_prefix(&self, prefix_hex: &str) -> Result<Vec<NodeRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT pubkey, name, requested_name, contact_type, enrollment, last_heard,
                    last_advert, lat, lon, prefix, hop_bytes, path_len, qr_file, qr_uri,
                    COALESCE(noticed, 1), last_radio_id
             FROM nodes WHERE pubkey LIKE ?1 || '%'",
        )?;
        let rows = stmt.query_map(params![prefix_hex], map_node)?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Error::from)
    }

    pub fn list_nodes(&self) -> Result<Vec<NodeRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT pubkey, name, requested_name, contact_type, enrollment, last_heard,
                    last_advert, lat, lon, prefix, hop_bytes, path_len, qr_file, qr_uri,
                    COALESCE(noticed, 1), last_radio_id
             FROM nodes
             ORDER BY CASE enrollment
                WHEN 'pending' THEN 0
                WHEN 'enrolled' THEN 1
                WHEN 'hq' THEN 2
                WHEN 'denied' THEN 4
                ELSE 3 END, name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], map_node)?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Error::from)
    }

    pub fn hq_node(&self) -> Result<Option<NodeRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT pubkey, name, requested_name, contact_type, enrollment, last_heard,
                    last_advert, lat, lon, prefix, hop_bytes, path_len, qr_file, qr_uri,
                    COALESCE(noticed, 1), last_radio_id
             FROM nodes WHERE enrollment = 'hq' LIMIT 1",
        )?;
        Ok(stmt.query_row([], map_node).optional()?)
    }

    pub fn get_node(&self, pubkey: &str) -> Result<Option<NodeRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT pubkey, name, requested_name, contact_type, enrollment, last_heard,
                    last_advert, lat, lon, prefix, hop_bytes, path_len, qr_file, qr_uri,
                    COALESCE(noticed, 1), last_radio_id
             FROM nodes WHERE pubkey = ?1",
        )?;
        Ok(stmt.query_row(params![pubkey], map_node).optional()?)
    }

    pub fn node_raw_json(&self, pubkey: &str) -> Result<Option<String>> {
        let conn = self.lock();
        Ok(conn
            .query_row(
                "SELECT raw_json FROM nodes WHERE pubkey = ?1",
                params![pubkey],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn messages_for_pubkey(&self, pubkey: &str, limit: i64) -> Result<Vec<MessageRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT m.id, m.direction, m.kind,
                    COALESCE(n.name, m.sender_prefix, 'channel'),
                    m.sender_pubkey, m.channel_idx, m.hop_count, m.display_text, m.delivery, m.received_at,
                    p.source, COALESCE(p.path, x''), COALESCE(p.hash_size, 1), m.snr
             FROM messages m
             LEFT JOIN nodes n ON n.pubkey = m.sender_pubkey
             LEFT JOIN paths p ON p.id = (
                SELECT id FROM paths WHERE message_id = m.id ORDER BY id DESC LIMIT 1
             )
             WHERE m.kind = 'direct' AND m.sender_pubkey = ?1
             ORDER BY m.received_at DESC, m.id DESC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![pubkey, limit], Self::map_message_row)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Error::from)
    }

    /// Newest `limit` DMs for a contact, oldest → newest. `has_older` when more exist.
    pub fn list_dm_messages(
        &self,
        pubkey: &str,
        limit: i64,
    ) -> Result<(Vec<MessageRow>, bool)> {
        self.list_dm_messages_page(pubkey, None, limit)
    }

    pub fn list_dm_messages_before(
        &self,
        pubkey: &str,
        before_id: i64,
        limit: i64,
    ) -> Result<(Vec<MessageRow>, bool)> {
        self.list_dm_messages_page(pubkey, Some(before_id), limit)
    }

    fn list_dm_messages_page(
        &self,
        pubkey: &str,
        before_id: Option<i64>,
        limit: i64,
    ) -> Result<(Vec<MessageRow>, bool)> {
        let limit = limit.max(0);
        let fetch = limit + 1;
        let conn = self.lock();
        let sql = if before_id.is_some() {
            "SELECT id, direction, kind, sender_name, sender_pubkey, channel_idx, hop_count,
                    display_text, delivery, received_at, route_source, path, hash_size, snr
             FROM (
               SELECT m.id, m.direction, m.kind,
                      COALESCE(n.name, m.sender_prefix, 'direct') AS sender_name,
                      m.sender_pubkey, m.channel_idx, m.hop_count, m.display_text, m.delivery,
                      m.received_at, p.source AS route_source, COALESCE(p.path, x'') AS path,
                      COALESCE(p.hash_size, 1) AS hash_size, m.snr
               FROM messages m
               LEFT JOIN nodes n ON n.pubkey = m.sender_pubkey
               LEFT JOIN paths p ON p.id = (
                  SELECT id FROM paths WHERE message_id = m.id ORDER BY id DESC LIMIT 1
               )
               WHERE m.kind = 'direct' AND m.sender_pubkey = ?1 AND m.id < ?2
               ORDER BY m.received_at DESC, m.id DESC
               LIMIT ?3
             )
             ORDER BY received_at ASC, id ASC"
        } else {
            "SELECT id, direction, kind, sender_name, sender_pubkey, channel_idx, hop_count,
                    display_text, delivery, received_at, route_source, path, hash_size, snr
             FROM (
               SELECT m.id, m.direction, m.kind,
                      COALESCE(n.name, m.sender_prefix, 'direct') AS sender_name,
                      m.sender_pubkey, m.channel_idx, m.hop_count, m.display_text, m.delivery,
                      m.received_at, p.source AS route_source, COALESCE(p.path, x'') AS path,
                      COALESCE(p.hash_size, 1) AS hash_size, m.snr
               FROM messages m
               LEFT JOIN nodes n ON n.pubkey = m.sender_pubkey
               LEFT JOIN paths p ON p.id = (
                  SELECT id FROM paths WHERE message_id = m.id ORDER BY id DESC LIMIT 1
               )
               WHERE m.kind = 'direct' AND m.sender_pubkey = ?1
               ORDER BY m.received_at DESC, m.id DESC
               LIMIT ?2
             )
             ORDER BY received_at ASC, id ASC"
        };
        let mut stmt = conn.prepare(sql)?;
        let rows = if let Some(before) = before_id {
            stmt.query_map(params![pubkey, before, fetch], Self::map_message_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            stmt.query_map(params![pubkey, fetch], Self::map_message_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        Ok(trim_message_page(rows, limit))
    }

    fn dm_last_read_key(pubkey: &str) -> String {
        format!("dm_last_read_{}", pubkey.trim().to_ascii_lowercase())
    }

    pub fn dm_last_read(&self, pubkey: &str) -> Result<Option<i64>> {
        Ok(self
            .setting(&Self::dm_last_read_key(pubkey))?
            .and_then(|v| v.parse::<i64>().ok())
            .filter(|id| *id > 0))
    }

    pub fn set_dm_last_read(&self, pubkey: &str, message_id: i64) -> Result<()> {
        self.set_setting(&Self::dm_last_read_key(pubkey), &message_id.to_string())
    }

    /// Delete all direct messages for a peer and clear the last-read cursor.
    /// Does not change enrollment or roster membership.
    pub fn clear_dm_thread(&self, pubkey: &str) -> Result<u64> {
        let key = pubkey.trim();
        if key.is_empty() {
            return Err(Error::Msg("Direct message peer required.".into()));
        }
        let conn = self.lock();
        let deleted = conn.execute(
            "DELETE FROM messages WHERE kind = 'direct' AND sender_pubkey = ?1",
            params![key],
        )? as u64;
        if deleted > 0 {
            conn.execute(
                "DELETE FROM paths WHERE message_id IS NOT NULL AND message_id NOT IN (SELECT id FROM messages)",
                [],
            )?;
        }
        drop(conn);
        let _ = self.set_setting(&Self::dm_last_read_key(key), "0");
        Ok(deleted)
    }

    pub fn list_dm_threads(&self) -> Result<Vec<DmThreadRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT m.sender_pubkey,
                    COALESCE(NULLIF(n.name, ''), m.sender_prefix, substr(m.sender_pubkey, 1, 12)),
                    COUNT(*),
                    MAX(m.received_at),
                    (SELECT m2.display_text FROM messages m2
                     WHERE m2.kind = 'direct' AND m2.sender_pubkey = m.sender_pubkey
                     ORDER BY m2.received_at DESC, m2.id DESC LIMIT 1),
                    CASE
                      WHEN EXISTS (
                        SELECT 1 FROM settings s
                        WHERE s.key = 'dm_last_read_' || lower(m.sender_pubkey)
                      ) THEN (
                        SELECT COUNT(*) FROM messages m3
                        WHERE m3.kind = 'direct'
                          AND m3.sender_pubkey = m.sender_pubkey
                          AND m3.direction = 'rx'
                          AND m3.id > COALESCE(
                            (SELECT CAST(s.value AS INTEGER) FROM settings s
                             WHERE s.key = 'dm_last_read_' || lower(m.sender_pubkey)),
                            0
                          )
                      )
                      ELSE 0
                    END,
                    CASE
                      WHEN EXISTS (
                        SELECT 1 FROM settings s
                        WHERE s.key = 'dm_last_read_' || lower(m.sender_pubkey)
                      ) THEN (
                        SELECT COUNT(*) FROM messages m3
                        WHERE m3.kind = 'direct'
                          AND m3.sender_pubkey = m.sender_pubkey
                          AND m3.direction = 'rx'
                          AND COALESCE(m3.mentions_hq, 0) = 1
                          AND m3.id > COALESCE(
                            (SELECT CAST(s.value AS INTEGER) FROM settings s
                             WHERE s.key = 'dm_last_read_' || lower(m.sender_pubkey)),
                            0
                          )
                      )
                      ELSE 0
                    END
             FROM messages m
             LEFT JOIN nodes n ON n.pubkey = m.sender_pubkey
             WHERE m.kind = 'direct'
               AND m.direction = 'rx'
               AND m.sender_pubkey IS NOT NULL AND m.sender_pubkey != ''
             GROUP BY m.sender_pubkey
             ORDER BY MAX(m.received_at) DESC, m.sender_pubkey",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(DmThreadRow {
                    pubkey: row.get(0)?,
                    name: row.get(1)?,
                    message_count: row.get(2)?,
                    last_message_at: row.get(3)?,
                    last_text: row.get(4)?,
                    unread_count: row.get(5)?,
                    unread_mentions: row.get(6)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn total_channel_unread(&self) -> Result<i64> {
        Ok(self
            .list_channels()?
            .into_iter()
            .map(|c| c.unread_count)
            .sum())
    }

    pub fn total_dm_unread(&self) -> Result<i64> {
        Ok(self
            .list_dm_threads()?
            .into_iter()
            .map(|t| t.unread_count)
            .sum())
    }

    pub fn total_unread_mentions(&self) -> Result<i64> {
        let channels: i64 = self
            .list_channels()?
            .into_iter()
            .map(|c| c.unread_mentions)
            .sum();
        let dms: i64 = self
            .list_dm_threads()?
            .into_iter()
            .map(|t| t.unread_mentions)
            .sum();
        Ok(channels + dms)
    }

    pub fn count_unnoticed_nodes(&self) -> Result<i64> {
        let conn = self.lock();
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM nodes
             WHERE COALESCE(noticed, 1) = 0 AND enrollment != 'hq'",
            [],
            |row| row.get(0),
        )?)
    }

    pub fn mark_node_noticed(&self, pubkey: &str) -> Result<()> {
        self.lock().execute(
            "UPDATE nodes SET noticed = 1 WHERE pubkey = ?1",
            params![pubkey],
        )?;
        Ok(())
    }

    /// Clear all NEW pills on Roster (mark every node noticed).
    pub fn mark_all_nodes_noticed(&self) -> Result<u64> {
        let conn = self.lock();
        let changed = conn.execute(
            "UPDATE nodes SET noticed = 1 WHERE COALESCE(noticed, 1) = 0",
            [],
        )? as u64;
        if changed > 0 {
            conn.execute(
                "INSERT INTO audit (at, action, pubkey, detail) VALUES (?1, 'roster_clear_new', '', ?2)",
                params![unix_now(), changed.to_string()],
            )?;
        }
        Ok(changed)
    }

    /// Count heard-only nodes (eligible for roster prune).
    pub fn count_heard_only_nodes(&self) -> Result<i64> {
        let conn = self.lock();
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM nodes WHERE enrollment = 'heard'",
            [],
            |row| row.get(0),
        )?)
    }

    /// Remove heard-only roster rows and related tracker/block/claim rows.
    /// Keeps enrolled, pending, denied, and HQ. Does not delete message history.
    pub fn prune_heard_nodes(&self) -> Result<u64> {
        let conn = self.lock();
        let keys: Vec<String> = {
            let mut stmt = conn.prepare(
                "SELECT pubkey FROM nodes WHERE enrollment = 'heard'",
            )?;
            let mapped = stmt.query_map([], |row| row.get(0))?;
            mapped.collect::<std::result::Result<Vec<_>, _>>()?
        };
        if keys.is_empty() {
            return Ok(0);
        }
        for key in &keys {
            conn.execute("DELETE FROM tracked_nodes WHERE pubkey = ?1", params![key])?;
            conn.execute("DELETE FROM blocked_nodes WHERE pubkey = ?1", params![key])?;
            conn.execute("DELETE FROM claimed_infra WHERE pubkey = ?1", params![key])?;
            conn.execute("DELETE FROM node_radios WHERE pubkey = ?1", params![key])?;
            conn.execute("DELETE FROM nodes WHERE pubkey = ?1", params![key])?;
        }
        let n = keys.len() as u64;
        conn.execute(
            "INSERT INTO audit (at, action, pubkey, detail) VALUES (?1, 'roster_prune_heard', '', ?2)",
            params![unix_now(), n.to_string()],
        )?;
        Ok(n)
    }

    pub fn rf_samples_for_pubkey(&self, pubkey: &str, limit: i64) -> Result<Vec<(i64, i64, f64, i64)>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT noise_floor, last_rssi, snr, at FROM rf_samples
             WHERE pubkey = ?1 ORDER BY at DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![pubkey, limit], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Error::from)
    }

    pub fn position_count_for_pubkey(&self, pubkey: &str) -> Result<i64> {
        let conn = self.lock();
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM positions WHERE pubkey = ?1",
            params![pubkey],
            |row| row.get(0),
        )?)
    }

    /// Slim row for the QR worker.
    pub fn qr_contact_row(&self, pubkey: &str) -> Result<Option<QrContactRow>> {
        let conn = self.lock();
        conn.query_row(
            "SELECT pubkey, name, contact_type, enrollment, qr_file, qr_uri
             FROM nodes WHERE pubkey = ?1",
            params![pubkey],
            |row| {
                Ok(QrContactRow {
                    pubkey: row.get(0)?,
                    name: row.get(1)?,
                    contact_type: row.get(2)?,
                    enrollment: row.get(3)?,
                    qr_file: row.get(4)?,
                    qr_uri: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(Error::from)
    }

    pub fn set_node_qr(&self, pubkey: &str, qr_uri: &str, qr_file: &str) -> Result<()> {
        self.lock().execute(
            "UPDATE nodes SET qr_uri = ?2, qr_file = ?3 WHERE pubkey = ?1",
            params![pubkey, qr_uri, qr_file],
        )?;
        Ok(())
    }

    pub fn update_node_name(&self, pubkey: &str, name: &str) -> Result<()> {
        self.lock().execute(
            "UPDATE nodes SET name = ?2 WHERE pubkey = ?1",
            params![pubkey, name],
        )?;
        Ok(())
    }

    /// All contact pubkeys — startup QR worker walks these and skips up-to-date rows.
    pub fn list_qr_backfill_pubkeys(&self) -> Result<Vec<String>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT pubkey FROM nodes WHERE TRIM(COALESCE(pubkey, '')) != ''",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Error::from)
    }

    pub fn pending_nodes(&self) -> Result<Vec<NodeRow>> {
        Ok(self
            .list_nodes()?
            .into_iter()
            .filter(|node| node.enrollment == Enrollment::Pending)
            .collect())
    }

    pub fn set_enrollment(&self, pubkey: &str, enrollment: Enrollment, requested_name: Option<String>) -> Result<()> {
        self.lock().execute(
            "UPDATE nodes SET enrollment = ?2, requested_name = COALESCE(?3, requested_name), last_heard = ?4
             WHERE pubkey = ?1",
            params![pubkey, enrollment.as_str(), requested_name, unix_now()],
        )?;
        Ok(())
    }

    pub fn set_route(&self, pubkey: &str, bytes: &[u8]) -> Result<()> {
        self.lock().execute(
            "UPDATE nodes SET hop_bytes = ?2, path_len = ?3 WHERE pubkey = ?1",
            params![pubkey, bytes, bytes.len() as i64],
        )?;
        Ok(())
    }

    /// Record that `radio_id` heard `pubkey` (updates junction + last_radio_id).
    pub fn touch_node_radio(&self, pubkey: &str, radio_id: i64) -> Result<()> {
        let now = unix_now();
        let conn = self.lock();
        conn.execute(
            "UPDATE nodes SET last_heard = ?2, last_radio_id = ?3 WHERE pubkey = ?1",
            params![pubkey, now, radio_id],
        )?;
        conn.execute(
            "INSERT INTO node_radios (pubkey, radio_id, last_heard)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(pubkey, radio_id) DO UPDATE SET last_heard = excluded.last_heard",
            params![pubkey, radio_id, now],
        )?;
        Ok(())
    }

    /// Radios that have heard this node, newest first: (radio_id, last_heard).
    pub fn list_node_radios(&self, pubkey: &str) -> Result<Vec<(i64, i64)>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT radio_id, last_heard FROM node_radios
             WHERE pubkey = ?1
             ORDER BY last_heard DESC",
        )?;
        let rows = stmt.query_map(params![pubkey], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Error::from)
    }

    pub fn radio_label(&self, radio_id: i64) -> Result<Option<String>> {
        let conn = self.lock();
        Ok(conn
            .query_row(
                "SELECT CASE
                    WHEN TRIM(COALESCE(label, '')) = '' THEN address
                    ELSE label
                 END
                 FROM radios WHERE id = ?1",
                params![radio_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?)
    }

    /// Per-radio contact sync watermark (`contacts_since_{id}`), seeded from legacy global.
    pub fn contacts_since_for_radio(&self, radio_id: i64) -> Result<u32> {
        let key = format!("contacts_since_{radio_id}");
        if let Some(raw) = self.setting(&key)? {
            return Ok(raw.parse::<u32>().unwrap_or(0));
        }
        let global = self
            .setting("contacts_since")?
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(0);
        if global > 0 {
            self.set_setting(&key, &global.to_string())?;
        }
        Ok(global)
    }

    pub fn set_contacts_since_for_radio(&self, radio_id: i64, since: u32) -> Result<()> {
        self.set_setting(&format!("contacts_since_{radio_id}"), &since.to_string())
    }

    /// Resolve a channel/display name to exactly one known node (no guessing).
    pub fn unique_node_by_name(&self, name: &str) -> Result<Option<NodeRow>> {
        let needle = name.trim();
        if needle.is_empty() {
            return Ok(None);
        }
        let matches: Vec<_> = self
            .list_nodes()?
            .into_iter()
            .filter(|n| {
                n.name.eq_ignore_ascii_case(needle)
                    || n.requested_name
                        .as_deref()
                        .is_some_and(|r| r.eq_ignore_ascii_case(needle))
            })
            .collect();
        Ok(if matches.len() == 1 {
            Some(matches.into_iter().next().unwrap())
        } else {
            None
        })
    }

    /// Like [`Self::unique_node_by_name`], but also matches after stripping emoji /
    /// punctuation (`PlumbHunter🤠` ↔ `PlumbHunter`). Still requires exactly one hit.
    pub fn unique_node_by_display_name(&self, name: &str) -> Result<Option<NodeRow>> {
        if let Some(node) = self.unique_node_by_name(name)? {
            return Ok(Some(node));
        }
        let needle = ascii_alnum_key(name);
        if needle.is_empty() {
            return Ok(None);
        }
        let matches: Vec<_> = self
            .list_nodes()?
            .into_iter()
            .filter(|n| {
                ascii_alnum_key(&n.name) == needle
                    || n.requested_name
                        .as_deref()
                        .is_some_and(|r| ascii_alnum_key(r) == needle)
            })
            .collect();
        Ok(if matches.len() == 1 {
            Some(matches.into_iter().next().unwrap())
        } else {
            None
        })
    }

    pub fn insert_message(
        &self,
        kind: &str,
        direction: &str,
        sender_pubkey: Option<&str>,
        sender_prefix: &str,
        channel_idx: Option<i64>,
        hop_count: Option<i64>,
        sender_timestamp: u32,
        text: &str,
        display_text: &str,
        snr: Option<f64>,
        signature: Option<&[u8]>,
        raw: &Value,
        delivery: &str,
        radio_id: Option<i64>,
    ) -> Result<Option<i64>> {
        let sender_key = sender_pubkey.unwrap_or(sender_prefix);
        let dedupe = dedupe_key(kind, sender_key, sender_timestamp, text);
        let mentions_hq = if direction == "rx" {
            let hq = self.hq_advert_name().unwrap_or_default();
            if !hq.is_empty()
                && (text_mentions_name(text, &hq) || text_mentions_name(display_text, &hq))
            {
                1i64
            } else {
                0
            }
        } else {
            0
        };
        let conn = self.lock();
        let changed = conn.execute(
            "INSERT OR IGNORE INTO messages (
                dedupe_key, kind, direction, sender_pubkey, sender_prefix, channel_idx,
                hop_count, sender_timestamp, text, display_text, snr, signature, raw_json,
                delivery, received_at, radio_id, origin, mentions_hq
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, 'local', ?17)",
            params![
                dedupe,
                kind,
                direction,
                sender_pubkey,
                sender_prefix,
                channel_idx,
                hop_count,
                sender_timestamp,
                text,
                display_text,
                snr,
                signature,
                raw.to_string(),
                delivery,
                unix_now(),
                radio_id,
                mentions_hq
            ],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        Ok(Some(conn.last_insert_rowid()))
    }

    pub fn save_path(
        &self,
        message_id: Option<i64>,
        node_pubkey: Option<&str>,
        source: &str,
        path: &[u8],
        hash_size: i64,
    ) -> Result<()> {
        self.lock().execute(
            "INSERT INTO paths (message_id, node_pubkey, source, path, hash_size, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![message_id, node_pubkey, source, path, hash_size, unix_now()],
        )?;
        Ok(())
    }

    fn map_message_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MessageRow> {
        Ok(MessageRow {
            id: row.get(0)?,
            direction: row.get(1)?,
            kind: row.get(2)?,
            sender_name: row.get(3)?,
            sender_pubkey: row.get(4)?,
            channel_idx: row.get(5)?,
            hop_count: row.get(6)?,
            text: row.get(7)?,
            delivery: row.get(8)?,
            received_at: row.get(9)?,
            route_source: row.get(10)?,
            path: row.get(11)?,
            hash_size: row.get(12)?,
            snr: row.get(13)?,
        })
    }

    pub fn list_messages(&self, limit: i64) -> Result<Vec<MessageRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT m.id, m.direction, m.kind,
                    COALESCE(n.name, m.sender_prefix, 'channel'),
                    m.sender_pubkey, m.channel_idx, m.hop_count, m.display_text, m.delivery, m.received_at,
                    p.source, COALESCE(p.path, x''), COALESCE(p.hash_size, 1), m.snr
             FROM messages m
             LEFT JOIN nodes n ON n.pubkey = m.sender_pubkey
             LEFT JOIN paths p ON p.id = (
                SELECT id FROM paths WHERE message_id = m.id ORDER BY id DESC LIMIT 1
             )
             ORDER BY m.received_at DESC, m.id DESC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], Self::map_message_row)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Error::from)
    }

    pub fn recent_messages(&self, limit: i64) -> Result<Vec<MessageRow>> {
        self.list_messages(limit)
    }

    /// Recent messages optionally limited to a lookback window (seconds).
    pub fn recent_messages_window(
        &self,
        limit: i64,
        since_unix: Option<i64>,
    ) -> Result<Vec<MessageRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT m.id, m.direction, m.kind,
                    COALESCE(n.name, m.sender_prefix, 'channel'),
                    m.sender_pubkey, m.channel_idx, m.hop_count, m.display_text, m.delivery, m.received_at,
                    p.source, COALESCE(p.path, x''), COALESCE(p.hash_size, 1), m.snr
             FROM messages m
             LEFT JOIN nodes n ON n.pubkey = m.sender_pubkey
             LEFT JOIN paths p ON p.id = (
                SELECT id FROM paths WHERE message_id = m.id ORDER BY id DESC LIMIT 1
             )
             WHERE (?2 IS NULL OR m.received_at >= ?2)
             ORDER BY m.received_at DESC, m.id DESC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit, since_unix], Self::map_message_row)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Error::from)
    }

    pub fn update_hq_coords(&self, lat: f64, lon: f64) -> Result<()> {
        self.lock().execute(
            "UPDATE nodes SET lat = ?1, lon = ?2, last_heard = ?3 WHERE enrollment = 'hq'",
            params![lat, lon, unix_now()],
        )?;
        Ok(())
    }

    pub fn insert_rf_sample(
        &self,
        pubkey: &str,
        noise_floor: i16,
        last_rssi: i16,
        snr: f32,
    ) -> Result<()> {
        self.lock().execute(
            "INSERT INTO rf_samples (pubkey, noise_floor, last_rssi, snr, at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![pubkey, noise_floor as i64, last_rssi as i64, snr as f64, unix_now()],
        )?;
        Ok(())
    }

    pub fn recent_rf_samples(&self, limit: i64) -> Result<Vec<RfSampleRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT r.pubkey, COALESCE(n.name, substr(r.pubkey, 1, 8)), n.lat, n.lon,
                    r.noise_floor, r.last_rssi, r.snr, r.at
             FROM rf_samples r
             INNER JOIN nodes n ON n.pubkey = r.pubkey
             WHERE n.lat IS NOT NULL AND n.lon IS NOT NULL
             ORDER BY r.at DESC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |row| {
            Ok(RfSampleRow {
                pubkey: row.get(0)?,
                name: row.get(1)?,
                lat: row.get(2)?,
                lon: row.get(3)?,
                noise_floor: row.get(4)?,
                last_rssi: row.get(5)?,
                snr: row.get(6)?,
                at: row.get(7)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Error::from)
    }

    /// Latest RF sample per pubkey: (noise, rssi, snr, at).
    pub fn latest_rf_by_pubkey(&self) -> Result<std::collections::HashMap<String, (i64, i64, f64, i64)>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT pubkey, noise_floor, last_rssi, snr, at FROM rf_samples r
             WHERE at = (SELECT MAX(at) FROM rf_samples r2 WHERE r2.pubkey = r.pubkey)",
        )?;
        let mut map = std::collections::HashMap::new();
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                (
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, f64>(3)?,
                    row.get::<_, i64>(4)?,
                ),
            ))
        })?;
        for row in rows {
            let (key, sample) = row?;
            map.insert(key, sample);
        }
        Ok(map)
    }

    pub fn queue_text(&self, pubkey: &str, body: &str, display: &str) -> Result<i64> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO outbound (pubkey, body, display_text, created_at, delivery)
             VALUES (?1, ?2, ?3, ?4, 'queued')",
            params![pubkey, body, display, unix_now()],
        )?;
        let id = conn.last_insert_rowid();
        Ok(id)
    }

    pub fn next_outbound(&self) -> Result<Option<OutboundRow>> {
        let conn = self.lock();
        Ok(conn
            .query_row(
                "SELECT id, pubkey, body FROM outbound
                 WHERE delivery = 'queued' ORDER BY id LIMIT 1",
                [],
                |row| {
                    Ok(OutboundRow {
                        id: row.get(0)?,
                        pubkey: row.get(1)?,
                        body: row.get(2)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn mark_outbound_sent(
        &self,
        id: i64,
        display: &str,
        expected_ack: &str,
        delivery: &str,
    ) -> Result<()> {
        self.lock().execute(
            "UPDATE outbound SET body = ?2, display_text = ?2, delivery = ?3, expected_ack = ?4, sent_at = ?5
             WHERE id = ?1",
            params![id, display, delivery, expected_ack, unix_now()],
        )?;
        Ok(())
    }

    pub fn mark_outbound_failed(&self, id: i64, error: &str) -> Result<()> {
        self.lock().execute(
            "UPDATE outbound SET delivery = 'failed', error = ?2, body = display_text WHERE id = ?1",
            params![id, error],
        )?;
        Ok(())
    }

    /// Mark operator sends that never left the laptop so the UI does not lie.
    pub fn fail_stale_tx_messages(&self, older_than_secs: i64) -> Result<usize> {
        let cutoff = unix_now().saturating_sub(older_than_secs.max(1));
        let changed = self.lock().execute(
            "UPDATE messages SET delivery = 'failed'
             WHERE direction = 'tx'
               AND delivery IN ('queued', 'sending')
               AND received_at < ?1",
            params![cutoff],
        )?;
        Ok(changed)
    }

    pub fn mark_ack(&self, ack_hex: &str) -> Result<()> {
        self.lock().execute(
            "UPDATE outbound SET delivery = 'acked' WHERE expected_ack = ?1 AND delivery = 'sent'",
            params![ack_hex],
        )?;
        self.lock().execute(
            "UPDATE messages SET delivery = 'acked'
             WHERE delivery = 'sent' AND raw_json LIKE '%' || ?1 || '%'",
            params![ack_hex],
        )?;
        Ok(())
    }

    pub fn mark_timed_out(&self, outbound_id: i64) -> Result<()> {
        self.lock().execute(
            "UPDATE outbound SET delivery = 'timed_out' WHERE id = ?1 AND delivery = 'sent'",
            params![outbound_id],
        )?;
        Ok(())
    }

    pub fn approve(&self, pubkey: &str, secrets: &Secrets) -> Result<()> {
        let info = self.info_channel_name()?;
        let net = self.net_channel_name()?;
        let info_secret = self.ensure_named_channel_secret(&info, secrets)?;
        let net_secret = self.ensure_named_channel_secret(&net, secrets)?;
        let info_body = approval_message(&info, &hex::encode(info_secret));
        let net_body = approval_message(&net, &hex::encode(net_secret));
        let now = unix_now();
        let conn = self.lock();
        let tx = conn.unchecked_transaction()?;
        let changed = tx.execute(
            "UPDATE nodes SET enrollment = 'enrolled' WHERE pubkey = ?1 AND enrollment != 'hq'",
            params![pubkey],
        )?;
        if changed == 0 {
            return Err(Error::Msg("that node cannot be enrolled".into()));
        }
        tx.execute(
            "INSERT INTO outbound (pubkey, body, display_text, created_at, delivery)
             VALUES (?1, ?2, ?3, ?4, 'queued')",
            params![pubkey, info_body, REDACTED_SECRET, now],
        )?;
        tx.execute(
            "INSERT INTO outbound (pubkey, body, display_text, created_at, delivery)
             VALUES (?1, ?2, ?3, ?4, 'queued')",
            params![pubkey, net_body, REDACTED_SECRET, now],
        )?;
        tx.execute(
            "INSERT INTO audit (at, action, pubkey, detail) VALUES (?1, 'approve', ?2, ?3)",
            params![now, pubkey, format!("{info}+{net}")],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn deny(&self, pubkey: &str) -> Result<()> {
        self.lock().execute(
            "UPDATE nodes SET enrollment = 'denied' WHERE pubkey = ?1 AND enrollment = 'pending'",
            params![pubkey],
        )?;
        self.audit("deny", pubkey, "")
    }

    /// Removing an enrolled node rotates both private secrets and re-sends them.
    pub fn revoke(&self, pubkey: &str, secrets: &Secrets) -> Result<usize> {
        let info = self.info_channel_name()?;
        let net = self.net_channel_name()?;
        let info_secret = Secrets::generate();
        let net_secret = Secrets::generate();
        let kind = secrets.store(&info, &info_secret).map_err(Error::Msg)?;
        let _ = secrets.store(&net, &net_secret).map_err(Error::Msg)?;
        let generation = self
            .setting("channel_generation")?
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(0)
            + 1;
        self.set_setting("channel_generation", &generation.to_string())?;
        self.set_setting("secret_store", kind.as_str())?;
        let conn = self.lock();
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE nodes SET enrollment = 'denied' WHERE pubkey = ?1 AND enrollment = 'enrolled'",
            params![pubkey],
        )?;
        let mut stmt = tx.prepare("SELECT pubkey FROM nodes WHERE enrollment = 'enrolled'")?;
        let enrolled: Vec<String> = stmt
            .query_map([], |row| row.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(stmt);
        let info_body = approval_message(&info, &hex::encode(info_secret));
        let net_body = approval_message(&net, &hex::encode(net_secret));
        let now = unix_now();
        for key in &enrolled {
            tx.execute(
                "INSERT INTO outbound (pubkey, body, display_text, created_at, delivery)
                 VALUES (?1, ?2, ?3, ?4, 'queued')",
                params![key, info_body, REDACTED_SECRET, now],
            )?;
            tx.execute(
                "INSERT INTO outbound (pubkey, body, display_text, created_at, delivery)
                 VALUES (?1, ?2, ?3, ?4, 'queued')",
                params![key, net_body, REDACTED_SECRET, now],
            )?;
        }
        tx.execute(
            "INSERT INTO audit (at, action, pubkey, detail) VALUES (?1, 'revoke', ?2, ?3)",
            params![
                now,
                pubkey,
                format!("rotated {info} and {net} secrets")
            ],
        )?;
        tx.commit()?;
        Ok(enrolled.len())
    }

    pub fn ensure_channel_secret(&self, secrets: &Secrets) -> Result<[u8; 16]> {
        let info = self.info_channel_name()?;
        self.ensure_named_channel_secret(&info, secrets)
    }

    pub fn ensure_net_channel_secret(&self, secrets: &Secrets) -> Result<[u8; 16]> {
        let net = self.net_channel_name()?;
        self.ensure_named_channel_secret(&net, secrets)
    }

    pub fn ensure_named_channel_secret(
        &self,
        channel_name: &str,
        secrets: &Secrets,
    ) -> Result<[u8; 16]> {
        let channel_name = channel_name.trim();
        if channel_name.is_empty() || channel_name.eq_ignore_ascii_case("Public") {
            return Err(Error::Msg("Public channels do not use a private secret.".into()));
        }
        if let Some(existing) = secrets.load(channel_name).map_err(Error::Msg)? {
            return Ok(existing);
        }
        let secret = Secrets::generate();
        let kind = secrets.store(channel_name, &secret).map_err(Error::Msg)?;
        self.set_setting("secret_store", kind.as_str())?;
        if self.is_reserved_private(channel_name) {
            let generation = self
                .setting("channel_generation")?
                .and_then(|value| value.parse::<i64>().ok())
                .unwrap_or(0)
                + 1;
            self.set_setting("channel_generation", &generation.to_string())?;
            let role = if self.is_info_channel(channel_name) {
                "info"
            } else {
                "net"
            };
            let prefer = if role == "info" { 1 } else { 2 };
            let slot = self.allocate_system_channel_slot(channel_name, prefer)?;
            self.ensure_channel_row(channel_name, slot, "private", role)?;
        }
        Ok(secret)
    }

    /// Rename bot-home or peer-net: migrate secret + DB row, re-send name to enrolled.
    /// Returns `(new_name, slot)` so the UI can push the companion.
    pub fn rename_system_channel(
        &self,
        which: SystemChannel,
        new_name: &str,
        secrets: &Secrets,
    ) -> Result<(String, i64)> {
        let new_name = normalize_channel_name(new_name).ok_or_else(|| {
            Error::Msg(format!(
                "Enter a valid {} channel name (1–{MAX_CHANNEL_NAME_BYTES} chars, not Public).",
                which.label()
            ))
        })?;
        let old_name = self.system_channel_name(which)?;
        let other = match which {
            SystemChannel::Info => self.net_channel_name()?,
            SystemChannel::Net => self.info_channel_name()?,
        };
        if new_name.eq_ignore_ascii_case(&other) {
            return Err(Error::Msg(
                "Bot home and peer net must use different names.".into(),
            ));
        }
        if new_name == old_name {
            let slot = self
                .list_channels()?
                .into_iter()
                .find(|row| row.name.eq_ignore_ascii_case(&old_name))
                .map(|row| row.slot)
                .unwrap_or(1);
            return Ok((new_name, slot));
        }
        let conflict = self
            .list_channels()?
            .into_iter()
            .any(|row| row.name.eq_ignore_ascii_case(&new_name) && !row.name.eq_ignore_ascii_case(&old_name));
        if conflict {
            return Err(Error::Msg(format!(
                "Channel {new_name} already exists."
            )));
        }

        let secret = self.ensure_named_channel_secret(&old_name, secrets)?;
        let kind = secrets.store(&new_name, &secret).map_err(Error::Msg)?;
        self.set_setting("secret_store", kind.as_str())?;
        if !old_name.eq_ignore_ascii_case(&new_name) {
            let _ = secrets.forget(&old_name);
        }

        let prefer = match which {
            SystemChannel::Info => 1,
            SystemChannel::Net => 2,
        };
        let slot = self
            .list_channels()?
            .into_iter()
            .find(|row| row.name.eq_ignore_ascii_case(&old_name))
            .map(|row| row.slot)
            .unwrap_or(prefer);
        let role = which.role();
        self.lock().execute(
            "UPDATE channels SET name = ?2, secret_account = ?2, visibility = 'private', role = ?3, active = 1
             WHERE name = ?1 COLLATE NOCASE",
            params![old_name, new_name, role],
        )?;
        // If the row was missing (legacy DB), ensure it.
        self.ensure_channel_row(&new_name, slot, "private", role)?;
        self.set_setting(which.setting_key(), &new_name)?;

        let body = approval_message(&new_name, &hex::encode(secret));
        let now = unix_now();
        let enrolled: Vec<String> = {
            let conn = self.lock();
            let mut stmt =
                conn.prepare("SELECT pubkey FROM nodes WHERE enrollment = 'enrolled'")?;
            let rows = stmt
                .query_map([], |row| row.get(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        };
        {
            let conn = self.lock();
            for key in &enrolled {
                conn.execute(
                    "INSERT INTO outbound (pubkey, body, display_text, created_at, delivery)
                     VALUES (?1, ?2, ?3, ?4, 'queued')",
                    params![key, body, REDACTED_SECRET, now],
                )?;
            }
        }
        self.audit(
            "channel_rename",
            "",
            &format!("{}: {old_name} → {new_name}", which.label()),
        )?;
        let generation = self
            .setting("channel_generation")?
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(0)
            + 1;
        self.set_setting("channel_generation", &generation.to_string())?;
        Ok((new_name, slot))
    }

    /// Manually send a private-channel secret to a node (same style as signup approval).
    pub fn invite_to_private_channel(
        &self,
        pubkey: &str,
        channel_name: &str,
        secrets: &Secrets,
    ) -> Result<()> {
        let channel_name = channel_name.trim();
        if channel_name.is_empty() || channel_name.eq_ignore_ascii_case("Public") {
            return Err(Error::Msg("Choose a private channel.".into()));
        }
        let exists = self
            .list_channels()?
            .into_iter()
            .any(|row| row.name.eq_ignore_ascii_case(channel_name) && row.visibility == "private");
        if !exists && !self.is_reserved_private(channel_name) {
            return Err(Error::Msg(format!(
                "Private channel {channel_name} is not in the channel list."
            )));
        }
        let secret = self.ensure_named_channel_secret(channel_name, secrets)?;
        let body = approval_message(channel_name, &hex::encode(secret));
        let conn = self.lock();
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE nodes SET enrollment = 'enrolled'
             WHERE pubkey = ?1 AND enrollment IN ('heard', 'pending', 'denied', 'enrolled')",
            params![pubkey],
        )?;
        let changed = tx.execute(
            "INSERT INTO outbound (pubkey, body, display_text, created_at, delivery)
             VALUES (?1, ?2, ?3, ?4, 'queued')",
            params![pubkey, body, REDACTED_SECRET, unix_now()],
        )?;
        if changed == 0 {
            return Err(Error::Msg("Unable to queue the channel invite.".into()));
        }
        tx.execute(
            "INSERT INTO audit (at, action, pubkey, detail) VALUES (?1, 'invite_channel', ?2, ?3)",
            params![unix_now(), pubkey, channel_name],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn channel_generation(&self) -> Result<i64> {
        Ok(self
            .setting("channel_generation")?
            .and_then(|value| value.parse().ok())
            .unwrap_or(0))
    }

    pub fn secret_store_label(&self) -> Result<String> {
        let kind = match self.setting("secret_store")?.as_deref() {
            Some("keyring") => SecretStoreKind::Keyring,
            Some("file") => SecretStoreKind::File,
            _ => return Ok("Not created yet".into()),
        };
        Ok(kind.label().into())
    }

    pub fn audit(&self, action: &str, pubkey: &str, detail: &str) -> Result<()> {
        self.lock().execute(
            "INSERT INTO audit (at, action, pubkey, detail) VALUES (?1, ?2, ?3, ?4)",
            params![unix_now(), action, pubkey, detail],
        )?;
        Ok(())
    }

    pub fn service_event_seen(&self, id: &str) -> Result<bool> {
        let conn = self.lock();
        let mut stmt = conn.prepare("SELECT 1 FROM service_events WHERE id = ?1")?;
        Ok(stmt.exists(params![id])?)
    }

    pub fn insert_service_event(
        &self,
        kind: &str,
        id: &str,
        body: &str,
        detail: &str,
        status: &str,
    ) -> Result<()> {
        let decided = if status == "pending" {
            None
        } else {
            Some(unix_now())
        };
        self.lock().execute(
            "INSERT OR IGNORE INTO service_events
             (id, kind, body, detail, status, created_at, decided_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id, kind, body, detail, status, unix_now(), decided],
        )?;
        Ok(())
    }

    pub fn list_pending_service_events(&self) -> Result<Vec<ServiceEventRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, kind, body, detail, status, created_at, decided_at
             FROM service_events
             WHERE status = 'pending'
             ORDER BY created_at DESC
             LIMIT 50",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(ServiceEventRow {
                    id: row.get(0)?,
                    kind: row.get(1)?,
                    body: row.get(2)?,
                    detail: row.get(3)?,
                    status: row.get(4)?,
                    created_at: row.get(5)?,
                    decided_at: row.get(6)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Claim a METIS-INFO bot command so reconnect/drain cannot re-answer it.
    /// Returns true if this is the first time we have seen `id`.
    pub fn claim_bot_command(&self, id: &str) -> Result<bool> {
        let changed = self.lock().execute(
            "INSERT OR IGNORE INTO bot_handled (id, at) VALUES (?1, ?2)",
            params![id, unix_now()],
        )?;
        Ok(changed > 0)
    }

    pub fn decide_service_event(&self, id: &str, status: &str) -> Result<Option<ServiceEventRow>> {
        let conn = self.lock();
        let updated = conn.execute(
            "UPDATE service_events SET status = ?2, decided_at = ?3
             WHERE id = ?1 AND status = 'pending'",
            params![id, status, unix_now()],
        )?;
        if updated == 0 {
            return Ok(None);
        }
        let mut stmt = conn.prepare(
            "SELECT id, kind, body, detail, status, created_at, decided_at
             FROM service_events WHERE id = ?1",
        )?;
        Ok(stmt
            .query_row(params![id], |row| {
                Ok(ServiceEventRow {
                    id: row.get(0)?,
                    kind: row.get(1)?,
                    body: row.get(2)?,
                    detail: row.get(3)?,
                    status: row.get(4)?,
                    created_at: row.get(5)?,
                    decided_at: row.get(6)?,
                })
            })
            .optional()?)
    }

    pub fn purge_preview(&self, cutoff: i64) -> Result<PurgePreview> {
        let conn = self.lock();
        Ok(PurgePreview {
            messages: count_older(&conn, "messages", "received_at", cutoff)?,
            paths: count_older(&conn, "paths", "created_at", cutoff)?,
            positions: count_older(&conn, "positions", "at", cutoff)?,
            telemetry: count_older(&conn, "telemetry_samples", "at", cutoff)?,
        })
    }

    pub fn purge(&self, cutoff: i64) -> Result<PurgePreview> {
        let preview = self.purge_preview(cutoff)?;
        let conn = self.lock();
        conn.execute("DELETE FROM messages WHERE received_at < ?1", params![cutoff])?;
        conn.execute("DELETE FROM paths WHERE created_at < ?1", params![cutoff])?;
        conn.execute(
            "DELETE FROM paths WHERE message_id IS NOT NULL AND message_id NOT IN (SELECT id FROM messages)",
            [],
        )?;
        conn.execute("DELETE FROM positions WHERE at < ?1", params![cutoff])?;
        conn.execute(
            "DELETE FROM telemetry_samples WHERE at < ?1",
            params![cutoff],
        )?;
        conn.execute(
            "INSERT INTO audit (at, action, pubkey, detail) VALUES (?1, 'purge', '', ?2)",
            params![unix_now(), cutoff.to_string()],
        )?;
        Ok(preview)
    }

    pub fn insert_telemetry_sample(
        &self,
        kind: &str,
        value_mv: f64,
        value_pct: f64,
    ) -> Result<()> {
        self.lock().execute(
            "INSERT INTO telemetry_samples (kind, value_mv, value_pct, at)
             VALUES (?1, ?2, ?3, ?4)",
            params![kind, value_mv, value_pct, unix_now()],
        )?;
        Ok(())
    }

    pub fn telemetry_series(&self, kind: &str, since: i64) -> Result<Vec<TelemetryPoint>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT at, value_pct, value_mv FROM telemetry_samples
             WHERE kind = ?1 AND at >= ?2
             ORDER BY at ASC",
        )?;
        let rows = stmt
            .query_map(params![kind, since], |row| {
                Ok(TelemetryPoint {
                    at: row.get(0)?,
                    value_pct: row.get(1)?,
                    value_mv: row.get(2)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Zero-filled hourly RX/TX counts for the last `hours` buckets.
    pub fn traffic_hourly(&self, hours: i64) -> Result<Vec<HourlyTraffic>> {
        let (since, starts) = hour_bucket_starts(hours);
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT (received_at / 3600) * 3600 AS hour_start,
                    SUM(CASE WHEN direction = 'rx' THEN 1 ELSE 0 END) AS rx,
                    SUM(CASE WHEN direction = 'tx' THEN 1 ELSE 0 END) AS tx
             FROM messages
             WHERE received_at >= ?1
             GROUP BY hour_start",
        )?;
        let mut map = std::collections::HashMap::<i64, (i64, i64)>::new();
        let rows = stmt.query_map(params![since], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?))
        })?;
        for row in rows {
            let (hour, rx, tx) = row?;
            map.insert(hour, (rx, tx));
        }
        Ok(starts
            .into_iter()
            .map(|hour_start| {
                let (rx, tx) = map.get(&hour_start).copied().unwrap_or((0, 0));
                HourlyTraffic {
                    hour_start,
                    rx,
                    tx,
                }
            })
            .collect())
    }

    pub fn traffic_kind_counts(&self, since: i64) -> Result<Vec<KindCount>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT kind, COUNT(*) FROM messages
             WHERE received_at >= ?1
             GROUP BY kind
             ORDER BY COUNT(*) DESC, kind COLLATE NOCASE",
        )?;
        let rows = stmt
            .query_map(params![since], |row| {
                Ok(KindCount {
                    kind: row.get(0)?,
                    count: row.get(1)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn active_senders_hourly(&self, hours: i64) -> Result<Vec<f32>> {
        let (since, starts) = hour_bucket_starts(hours);
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT (received_at / 3600) * 3600 AS hour_start,
                    COUNT(DISTINCT sender_pubkey)
             FROM messages
             WHERE received_at >= ?1
               AND sender_pubkey IS NOT NULL
               AND TRIM(sender_pubkey) != ''
             GROUP BY hour_start",
        )?;
        Ok(fill_hourly_f32(&starts, query_hour_counts(&mut stmt, since)?))
    }

    pub fn inbound_direct_hourly(&self, hours: i64) -> Result<Vec<f32>> {
        let (since, starts) = hour_bucket_starts(hours);
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT (received_at / 3600) * 3600 AS hour_start, COUNT(*)
             FROM messages
             WHERE received_at >= ?1 AND kind = 'direct' AND direction = 'rx'
             GROUP BY hour_start",
        )?;
        Ok(fill_hourly_f32(&starts, query_hour_counts(&mut stmt, since)?))
    }

    pub fn position_updates_hourly(&self, hours: i64) -> Result<Vec<f32>> {
        let (since, starts) = hour_bucket_starts(hours);
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT (at / 3600) * 3600 AS hour_start, COUNT(*)
             FROM positions
             WHERE at >= ?1
             GROUP BY hour_start",
        )?;
        Ok(fill_hourly_f32(&starts, query_hour_counts(&mut stmt, since)?))
    }

    pub fn messages_hourly(&self, hours: i64) -> Result<Vec<f32>> {
        let (since, starts) = hour_bucket_starts(hours);
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT (received_at / 3600) * 3600 AS hour_start, COUNT(*)
             FROM messages
             WHERE received_at >= ?1
             GROUP BY hour_start",
        )?;
        Ok(fill_hourly_f32(&starts, query_hour_counts(&mut stmt, since)?))
    }

    pub fn messages_since_count(&self, since: i64) -> Result<i64> {
        let conn = self.lock();
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE received_at >= ?1",
            params![since],
            |row| row.get(0),
        )?)
    }

    /// All-time stored message count for a kind (`channel`, `direct`, …).
    pub fn count_messages_kind(&self, kind: &str) -> Result<i64> {
        let kind = kind.trim();
        if kind.is_empty() {
            return Ok(0);
        }
        let conn = self.lock();
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE kind = ?1",
            params![kind],
            |row| row.get(0),
        )?)
    }

    pub fn list_tracked_nodes(&self) -> Result<Vec<TrackedNodeRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT pubkey, label, notes, added_at FROM tracked_nodes ORDER BY added_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(TrackedNodeRow {
                pubkey: row.get(0)?,
                label: row.get(1)?,
                notes: row.get(2)?,
                added_at: row.get(3)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Error::from)
    }

    pub fn tracked_pubkey_set(&self) -> Result<std::collections::HashSet<String>> {
        Ok(self
            .list_tracked_nodes()?
            .into_iter()
            .map(|row| row.pubkey)
            .collect())
    }

    pub fn add_tracked_node(&self, pubkey: &str, label: &str, notes: &str) -> Result<()> {
        let key = pubkey.trim();
        if key.is_empty() {
            return Err(Error::Msg("Missing pubkey.".into()));
        }
        self.lock().execute(
            "INSERT INTO tracked_nodes (pubkey, label, notes, added_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(pubkey) DO UPDATE SET
                label = excluded.label,
                notes = excluded.notes",
            params![key, label.trim(), notes.trim(), unix_now()],
        )?;
        Ok(())
    }

    pub fn remove_tracked_node(&self, pubkey: &str) -> Result<()> {
        self.lock()
            .execute("DELETE FROM tracked_nodes WHERE pubkey = ?1", params![pubkey])?;
        Ok(())
    }

    pub fn list_blocked_nodes(&self) -> Result<Vec<BlockedNodeRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT pubkey, label, added_at FROM blocked_nodes ORDER BY added_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(BlockedNodeRow {
                pubkey: row.get(0)?,
                label: row.get(1)?,
                added_at: row.get(2)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Error::from)
    }

    pub fn is_blocked(&self, pubkey: &str) -> Result<bool> {
        let key = pubkey.trim();
        if key.is_empty() {
            return Ok(false);
        }
        let conn = self.lock();
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM blocked_nodes WHERE pubkey = ?1",
            params![key],
            |row| row.get(0),
        )?;
        Ok(n > 0)
    }

    /// True when this channel sender is blocked by pubkey and/or display name.
    pub fn is_channel_sender_blocked(
        &self,
        display_name: Option<&str>,
        pubkey: Option<&str>,
    ) -> Result<bool> {
        if let Some(pk) = pubkey.map(str::trim).filter(|s| !s.is_empty()) {
            if self.is_blocked(pk)? {
                return Ok(true);
            }
        }
        if let Some(name) = display_name.map(str::trim).filter(|s| !s.is_empty()) {
            if let Some(nk) = name_block_key(name) {
                if self.is_blocked(&nk)? {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// Drop future channel/DM traffic from this pubkey (does not change enrollment).
    pub fn block_node(&self, pubkey: &str, label: &str) -> Result<()> {
        let key = pubkey.trim();
        if key.is_empty() {
            return Err(Error::Msg("Missing pubkey.".into()));
        }
        let name = label.trim();
        let fallback = key.chars().take(12).collect::<String>();
        let stored = if name.is_empty() { fallback.as_str() } else { name };
        self.lock().execute(
            "INSERT INTO blocked_nodes (pubkey, label, added_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(pubkey) DO UPDATE SET label = excluded.label",
            params![key, stored, unix_now()],
        )?;
        self.audit("block", key, stored)
    }

    /// Block a channel/DM sender. Channel frames often lack a pubkey — in that
    /// case we store a stable `name:<alnum>` key so matching channel traffic drops.
    /// When a pubkey is known (or uniquely resolvable from the roster), that is
    /// blocked too so DMs are covered.
    pub fn block_sender(&self, pubkey: Option<&str>, label: &str) -> Result<()> {
        let name = label.trim();
        if name.is_empty() && pubkey.map(str::trim).unwrap_or("").is_empty() {
            return Err(Error::Msg("Missing sender to block.".into()));
        }
        let mut resolved = pubkey
            .map(str::trim)
            .filter(|s| !s.is_empty() && !s.starts_with(NAME_BLOCK_PREFIX))
            .map(str::to_string);
        if resolved.is_none() && !name.is_empty() {
            resolved = self
                .unique_node_by_display_name(name)?
                .map(|n| n.pubkey);
        }
        if let Some(ref pk) = resolved {
            self.block_node(pk, if name.is_empty() { pk } else { name })?;
        }
        if !name.is_empty() {
            if let Some(nk) = name_block_key(name) {
                self.block_node(&nk, name)?;
            } else if resolved.is_none() {
                return Err(Error::Msg("Missing sender to block.".into()));
            }
        } else if resolved.is_none() {
            return Err(Error::Msg("Missing sender to block.".into()));
        }
        Ok(())
    }

    /// If this display name was blocked from a channel, also block the real pubkey
    /// once the node appears on the roster (so DMs drop too).
    pub fn promote_name_block_to_pubkey(&self, pubkey: &str, name: &str) -> Result<()> {
        let key = pubkey.trim();
        if key.is_empty() || key.starts_with(NAME_BLOCK_PREFIX) {
            return Ok(());
        }
        let Some(nk) = name_block_key(name) else {
            return Ok(());
        };
        if !self.is_blocked(&nk)? || self.is_blocked(key)? {
            return Ok(());
        }
        self.block_node(key, name.trim())
    }

    pub fn unblock_node(&self, pubkey: &str) -> Result<()> {
        let key = pubkey.trim();
        if key.is_empty() {
            return Err(Error::Msg("Missing pubkey.".into()));
        }
        let n = self
            .lock()
            .execute("DELETE FROM blocked_nodes WHERE pubkey = ?1", params![key])?;
        if n == 0 {
            return Err(Error::Msg("That contact is not blocked.".into()));
        }
        self.audit("unblock", key, "")
    }

    pub fn update_tracked_notes(&self, pubkey: &str, notes: &str) -> Result<()> {
        let n = self.lock().execute(
            "UPDATE tracked_nodes SET notes = ?2 WHERE pubkey = ?1",
            params![pubkey, notes.trim()],
        )?;
        if n == 0 {
            return Err(Error::Msg("That device is not on the Tracker list.".into()));
        }
        Ok(())
    }

    /// Default claimed-infra kind from MeshCore `contact_type` (2 repeater, 3 room, 4 sensor).
    pub fn infra_kind_from_contact_type(contact_type: u8) -> &'static str {
        match contact_type {
            3 => "room",
            4 => "sensor",
            _ => "repeater",
        }
    }

    pub fn normalize_infra_kind(kind: &str) -> &'static str {
        match kind.trim().to_ascii_lowercase().as_str() {
            "room" | "roomserver" | "room_server" => "room",
            "sensor" => "sensor",
            _ => "repeater",
        }
    }

    pub fn list_claimed_infra(&self) -> Result<Vec<ClaimedInfraRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT pubkey, kind, label, notes, added_at, verified_at
             FROM claimed_infra
             ORDER BY added_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ClaimedInfraRow {
                pubkey: row.get(0)?,
                kind: row.get(1)?,
                label: row.get(2)?,
                notes: row.get(3)?,
                added_at: row.get(4)?,
                verified_at: row.get(5)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Error::from)
    }

    pub fn claimed_pubkey_set(&self) -> Result<std::collections::HashSet<String>> {
        Ok(self
            .list_claimed_infra()?
            .into_iter()
            .map(|row| row.pubkey)
            .collect())
    }

    pub fn count_claimed_infra(&self) -> Result<i64> {
        Ok(self.lock().query_row(
            "SELECT COUNT(*) FROM claimed_infra",
            [],
            |row| row.get(0),
        )?)
    }

    /// Insert/update a claim only after RF admin login succeeded. Never store passwords.
    pub fn claim_infra_verified(
        &self,
        pubkey: &str,
        kind_override: Option<&str>,
        label: &str,
        notes: &str,
    ) -> Result<()> {
        let key = pubkey.trim();
        if key.is_empty() {
            return Err(Error::Msg("Missing pubkey.".into()));
        }
        let kind = if let Some(raw) = kind_override {
            Self::normalize_infra_kind(raw).to_string()
        } else {
            let contact_type = self
                .get_node(key)?
                .map(|n| n.contact_type)
                .unwrap_or(2);
            Self::infra_kind_from_contact_type(contact_type).to_string()
        };
        let now = unix_now();
        self.lock().execute(
            "INSERT INTO claimed_infra (pubkey, kind, label, notes, added_at, verified_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(pubkey) DO UPDATE SET
                kind = excluded.kind,
                label = excluded.label,
                notes = CASE
                    WHEN excluded.notes = '' THEN claimed_infra.notes
                    ELSE excluded.notes
                END,
                verified_at = excluded.verified_at",
            params![key, kind, label.trim(), notes.trim(), now, now],
        )?;
        let _ = self.audit("claim_infra_verified", key, &kind);
        Ok(())
    }

    /// Re-verify an existing claim after ACL was lost (sets verified_at).
    pub fn mark_claimed_verified(&self, pubkey: &str) -> Result<()> {
        let n = self.lock().execute(
            "UPDATE claimed_infra SET verified_at = ?2 WHERE pubkey = ?1",
            params![pubkey, unix_now()],
        )?;
        if n == 0 {
            return Err(Error::Msg("That node is not on the Infrastructure list.".into()));
        }
        let _ = self.audit("claim_infra_reverified", pubkey, "");
        Ok(())
    }

    pub fn unclaim_infra(&self, pubkey: &str) -> Result<()> {
        self.lock()
            .execute("DELETE FROM claimed_infra WHERE pubkey = ?1", params![pubkey])?;
        let _ = self.audit("unclaim_infra", pubkey, "");
        Ok(())
    }

    pub fn update_claimed_notes(&self, pubkey: &str, notes: &str) -> Result<()> {
        let n = self.lock().execute(
            "UPDATE claimed_infra SET notes = ?2 WHERE pubkey = ?1",
            params![pubkey, notes.trim()],
        )?;
        if n == 0 {
            return Err(Error::Msg("That node is not on the Infrastructure list.".into()));
        }
        Ok(())
    }

    pub fn update_claimed_kind(&self, pubkey: &str, kind: &str) -> Result<()> {
        let kind = Self::normalize_infra_kind(kind);
        let n = self.lock().execute(
            "UPDATE claimed_infra SET kind = ?2 WHERE pubkey = ?1",
            params![pubkey, kind],
        )?;
        if n == 0 {
            return Err(Error::Msg("That node is not on the Infrastructure list.".into()));
        }
        Ok(())
    }

    /// Position breadcrumbs for trail drawing (oldest → newest per query order).
    pub fn positions_for_pubkeys(
        &self,
        pubkeys: &[String],
        since_unix: Option<i64>,
    ) -> Result<Vec<PositionPoint>> {
        if pubkeys.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.lock();
        let mut out = Vec::new();
        for key in pubkeys {
            let mut stmt = conn.prepare(
                "SELECT pubkey, lat, lon, at FROM positions
                 WHERE pubkey = ?1 AND (?2 IS NULL OR at >= ?2)
                 ORDER BY at ASC
                 LIMIT 2000",
            )?;
            let rows = stmt.query_map(params![key, since_unix], |row| {
                Ok(PositionPoint {
                    pubkey: row.get(0)?,
                    lat: row.get(1)?,
                    lon: row.get(2)?,
                })
            })?;
            for row in rows {
                out.push(row?);
            }
        }
        Ok(out)
    }

    pub fn known_nodes_for_paths(&self) -> Result<Vec<crate::path::KnownNode>> {
        let conn = self.lock();
        let mut stmt = conn.prepare("SELECT pubkey, name, lat, lon FROM nodes")?;
        let rows = stmt.query_map([], |row| {
            let pubkey: String = row.get(0)?;
            Ok(crate::path::KnownNode {
                pubkey: hex::decode(&pubkey).unwrap_or_default(),
                name: row.get(1)?,
                lat: row.get(2)?,
                lon: row.get(3)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Error::from)
    }

    pub fn hq_position(&self) -> Result<Option<(String, f64, f64)>> {
        let conn = self.lock();
        Ok(conn
            .query_row(
                "SELECT pubkey, lat, lon FROM nodes WHERE enrollment = 'hq' AND lat IS NOT NULL AND lon IS NOT NULL LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?)
    }

    pub fn hq_radio_raw(&self) -> Result<Option<Value>> {
        let conn = self.lock();
        let raw: Option<String> = conn
            .query_row(
                "SELECT raw_json FROM nodes WHERE enrollment = 'hq'
                 ORDER BY last_heard DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        Ok(raw.and_then(|text| serde_json::from_str(&text).ok()))
    }

    /// Advert / device name for the headquarters companion.
    pub fn hq_advert_name(&self) -> Result<String> {
        if let Some(name) = self.setting("hq_name")? {
            let name = name.trim();
            if !name.is_empty() {
                return Ok(name.to_string());
            }
        }
        let conn = self.lock();
        let name: Option<String> = conn
            .query_row(
                "SELECT name FROM nodes
                 WHERE enrollment = 'hq' AND TRIM(COALESCE(name, '')) != ''
                 ORDER BY last_heard DESC
                 LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        Ok(name
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "hq".into()))
    }
}

#[derive(Debug, Clone)]
pub struct QrContactRow {
    pub pubkey: String,
    pub name: String,
    pub contact_type: u8,
    pub enrollment: String,
    pub qr_file: Option<String>,
    pub qr_uri: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TrackedNodeRow {
    pub pubkey: String,
    pub label: String,
    pub notes: String,
    pub added_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockedNodeRow {
    /// Real 32-byte hex pubkey, or `name:<alnum>` for channel display-name blocks.
    pub pubkey: String,
    pub label: String,
    pub added_at: i64,
}

impl BlockedNodeRow {
    pub fn is_name_block(&self) -> bool {
        self.pubkey.starts_with(NAME_BLOCK_PREFIX)
    }
}

#[derive(Debug, Clone)]
pub struct ClaimedInfraRow {
    pub pubkey: String,
    pub kind: String,
    pub label: String,
    pub notes: String,
    pub added_at: i64,
    /// Unix seconds; 0 = unverified (pre-change claim or failed gate).
    pub verified_at: i64,
}

impl ClaimedInfraRow {
    pub fn is_verified(&self) -> bool {
        self.verified_at > 0
    }
}

#[derive(Debug, Clone)]
pub struct PositionPoint {
    pub pubkey: String,
    pub lat: f64,
    pub lon: f64,
}

/// Drop the extra oldest row when we fetched `limit + 1` to detect has_older.
fn trim_message_page(mut rows: Vec<MessageRow>, limit: i64) -> (Vec<MessageRow>, bool) {
    let has_older = (rows.len() as i64) > limit;
    if has_older && !rows.is_empty() {
        rows.remove(0);
    }
    (rows, has_older)
}

fn map_node(row: &rusqlite::Row<'_>) -> rusqlite::Result<NodeRow> {
    let lat: Option<f64> = row.get(7)?;
    let lon: Option<f64> = row.get(8)?;
    let enrollment: String = row.get(4)?;
    let noticed_i: i64 = row.get(14)?;
    Ok(NodeRow {
        pubkey: row.get(0)?,
        name: row.get(1)?,
        requested_name: row.get(2)?,
        contact_type: row.get(3)?,
        enrollment: Enrollment::parse(&enrollment),
        last_heard: row.get(5)?,
        last_advert: row.get(6)?,
        lat,
        lon,
        has_gps: lat.is_some() && lon.is_some(),
        prefix: row.get(9)?,
        hop_bytes: row.get(10)?,
        path_len: row.get(11)?,
        qr_file: row.get(12)?,
        qr_uri: row.get(13)?,
        noticed: noticed_i != 0,
        last_radio_id: row.get(15)?,
    })
}

fn count_older(conn: &Connection, table: &str, column: &str, cutoff: i64) -> Result<i64> {
    let sql = format!("SELECT COUNT(*) FROM {table} WHERE {column} < ?1");
    let count = conn.query_row(&sql, params![cutoff], |row| row.get(0))?;
    Ok(count)
}

fn hour_bucket_starts(hours: i64) -> (i64, Vec<i64>) {
    let hours = hours.max(1);
    let now = unix_now();
    let end_hour = (now / 3600) * 3600;
    let starts: Vec<i64> = (0..hours)
        .rev()
        .map(|offset| end_hour - offset * 3600)
        .collect();
    let since = starts.first().copied().unwrap_or(end_hour);
    (since, starts)
}

fn query_hour_counts(
    stmt: &mut rusqlite::Statement<'_>,
    since: i64,
) -> Result<std::collections::HashMap<i64, i64>> {
    let mut map = std::collections::HashMap::new();
    let rows = stmt.query_map(params![since], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
    })?;
    for row in rows {
        let (hour, count) = row?;
        map.insert(hour, count);
    }
    Ok(map)
}

fn fill_hourly_f32(starts: &[i64], map: std::collections::HashMap<i64, i64>) -> Vec<f32> {
    starts
        .iter()
        .map(|hour| map.get(hour).copied().unwrap_or(0) as f32)
        .collect()
}

/// Trim and validate a MeshCore channel name for HQ settings / create.
pub fn normalize_channel_name(raw: &str) -> Option<String> {
    let name = raw.trim();
    if name.is_empty() || name.eq_ignore_ascii_case("Public") {
        return None;
    }
    if name.len() > MAX_CHANNEL_NAME_BYTES {
        return None;
    }
    if name.chars().any(|c| c.is_control() || c == '\n' || c == '\r') {
        return None;
    }
    Some(name.to_string())
}

/// Prefix for channel display-name blocks stored in `blocked_nodes.pubkey`.
const NAME_BLOCK_PREFIX: &str = "name:";

/// Lowercase ASCII letters/digits only — strip emoji and punctuation for name match.
fn ascii_alnum_key(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Stable key for channel-name blocks (`name:ryinescondido`).
pub fn name_block_key(display_name: &str) -> Option<String> {
    let key = ascii_alnum_key(display_name);
    if key.is_empty() {
        None
    } else {
        Some(format!("{NAME_BLOCK_PREFIX}{key}"))
    }
}

pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

/// Temp SQLite path unique across parallel tests (pid + nanos + counter).
fn unique_temp_db_path(prefix: &str) -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "{prefix}-{}-{nanos}-{seq}.db",
        std::process::id()
    ))
}

pub fn merge_enrollment_rank(current: Enrollment, incoming: Enrollment) -> Enrollment {
    fn rank(e: Enrollment) -> u8 {
        match e {
            Enrollment::Hq => 4,
            Enrollment::Enrolled => 3,
            Enrollment::Pending => 2,
            Enrollment::Heard => 1,
            Enrollment::Denied => 0,
        }
    }
    if rank(incoming) > rank(current) {
        incoming
    } else {
        current
    }
}

pub fn coords_from_micro(lat_micro: i32, lon_micro: i32) -> (Option<f64>, Option<f64>) {
    if lat_micro == 0 && lon_micro == 0 {
        return (None, None);
    }
    let lat = lat_micro as f64 / 1_000_000.0;
    let lon = lon_micro as f64 / 1_000_000.0;
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return (None, None);
    }
    (Some(lat), Some(lon))
}

pub fn contact_snapshot(
    name: &str,
    contact_type: u8,
    flags: u8,
    path_len: i64,
    last_advert: i64,
    lat_micro: i32,
    lon_micro: i32,
    lastmod: i64,
) -> Value {
    json!({
        "name": name,
        "type": contact_type,
        "flags": flags,
        "path_len": path_len,
        "last_advert": last_advert,
        "lat_micro": lat_micro,
        "lon_micro": lon_micro,
        "lastmod": lastmod,
    })
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS radios (
    id INTEGER PRIMARY KEY,
    transport TEXT NOT NULL,
    address TEXT NOT NULL,
    label TEXT NOT NULL,
    role TEXT NOT NULL DEFAULT 'companion',
    last_error TEXT NOT NULL DEFAULT '',
    device_json TEXT NOT NULL DEFAULT '{}',
    last_connected INTEGER,
    usb_identity TEXT NOT NULL DEFAULT '',
    UNIQUE(transport, address)
);
CREATE TABLE IF NOT EXISTS nodes (
    pubkey TEXT PRIMARY KEY,
    name TEXT NOT NULL DEFAULT '',
    requested_name TEXT,
    contact_type INTEGER NOT NULL DEFAULT 0,
    flags INTEGER NOT NULL DEFAULT 0,
    path_len INTEGER NOT NULL DEFAULT -1,
    hop_bytes BLOB NOT NULL DEFAULT x'',
    last_advert INTEGER,
    lat REAL,
    lon REAL,
    site_lat REAL,
    site_lon REAL,
    lastmod INTEGER,
    enrollment TEXT NOT NULL DEFAULT 'heard',
    prefix TEXT NOT NULL DEFAULT '',
    last_heard INTEGER,
    raw_json TEXT NOT NULL DEFAULT '{}',
    last_radio_id INTEGER
);
CREATE TABLE IF NOT EXISTS node_radios (
    pubkey TEXT NOT NULL,
    radio_id INTEGER NOT NULL,
    last_heard INTEGER NOT NULL,
    PRIMARY KEY (pubkey, radio_id)
);
CREATE INDEX IF NOT EXISTS node_radios_radio ON node_radios(radio_id);
CREATE TABLE IF NOT EXISTS messages (
    id INTEGER PRIMARY KEY,
    dedupe_key TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL,
    direction TEXT NOT NULL,
    sender_pubkey TEXT,
    sender_prefix TEXT,
    channel_idx INTEGER,
    hop_count INTEGER,
    sender_timestamp INTEGER NOT NULL,
    text TEXT NOT NULL,
    display_text TEXT NOT NULL,
    snr REAL,
    signature BLOB,
    raw_json TEXT NOT NULL DEFAULT '{}',
    delivery TEXT NOT NULL,
    received_at INTEGER NOT NULL,
    radio_id INTEGER,
    origin TEXT NOT NULL DEFAULT 'local',
    mentions_hq INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS messages_time ON messages(received_at);
CREATE INDEX IF NOT EXISTS messages_sender ON messages(sender_pubkey);
CREATE TABLE IF NOT EXISTS paths (
    id INTEGER PRIMARY KEY,
    message_id INTEGER,
    node_pubkey TEXT,
    source TEXT NOT NULL,
    path BLOB NOT NULL,
    hash_size INTEGER NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS paths_message ON paths(message_id);
CREATE TABLE IF NOT EXISTS positions (
    id INTEGER PRIMARY KEY,
    pubkey TEXT NOT NULL,
    lat REAL NOT NULL,
    lon REAL NOT NULL,
    source TEXT NOT NULL,
    at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS positions_time ON positions(at);
CREATE INDEX IF NOT EXISTS positions_pubkey_time ON positions(pubkey, at);
CREATE TABLE IF NOT EXISTS tracked_nodes (
    pubkey TEXT PRIMARY KEY,
    label TEXT NOT NULL DEFAULT '',
    notes TEXT NOT NULL DEFAULT '',
    added_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS blocked_nodes (
    pubkey TEXT PRIMARY KEY,
    label TEXT NOT NULL DEFAULT '',
    added_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS claimed_infra (
    pubkey TEXT PRIMARY KEY,
    kind TEXT NOT NULL DEFAULT 'repeater',
    label TEXT NOT NULL DEFAULT '',
    notes TEXT NOT NULL DEFAULT '',
    added_at INTEGER NOT NULL,
    verified_at INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS telemetry_samples (
    id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL,
    value_mv REAL,
    value_pct REAL,
    at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS telemetry_kind_time ON telemetry_samples(kind, at);
CREATE TABLE IF NOT EXISTS channels (
    name TEXT PRIMARY KEY,
    slot INTEGER NOT NULL,
    secret_account TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS outbound (
    id INTEGER PRIMARY KEY,
    pubkey TEXT NOT NULL,
    body TEXT NOT NULL,
    display_text TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    sent_at INTEGER,
    delivery TEXT NOT NULL,
    expected_ack TEXT,
    error TEXT
);
CREATE TABLE IF NOT EXISTS audit (
    id INTEGER PRIMARY KEY,
    at INTEGER NOT NULL,
    action TEXT NOT NULL,
    pubkey TEXT NOT NULL DEFAULT '',
    detail TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS service_events (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    body TEXT NOT NULL,
    detail TEXT NOT NULL DEFAULT '{}',
    status TEXT NOT NULL DEFAULT 'pending',
    created_at INTEGER NOT NULL,
    decided_at INTEGER
);
CREATE INDEX IF NOT EXISTS service_events_status ON service_events(status, created_at);
CREATE TABLE IF NOT EXISTS bot_handled (
    id TEXT PRIMARY KEY,
    at INTEGER NOT NULL
);
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::Secrets;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use std::thread;

    static TEST_SEQ: AtomicU64 = AtomicU64::new(0);

    fn test_db() -> (Db, Secrets, std::path::PathBuf) {
        let conn = Connection::open_in_memory().unwrap();
        let db = Db::from_conn(conn).unwrap();
        db.ensure_defaults().unwrap();
        let seq = TEST_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "metis-db-{}-{}-{}",
            std::process::id(),
            unix_now(),
            seq
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let secrets = Secrets::file_only(&dir);
        (db, secrets, dir)
    }

    fn temp_db_path(label: &str) -> PathBuf {
        let seq = TEST_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "metis-wal-{}-{}-{}-{}",
            label,
            std::process::id(),
            unix_now(),
            seq
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("metis.db")
    }

    #[test]
    fn open_sets_wal_journal_mode() {
        let path = temp_db_path("open");
        let db = Db::open(&path).unwrap();
        let mode: String = db
            .lock()
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        assert_eq!(mode.to_ascii_lowercase(), "wal");
        db.set_setting("h3_wal_probe", "1").unwrap();
        // After a write, SQLite typically creates the WAL sidecar beside the main file.
        let wal = PathBuf::from(format!("{}-wal", path.display()));
        assert!(
            wal.is_file() || path.is_file(),
            "expected metis.db (and usually metis.db-wal) after a write"
        );
        drop(db);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn concurrent_writes_survive() {
        let path = temp_db_path("conc");
        let db = Arc::new(Db::open(&path).unwrap());
        let mut handles = Vec::new();
        for t in 0..8 {
            let db = Arc::clone(&db);
            handles.push(thread::spawn(move || {
                for i in 0..40 {
                    db.set_setting(&format!("h3_t{t}_i{i}"), "ok")
                        .expect("setting write");
                    db.audit("h3_conc", "", &format!("{t}:{i}"))
                        .expect("audit write");
                }
            }));
        }
        for handle in handles {
            handle.join().expect("worker panicked");
        }
        let mode: String = db
            .lock()
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        assert_eq!(mode.to_ascii_lowercase(), "wal");
        drop(db);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    fn queued_bodies(db: &Db, pubkey: &str) -> Vec<(String, String)> {
        let conn = db.lock();
        let mut stmt = conn
            .prepare(
                "SELECT body, display_text FROM outbound
                 WHERE pubkey = ?1 AND delivery = 'queued'
                 ORDER BY id ASC",
            )
            .unwrap();
        stmt.query_map(params![pubkey], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap()
    }

    #[test]
    fn duplicate_message_is_ignored() {
        let (db, _, dir) = test_db();
        let raw = serde_json::json!({});
        let first = db
            .insert_message(
                "direct", "rx", Some("aa"), "aa", None, Some(1), 10, "signup", "signup",
                None, None, &raw, "received", None,
            )
            .unwrap();
        let second = db
            .insert_message(
                "direct", "rx", Some("aa"), "aa", None, Some(1), 10, "signup", "signup",
                None, None, &raw, "received", None,
            )
            .unwrap();
        assert!(first.is_some());
        assert!(second.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn purge_keeps_roster() {
        let (db, secrets, dir) = test_db();
        db.upsert_contact("abcd", "Ada", 1, 0, -1, &[], 1, 37_000_000, -122_000_000, 1, &json!({}), true, None)
            .unwrap();
        db.approve("abcd", &secrets).unwrap();
        db.insert_message(
            "direct", "rx", Some("abcd"), "abcd", None, Some(0), 2, "signup", "signup",
            None, None, &json!({}), "received", None,
        )
        .unwrap();
        let preview = db.purge(unix_now() + 10).unwrap();
        assert!(preview.messages >= 1);
        let nodes = db.list_nodes().unwrap();
        assert_eq!(nodes[0].enrollment, Enrollment::Enrolled);
        assert!(db.list_messages(10).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn forget_radio_clears_primary_and_list() {
        let (db, _, dir) = test_db();
        let id = db
            .remember_radio("usb", "/dev/ttyACM0", "Test USB")
            .unwrap();
        db.set_primary_radio_id(id).unwrap();
        assert_eq!(db.primary_radio_id().unwrap(), Some(id));
        db.forget_radio(id).unwrap();
        assert!(db.list_saved_radios().unwrap().is_empty());
        assert_eq!(db.primary_radio_id().unwrap(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remap_radio_address_keeps_id_and_identity() {
        let (db, _, dir) = test_db();
        let id = db
            .remember_radio_with_identity(
                "usb",
                "/dev/ttyACM0",
                "Test USB",
                "vid:303a:pid:1001:sn:ABC",
            )
            .unwrap();
        db.set_primary_radio_id(id).unwrap();
        // Stale row that already claimed the stable by-id path.
        let stale = db
            .remember_radio("usb", "/dev/serial/by-id/usb-ABC", "Stale")
            .unwrap();
        assert_ne!(id, stale);
        db.remap_radio_address(id, "/dev/serial/by-id/usb-ABC")
            .unwrap();
        let rows = db.list_saved_radios().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, id);
        assert_eq!(rows[0].address, "/dev/serial/by-id/usb-ABC");
        assert_eq!(rows[0].usb_identity, "vid:303a:pid:1001:sn:ABC");
        assert_eq!(db.primary_radio_id().unwrap(), Some(id));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn zero_coordinate_is_not_a_fix() {
        assert_eq!(coords_from_micro(0, 0), (None, None));
        let (lat, lon) = coords_from_micro(37_774_900, -122_419_400);
        assert!(lat.unwrap() > 37.0);
        assert!(lon.unwrap() < -122.0);
    }

    #[test]
    fn default_channels_include_info_and_net() {
        let (db, _, dir) = test_db();
        let names: Vec<_> = db
            .list_channels()
            .unwrap()
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert!(names.iter().any(|n| n == CHANNEL_NAME));
        assert!(names.iter().any(|n| n == NET_CHANNEL_NAME));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn approve_queues_both_channel_secrets() {
        let (db, secrets, dir) = test_db();
        db.upsert_contact("aa11", "Ada", 1, 0, -1, &[], 1, 0, 0, 1, &json!({}), true, None)
            .unwrap();
        db.set_enrollment("aa11", Enrollment::Pending, None).unwrap();
        db.approve("aa11", &secrets).unwrap();
        let queued = queued_bodies(&db, "aa11");
        assert_eq!(queued.len(), 2);
        assert!(queued.iter().all(|(_, display)| display == REDACTED_SECRET));
        assert!(queued[0].0.contains(CHANNEL_NAME));
        assert!(queued[1].0.contains(NET_CHANNEL_NAME));
        assert!(secrets.load(CHANNEL_NAME).unwrap().is_some());
        assert!(secrets.load(NET_CHANNEL_NAME).unwrap().is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn revoke_rotates_both_and_resends_to_remaining() {
        let (db, secrets, dir) = test_db();
        for (key, name) in [("aa11", "Ada"), ("bb22", "Bob")] {
            db.upsert_contact(key, name, 1, 0, -1, &[], 1, 0, 0, 1, &json!({}), true, None)
                .unwrap();
            db.set_enrollment(key, Enrollment::Pending, None).unwrap();
            db.approve(key, &secrets).unwrap();
        }
        let info_before = secrets.load(CHANNEL_NAME).unwrap().unwrap();
        let net_before = secrets.load(NET_CHANNEL_NAME).unwrap().unwrap();
        // Clear outbound so we can count revoke re-sends alone.
        db.lock()
            .execute("DELETE FROM outbound", [])
            .unwrap();
        let remaining = db.revoke("aa11", &secrets).unwrap();
        assert_eq!(remaining, 1);
        assert_ne!(secrets.load(CHANNEL_NAME).unwrap().unwrap(), info_before);
        assert_ne!(secrets.load(NET_CHANNEL_NAME).unwrap().unwrap(), net_before);
        let bob = queued_bodies(&db, "bb22");
        assert_eq!(bob.len(), 2);
        assert!(queued_bodies(&db, "aa11").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rename_info_migrates_secret_and_alerts_use_bot_home() {
        let (db, secrets, dir) = test_db();
        db.upsert_contact("aa11", "Ada", 1, 0, -1, &[], 1, 0, 0, 1, &json!({}), true, None)
            .unwrap();
        db.set_enrollment("aa11", Enrollment::Pending, None).unwrap();
        db.approve("aa11", &secrets).unwrap();
        let secret = secrets.load(CHANNEL_NAME).unwrap().unwrap();
        db.lock()
            .execute("DELETE FROM outbound", [])
            .unwrap();
        let (new_name, _slot) = db
            .rename_system_channel(SystemChannel::Info, "HQ-BOT", &secrets)
            .unwrap();
        assert_eq!(new_name, "HQ-BOT");
        assert_eq!(db.info_channel_name().unwrap(), "HQ-BOT");
        assert_eq!(secrets.load("HQ-BOT").unwrap(), Some(secret));
        assert!(secrets.load(CHANNEL_NAME).unwrap().is_none());
        assert!(db
            .list_channels()
            .unwrap()
            .iter()
            .any(|c| c.name == "HQ-BOT" && c.role == "info"));
        assert_eq!(
            crate::services::alerts_channel_slot(&db),
            db.list_channels()
                .unwrap()
                .into_iter()
                .find(|c| c.name == "HQ-BOT")
                .unwrap()
                .slot as u8
        );
        let queued = queued_bodies(&db, "aa11");
        assert_eq!(queued.len(), 1);
        assert!(queued[0].0.contains("HQ-BOT"));
        assert_eq!(queued[0].1, REDACTED_SECRET);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn live_upsert_marks_unnoticed_until_opened() {
        let (db, _, dir) = test_db();
        db.upsert_contact("aa11", "Ada", 1, 0, -1, &[], 1, 0, 0, 1, &json!({}), false, None)
            .unwrap();
        assert_eq!(db.count_unnoticed_nodes().unwrap(), 1);
        assert!(!db.get_node("aa11").unwrap().unwrap().noticed);
        db.mark_node_noticed("aa11").unwrap();
        assert_eq!(db.count_unnoticed_nodes().unwrap(), 0);
        assert!(db.get_node("aa11").unwrap().unwrap().noticed);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn quiet_upsert_stays_noticed() {
        let (db, _, dir) = test_db();
        db.upsert_contact("aa11", "Ada", 1, 0, -1, &[], 1, 0, 0, 1, &json!({}), true, None)
            .unwrap();
        assert_eq!(db.count_unnoticed_nodes().unwrap(), 0);
        assert!(db.get_node("aa11").unwrap().unwrap().noticed);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn channel_unread_ignores_outbound_tx() {
        let (db, _, dir) = test_db();
        db.ensure_channel_row("Public", 0, "public", "general").unwrap();
        let raw = json!({});
        let rx1 = db
            .insert_message(
                "channel", "rx", Some("aa11"), "aa11", Some(0), Some(1), 10, "hello", "hello",
                None, None, &raw, "received", None,
            )
            .unwrap()
            .unwrap();
        let _tx = db
            .insert_message(
                "channel", "tx", None, "HQ", Some(0), Some(0), 11, "bot reply", "bot reply",
                None, None, &raw, "sent", None,
            )
            .unwrap()
            .unwrap();
        let rx2 = db
            .insert_message(
                "channel", "rx", Some("bb22"), "bb22", Some(0), Some(1), 12, "again", "again",
                None, None, &raw, "received", None,
            )
            .unwrap()
            .unwrap();
        db.set_channel_last_read(0, rx1).unwrap();
        let channels = db.list_channels().unwrap();
        let public = channels.iter().find(|c| c.slot == 0).unwrap();
        // Only rx2 is unread — the HQ TX between them must not inflate the badge.
        assert_eq!(public.unread_count, 1, "tx must not count as unread");
        assert_eq!(db.total_channel_unread().unwrap(), 1);
        db.set_channel_last_read(0, rx2).unwrap();
        assert_eq!(db.total_channel_unread().unwrap(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dm_thread_unread_clears_after_last_read() {
        let (db, _, dir) = test_db();
        db.upsert_contact("aa11", "Ada", 1, 0, -1, &[], 1, 0, 0, 1, &json!({}), true, None)
            .unwrap();
        let raw = json!({});
        let id1 = db
            .insert_message(
                "direct", "rx", Some("aa11"), "aa11", None, Some(1), 10, "hi", "hi",
                None, None, &raw, "received", None,
            )
            .unwrap()
            .unwrap();
        let id2 = db
            .insert_message(
                "direct", "rx", Some("aa11"), "aa11", None, Some(1), 11, "again", "again",
                None, None, &raw, "received", None,
            )
            .unwrap()
            .unwrap();
        // No cursor yet → unread stays 0 (same as channels: first open marks caught up).
        let threads = db.list_dm_threads().unwrap();
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].message_count, 2);
        assert_eq!(threads[0].unread_count, 0);
        db.set_dm_last_read("aa11", id1).unwrap();
        let threads = db.list_dm_threads().unwrap();
        assert_eq!(threads[0].unread_count, 1);
        db.set_dm_last_read("aa11", id2).unwrap();
        assert_eq!(db.total_dm_unread().unwrap(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn block_node_drops_and_unblocks() {
        let (db, _, dir) = test_db();
        assert!(!db.is_blocked("aa11").unwrap());
        db.block_node("aa11", "Ada").unwrap();
        assert!(db.is_blocked("aa11").unwrap());
        assert_eq!(db.list_blocked_nodes().unwrap()[0].label, "Ada");
        db.unblock_node("aa11").unwrap();
        assert!(!db.is_blocked("aa11").unwrap());
        assert!(db.list_blocked_nodes().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn count_messages_kind_totals() {
        let (db, _, dir) = test_db();
        let raw = json!({});
        assert_eq!(db.count_messages_kind("channel").unwrap(), 0);
        assert_eq!(db.count_messages_kind("direct").unwrap(), 0);
        db.insert_message(
            "channel", "rx", None, "Ada", Some(0), Some(0), 1, "hi", "Ada: hi",
            None, None, &raw, "received", None,
        )
        .unwrap();
        db.insert_message(
            "channel", "rx", None, "Bob", Some(0), Some(0), 2, "yo", "Bob: yo",
            None, None, &raw, "received", None,
        )
        .unwrap();
        db.insert_message(
            "direct", "rx", Some("aa11"), "aa11", None, Some(1), 3, "dm", "dm",
            None, None, &raw, "received", None,
        )
        .unwrap();
        assert_eq!(db.count_messages_kind("channel").unwrap(), 2);
        assert_eq!(db.count_messages_kind("direct").unwrap(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn block_sender_by_channel_name_and_promote() {
        let (db, _, dir) = test_db();
        db.block_sender(None, "Ry-in-escondido 🚜").unwrap();
        let nk = name_block_key("Ry-in-escondido🚜").unwrap();
        assert!(db.is_blocked(&nk).unwrap());
        assert!(db
            .is_channel_sender_blocked(Some("Ry-in-escondido🚜"), None)
            .unwrap());
        // Later advert promotes the real pubkey while keeping the name block.
        db.upsert_contact(
            "bb22",
            "Ry-in-escondido🚜",
            1,
            0,
            -1,
            &[],
            1,
            0,
            0,
            1,
            &json!({}),
            true,
            None,
        )
        .unwrap();
        assert!(db.is_blocked("bb22").unwrap());
        db.unblock_node(&nk).unwrap();
        db.unblock_node("bb22").unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clear_dm_thread_removes_messages_and_unread() {
        let (db, _, dir) = test_db();
        db.upsert_contact("aa11", "Ada", 1, 0, -1, &[], 1, 0, 0, 1, &json!({}), true, None)
            .unwrap();
        let raw = json!({});
        let id1 = db
            .insert_message(
                "direct", "rx", Some("aa11"), "aa11", None, Some(1), 10, "hi", "hi",
                None, None, &raw, "received", None,
            )
            .unwrap()
            .unwrap();
        db.set_dm_last_read("aa11", id1).unwrap();
        assert_eq!(db.list_dm_threads().unwrap().len(), 1);
        assert_eq!(db.clear_dm_thread("aa11").unwrap(), 1);
        assert!(db.list_dm_threads().unwrap().is_empty());
        assert!(db.list_dm_messages("aa11", 50).unwrap().0.is_empty());
        assert_eq!(db.dm_last_read("aa11").unwrap(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn claim_infra_verified_sets_kind_and_verified_at() {
        let (db, _, dir) = test_db();
        // contact_type 2 → repeater, 3 → room, 4 → sensor
        db.upsert_contact("rep01", "P1", 2, 0, -1, &[], 1, 0, 0, 1, &json!({}), true, None)
            .unwrap();
        db.upsert_contact("room1", "Room", 3, 0, -1, &[], 1, 0, 0, 1, &json!({}), true, None)
            .unwrap();
        db.upsert_contact("sens1", "Sense", 4, 0, -1, &[], 1, 0, 0, 1, &json!({}), true, None)
            .unwrap();
        db.claim_infra_verified("rep01", None, "P1", "").unwrap();
        db.claim_infra_verified("room1", None, "Room", "").unwrap();
        db.claim_infra_verified("sens1", None, "Sense", "").unwrap();
        let rows = db.list_claimed_infra().unwrap();
        assert_eq!(db.count_claimed_infra().unwrap(), 3);
        let kind = |key: &str| {
            rows.iter()
                .find(|r| r.pubkey == key)
                .map(|r| r.kind.as_str())
                .unwrap()
        };
        assert_eq!(kind("rep01"), "repeater");
        assert_eq!(kind("room1"), "room");
        assert_eq!(kind("sens1"), "sensor");
        assert!(rows.iter().all(|r| r.is_verified()));
        db.unclaim_infra("rep01").unwrap();
        assert_eq!(db.count_claimed_infra().unwrap(), 2);
        assert!(!db.claimed_pubkey_set().unwrap().contains("rep01"));
        // Override kind even when roster says otherwise.
        db.claim_infra_verified("rep01", Some("sensor"), "P1", "roof")
            .unwrap();
        let again = db.list_claimed_infra().unwrap();
        let row = again.iter().find(|r| r.pubkey == "rep01").unwrap();
        assert_eq!(row.kind, "sensor");
        assert_eq!(row.notes, "roof");
        assert!(row.is_verified());
        // Simulate unverified legacy row then re-verify.
        db.lock()
            .execute(
                "UPDATE claimed_infra SET verified_at = 0 WHERE pubkey = 'rep01'",
                [],
            )
            .unwrap();
        assert!(!db
            .list_claimed_infra()
            .unwrap()
            .iter()
            .find(|r| r.pubkey == "rep01")
            .unwrap()
            .is_verified());
        db.mark_claimed_verified("rep01").unwrap();
        assert!(db
            .list_claimed_infra()
            .unwrap()
            .iter()
            .find(|r| r.pubkey == "rep01")
            .unwrap()
            .is_verified());
        db.update_claimed_notes("rep01", "checked").unwrap();
        assert_eq!(
            db.list_claimed_infra()
                .unwrap()
                .iter()
                .find(|r| r.pubkey == "rep01")
                .unwrap()
                .notes,
            "checked"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn infra_kind_helpers() {
        assert_eq!(Db::infra_kind_from_contact_type(2), "repeater");
        assert_eq!(Db::infra_kind_from_contact_type(3), "room");
        assert_eq!(Db::infra_kind_from_contact_type(4), "sensor");
        assert_eq!(Db::infra_kind_from_contact_type(1), "repeater");
        assert_eq!(Db::normalize_infra_kind("Room"), "room");
        assert_eq!(Db::normalize_infra_kind("SENSOR"), "sensor");
        assert_eq!(Db::normalize_infra_kind("whatever"), "repeater");
    }

    #[test]
    fn contact_hears_tag_by_radio_without_doubling_nodes() {
        let (db, _, dir) = test_db();
        let a = db.remember_radio("ble", "AA:BB:CC:DD:EE:01", "Radio A").unwrap();
        let b = db.remember_radio("usb", "/dev/ttyACM0", "Radio B").unwrap();
        let raw = json!({});
        db.upsert_contact(
            "deadbeef", "Node", 2, 0, -1, &[], 10, 37_000_000, -122_000_000, 10, &raw, true,
            Some(a),
        )
        .unwrap();
        db.upsert_contact(
            "deadbeef", "Node", 2, 0, -1, &[], 20, 37_000_000, -122_000_000, 20, &raw, true,
            Some(b),
        )
        .unwrap();
        let nodes = db.list_nodes().unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].last_radio_id, Some(b));
        let heard = db.list_node_radios("deadbeef").unwrap();
        assert_eq!(heard.len(), 2);
        assert!(heard.iter().any(|(id, _)| *id == a));
        assert!(heard.iter().any(|(id, _)| *id == b));
        assert_eq!(db.radio_label(a).unwrap().as_deref(), Some("Radio A"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn message_dedupe_ignores_radio_id() {
        let (db, _, dir) = test_db();
        let raw = json!({});
        let first = db
            .insert_message(
                "direct", "rx", Some("aa"), "aa", None, Some(1), 10, "hi", "hi",
                None, None, &raw, "received", Some(1),
            )
            .unwrap();
        let second = db
            .insert_message(
                "direct", "rx", Some("aa"), "aa", None, Some(1), 10, "hi", "hi",
                None, None, &raw, "received", Some(2),
            )
            .unwrap();
        assert!(first.is_some());
        assert!(second.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn contacts_since_is_per_radio() {
        let (db, _, dir) = test_db();
        let a = db.remember_radio("ble", "AA:BB:CC:DD:EE:01", "A").unwrap();
        let b = db.remember_radio("usb", "/dev/ttyACM0", "B").unwrap();
        db.set_setting("contacts_since", "42").unwrap();
        assert_eq!(db.contacts_since_for_radio(a).unwrap(), 42);
        assert_eq!(db.contacts_since_for_radio(b).unwrap(), 42);
        db.set_contacts_since_for_radio(a, 99).unwrap();
        assert_eq!(db.contacts_since_for_radio(a).unwrap(), 99);
        assert_eq!(db.contacts_since_for_radio(b).unwrap(), 42);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn channel_message_window_returns_newest_and_before() {
        let (db, _, dir) = test_db();
        let raw = json!({});
        let mut ids = Vec::new();
        for i in 0..5 {
            let id = db
                .insert_message(
                    "channel",
                    "rx",
                    Some("aa"),
                    "aa",
                    Some(0),
                    Some(1),
                    100 + i,
                    &format!("m{i}"),
                    &format!("m{i}"),
                    None,
                    None,
                    &raw,
                    "received",
                    None,
                )
                .unwrap()
                .unwrap();
            ids.push(id);
        }
        let (page, has_older) = db.list_channel_messages(0, 2).unwrap();
        assert!(has_older);
        assert_eq!(page.len(), 2);
        assert_eq!(page[0].id, ids[3]);
        assert_eq!(page[1].id, ids[4]);
        let (older, still) = db
            .list_channel_messages_before(0, page[0].id, 2)
            .unwrap();
        assert!(still);
        assert_eq!(older.len(), 2);
        assert_eq!(older[0].id, ids[1]);
        assert_eq!(older[1].id, ids[2]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mentions_hq_sets_flag_and_unread_count() {
        let (db, _, dir) = test_db();
        db.set_setting("hq_name", "MetisHQ").unwrap();
        let raw = json!({});
        let id1 = db
            .insert_message(
                "channel",
                "rx",
                Some("aa"),
                "aa",
                Some(0),
                Some(1),
                20,
                "hello @[MetisHQ]",
                "hello @[MetisHQ]",
                None,
                None,
                &raw,
                "received",
                None,
            )
            .unwrap()
            .unwrap();
        let _id2 = db
            .insert_message(
                "channel",
                "rx",
                Some("aa"),
                "aa",
                Some(0),
                Some(1),
                21,
                "no mention",
                "no mention",
                None,
                None,
                &raw,
                "received",
                None,
            )
            .unwrap()
            .unwrap();
        let flagged: i64 = db
            .lock()
            .query_row(
                "SELECT mentions_hq FROM messages WHERE id = ?1",
                params![id1],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(flagged, 1);
        db.set_channel_last_read(0, id1 - 1).unwrap();
        let public = db
            .list_channels()
            .unwrap()
            .into_iter()
            .find(|c| c.slot == 0)
            .unwrap();
        assert_eq!(public.unread_mentions, 1);
        assert_eq!(db.total_unread_mentions().unwrap(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn messages_appearance_global_and_channel_override() {
        let (db, _, dir) = test_db();
        let mut global = crate::messages_appearance::MessagesAppearance::default();
        global.bg_opacity = 30;
        global.text_size = crate::messages_appearance::TextSize::Lg;
        db.set_messages_appearance(&global).unwrap();
        let loaded = db.messages_appearance().unwrap();
        assert_eq!(loaded.bg_opacity, 30);
        assert_eq!(loaded.text_size, crate::messages_appearance::TextSize::Lg);

        let mut over = global.clone();
        over.bg_opacity = 10;
        over.bg_color = "#112233".into();
        db.set_channel_appearance("Public", Some(&over)).unwrap();
        let resolved = db.resolved_appearance("Public").unwrap();
        assert_eq!(resolved.bg_opacity, 10);
        assert_eq!(resolved.bg_color, "#112233");
        db.set_channel_appearance("Public", None).unwrap();
        let back = db.resolved_appearance("Public").unwrap();
        assert_eq!(back.bg_opacity, 30);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mark_all_noticed_and_prune_heard() {
        let (db, secrets, dir) = test_db();
        db.upsert_contact("aa11", "Ada", 1, 0, -1, &[], 1, 0, 0, 1, &json!({}), false, None)
            .unwrap();
        db.upsert_contact("bb22", "Bob", 1, 0, -1, &[], 1, 0, 0, 1, &json!({}), false, None)
            .unwrap();
        db.upsert_contact("cc33", "Cara", 1, 0, -1, &[], 1, 0, 0, 1, &json!({}), false, None)
            .unwrap();
        db.set_enrollment("bb22", Enrollment::Pending, None).unwrap();
        db.approve("cc33", &secrets).unwrap();
        assert_eq!(db.count_unnoticed_nodes().unwrap(), 3);
        assert_eq!(db.mark_all_nodes_noticed().unwrap(), 3);
        assert_eq!(db.count_unnoticed_nodes().unwrap(), 0);
        db.add_tracked_node("aa11", "Ada", "").unwrap();
        db.block_node("aa11", "Ada").unwrap();
        assert_eq!(db.count_heard_only_nodes().unwrap(), 1);
        assert_eq!(db.prune_heard_nodes().unwrap(), 1);
        assert!(db.get_node("aa11").unwrap().is_none());
        assert!(db.get_node("bb22").unwrap().is_some());
        assert!(db.get_node("cc33").unwrap().is_some());
        assert!(!db.is_blocked("aa11").unwrap());
        assert!(db.list_tracked_nodes().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
