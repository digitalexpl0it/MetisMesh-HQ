//! Local history. WAL SQLite, one file for every radio.

use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

use crate::path::dedupe_key;
use crate::secrets::{SecretStoreKind, Secrets, CHANNEL_NAME};
use crate::signup::{approval_message, Enrollment};

const REDACTED_SECRET: &str = "[secret sent]";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("{0}")]
    Msg(String),
}

pub type Result<T> = std::result::Result<T, Error>;

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
    /// Messages newer than `channel_last_read_{slot}`; 0 if never opened.
    pub unread_count: i64,
    pub last_message_at: Option<i64>,
    pub last_text: Option<String>,
    /// Per-channel override: `1h`…`forever`, or `None` to use the global default.
    pub message_keep: Option<String>,
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

#[derive(Debug, Clone)]
pub struct RadioRow {
    #[allow(dead_code)]
    pub id: i64,
    #[allow(dead_code)]
    pub transport: String,
    pub address: String,
    pub label: String,
    pub last_error: String,
    #[allow(dead_code)]
    pub last_connected: Option<i64>,
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
        db.conn
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .pragma_update(None, "journal_mode", "WAL")?;
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
            "ALTER TABLE nodes ADD COLUMN qr_file TEXT",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE nodes ADD COLUMN qr_uri TEXT",
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
            CREATE INDEX IF NOT EXISTS rf_samples_time ON rf_samples(at);",
        );
        let _ = conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS tracked_nodes (
                pubkey TEXT PRIMARY KEY,
                label TEXT NOT NULL DEFAULT '',
                notes TEXT NOT NULL DEFAULT '',
                added_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS positions_pubkey_time ON positions(pubkey, at);",
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
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|err| err.into_inner())
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
        if self.setting("svc_alerts_channel")?.is_none() {
            self.set_setting("svc_alerts_channel", "METIS-INFO")?;
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
        // METIS-INFO: keep its existing slot when free of Public; else next free.
        let metis_slot = {
            let conn = self.lock();
            let existing: Option<i64> = conn
                .query_row(
                    "SELECT slot FROM channels WHERE name = ?1 COLLATE NOCASE",
                    params![CHANNEL_NAME],
                    |row| row.get(0),
                )
                .optional()?;
            match existing {
                Some(slot) if slot != 0 => slot,
                _ => {
                    let taken: Vec<i64> = conn
                        .prepare("SELECT slot FROM channels WHERE COALESCE(active, 1) = 1")?
                        .query_map([], |row| row.get(0))?
                        .collect::<std::result::Result<Vec<_>, _>>()?;
                    if !taken.contains(&1) {
                        1
                    } else {
                        (0..32)
                            .find(|slot| *slot != 0 && !taken.contains(slot))
                            .unwrap_or(1)
                    }
                }
            }
        };
        self.vacate_channel_slot(metis_slot, CHANNEL_NAME)?;
        self.lock().execute(
            "INSERT INTO channels (name, slot, secret_account, visibility, role, active, created_at)
             VALUES (?1, ?2, ?1, 'private', 'info', 1, ?3)
             ON CONFLICT(name) DO UPDATE SET
               slot = excluded.slot,
               visibility = 'private',
               role = 'info',
               active = 1",
            params![CHANNEL_NAME, metis_slot, now],
        )?;
        Ok(())
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
        let metis_slot: Option<i64> = self
            .lock()
            .query_row(
                "SELECT slot FROM channels
                 WHERE name = ?1 COLLATE NOCASE AND COALESCE(active, 1) = 1",
                params![CHANNEL_NAME],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(current) = metis_slot {
            let slot = if current == 0 { 1 } else { current };
            self.vacate_channel_slot(slot, CHANNEL_NAME)?;
            self.lock().execute(
                "UPDATE channels SET slot = ?2 WHERE name = ?1 COLLATE NOCASE",
                params![CHANNEL_NAME, slot],
            )?;
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
            } else if names.iter().any(|n| n.eq_ignore_ascii_case(CHANNEL_NAME)) {
                CHANNEL_NAME.to_string()
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
        let conn = self.lock();
        conn.execute(
            "INSERT INTO radios (transport, address, label, last_error, last_connected)
             VALUES (?1, ?2, ?3, '', NULL)
             ON CONFLICT(transport, address) DO UPDATE SET
               label = CASE
                 WHEN excluded.label = '' OR excluded.label = 'Companion' THEN radios.label
                 ELSE excluded.label
               END",
            params![transport, address, label],
        )?;
        let id = conn.query_row(
            "SELECT id FROM radios WHERE transport = ?1 AND address = ?2",
            params![transport, address],
            |row| row.get(0),
        )?;
        Ok(id)
    }

    pub fn remember_radio(&self, transport: &str, address: &str, label: &str) -> Result<i64> {
        let id = self.upsert_radio(transport, address, label)?;
        self.lock().execute(
            "UPDATE radios SET last_connected = ?2, last_error = '',
               label = CASE
                 WHEN ?3 = '' OR ?3 = 'Companion' THEN label
                 ELSE ?3
               END
             WHERE id = ?1",
            params![id, unix_now(), label],
        )?;
        self.set_setting("radio_name", address)?;
        Ok(id)
    }

    pub fn list_saved_radios(&self) -> Result<Vec<RadioRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, transport, address, label, last_error, last_connected
             FROM radios
             ORDER BY COALESCE(last_connected, 0) DESC, id DESC",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(RadioRow {
                    id: row.get(0)?,
                    transport: row.get(1)?,
                    address: row.get(2)?,
                    label: row.get(3)?,
                    last_error: row.get(4)?,
                    last_connected: row.get(5)?,
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
                    NULLIF(TRIM(COALESCE(c.message_keep, '')), '')
             FROM channels c
             WHERE COALESCE(c.active, 1) = 1
             ORDER BY
               CASE
                 WHEN c.name = 'Public' COLLATE NOCASE THEN 0
                 WHEN c.name = 'METIS-INFO' COLLATE NOCASE THEN 1
                 ELSE 2
               END,
               c.name COLLATE NOCASE",
        )?;
        let rows = stmt
            .query_map([], |row| {
                let keep_raw: Option<String> = row.get(9)?;
                let message_keep = keep_raw.and_then(|v| {
                    Self::normalize_message_keep(&v).map(str::to_string)
                });
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
                    last_message_at: row.get(7)?,
                    last_text: row.get(8)?,
                    message_keep,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn ensure_channel_row(
        &self,
        name: &str,
        slot: i64,
        visibility: &str,
        role: &str,
    ) -> Result<()> {
        // Public is immovable from mesh slot 0.
        let slot = if name.eq_ignore_ascii_case("Public") {
            0
        } else if name.eq_ignore_ascii_case(CHANNEL_NAME) && slot == 0 {
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
        if name.eq_ignore_ascii_case("Public") || name.eq_ignore_ascii_case(CHANNEL_NAME) {
            return Err(Error::Msg("That channel name is reserved.".into()));
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
        if name.eq_ignore_ascii_case(CHANNEL_NAME) {
            return Err(Error::Msg("METIS-INFO stays private.".into()));
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
        if name.eq_ignore_ascii_case("Public") || name.eq_ignore_ascii_case(CHANNEL_NAME) {
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

    /// Import a channel discovered on the companion radio without clobbering METIS-INFO.
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
        if name.eq_ignore_ascii_case(CHANNEL_NAME) {
            self.ensure_channel_row(CHANNEL_NAME, slot, "private", "info")?;
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

    pub fn list_channel_messages(&self, slot: i64, limit: i64) -> Result<Vec<MessageRow>> {
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
             WHERE m.kind = 'channel' AND m.channel_idx = ?1
             ORDER BY m.received_at ASC, m.id ASC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![slot, limit], Self::map_message_row)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Error::from)
    }

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
        conn.execute(
            "INSERT INTO nodes (
                pubkey, name, contact_type, flags, path_len, hop_bytes, last_advert,
                lat, lon, lastmod, enrollment, prefix, last_heard, raw_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
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
                raw_json = excluded.raw_json",
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
                raw.to_string()
            ],
        )?;
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
        Ok(())
    }

    pub fn save_hq(&self, pubkey: &str, name: &str, lat_micro: i32, lon_micro: i32, raw: &Value) -> Result<()> {
        let (lat, lon) = coords_from_micro(lat_micro, lon_micro);
        let prefix = pubkey.chars().take(12).collect::<String>();
        self.lock().execute(
            "INSERT INTO nodes (
                pubkey, name, contact_type, flags, path_len, hop_bytes, last_advert,
                lat, lon, lastmod, enrollment, prefix, last_heard, raw_json
             ) VALUES (?1, ?2, 1, 0, -1, x'', ?3, ?4, ?5, ?3, 'hq', ?6, ?3, ?7)
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
                    last_advert, lat, lon, prefix, hop_bytes, path_len, qr_file, qr_uri
             FROM nodes WHERE pubkey LIKE ?1 || '%'",
        )?;
        let rows = stmt.query_map(params![prefix_hex], map_node)?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Error::from)
    }

    pub fn list_nodes(&self) -> Result<Vec<NodeRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT pubkey, name, requested_name, contact_type, enrollment, last_heard,
                    last_advert, lat, lon, prefix, hop_bytes, path_len, qr_file, qr_uri
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
                    last_advert, lat, lon, prefix, hop_bytes, path_len, qr_file, qr_uri
             FROM nodes WHERE enrollment = 'hq' LIMIT 1",
        )?;
        Ok(stmt.query_row([], map_node).optional()?)
    }

    pub fn get_node(&self, pubkey: &str) -> Result<Option<NodeRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT pubkey, name, requested_name, contact_type, enrollment, last_heard,
                    last_advert, lat, lon, prefix, hop_bytes, path_len, qr_file, qr_uri
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
             WHERE m.sender_pubkey = ?1
             ORDER BY m.received_at DESC, m.id DESC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![pubkey, limit], Self::map_message_row)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Error::from)
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

    pub fn touch_last_heard(&self, pubkey: &str) -> Result<()> {
        self.lock().execute(
            "UPDATE nodes SET last_heard = ?2 WHERE pubkey = ?1",
            params![pubkey, unix_now()],
        )?;
        Ok(())
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
        let conn = self.lock();
        let changed = conn.execute(
            "INSERT OR IGNORE INTO messages (
                dedupe_key, kind, direction, sender_pubkey, sender_prefix, channel_idx,
                hop_count, sender_timestamp, text, display_text, snr, signature, raw_json,
                delivery, received_at, radio_id, origin
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, 'local')",
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
                radio_id
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
        let secret = self.ensure_channel_secret(secrets)?;
        let body = approval_message(CHANNEL_NAME, &hex::encode(secret));
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
            params![pubkey, body, REDACTED_SECRET, unix_now()],
        )?;
        tx.execute(
            "INSERT INTO audit (at, action, pubkey, detail) VALUES (?1, 'approve', ?2, ?3)",
            params![unix_now(), pubkey, CHANNEL_NAME],
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

    /// Removing an enrolled node rotates the secret and re-sends it to the rest.
    pub fn revoke(&self, pubkey: &str, secrets: &Secrets) -> Result<usize> {
        let secret = Secrets::generate();
        let kind = secrets
            .store(CHANNEL_NAME, &secret)
            .map_err(Error::Msg)?;
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
        let body = approval_message(CHANNEL_NAME, &hex::encode(secret));
        for key in &enrolled {
            tx.execute(
                "INSERT INTO outbound (pubkey, body, display_text, created_at, delivery)
                 VALUES (?1, ?2, ?3, ?4, 'queued')",
                params![key, body, REDACTED_SECRET, unix_now()],
            )?;
        }
        tx.execute(
            "INSERT INTO audit (at, action, pubkey, detail) VALUES (?1, 'revoke', ?2, 'rotated channel secret')",
            params![unix_now(), pubkey],
        )?;
        tx.commit()?;
        Ok(enrolled.len())
    }

    pub fn ensure_channel_secret(&self, secrets: &Secrets) -> Result<[u8; 16]> {
        self.ensure_named_channel_secret(CHANNEL_NAME, secrets)
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
        if channel_name == CHANNEL_NAME {
            let generation = self
                .setting("channel_generation")?
                .and_then(|value| value.parse::<i64>().ok())
                .unwrap_or(0)
                + 1;
            self.set_setting("channel_generation", &generation.to_string())?;
            self.lock().execute(
                "INSERT INTO channels (name, slot, secret_account) VALUES (?1, 1, ?1)
                 ON CONFLICT(name) DO NOTHING",
                params![CHANNEL_NAME],
            )?;
        }
        Ok(secret)
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
        if !exists && channel_name != CHANNEL_NAME {
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

#[derive(Debug, Clone)]
pub struct PositionPoint {
    pub pubkey: String,
    pub lat: f64,
    pub lon: f64,
}

fn map_node(row: &rusqlite::Row<'_>) -> rusqlite::Result<NodeRow> {
    let lat: Option<f64> = row.get(7)?;
    let lon: Option<f64> = row.get(8)?;
    let enrollment: String = row.get(4)?;
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

pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
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
    last_error TEXT NOT NULL DEFAULT '',
    device_json TEXT NOT NULL DEFAULT '{}',
    last_connected INTEGER,
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
    raw_json TEXT NOT NULL DEFAULT '{}'
);
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
    origin TEXT NOT NULL DEFAULT 'local'
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

    fn test_db() -> (Db, Secrets) {
        let conn = Connection::open_in_memory().unwrap();
        let db = Db::from_conn(conn).unwrap();
        db.ensure_defaults().unwrap();
        let dir = std::env::temp_dir().join(format!("metis-db-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        (db, Secrets::file_only(dir))
    }

    #[test]
    fn duplicate_message_is_ignored() {
        let (db, _) = test_db();
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
    }

    #[test]
    fn purge_keeps_roster() {
        let (db, secrets) = test_db();
        db.upsert_contact("abcd", "Ada", 1, 0, -1, &[], 1, 37_000_000, -122_000_000, 1, &json!({}))
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
    }

    #[test]
    fn zero_coordinate_is_not_a_fix() {
        assert_eq!(coords_from_micro(0, 0), (None, None));
        let (lat, lon) = coords_from_micro(37_774_900, -122_419_400);
        assert!(lat.unwrap() > 37.0);
        assert!(lon.unwrap() < -122.0);
    }
}
