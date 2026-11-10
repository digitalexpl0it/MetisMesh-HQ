//! Encrypted USB node identity kits (`.metisnode`).
//!
//! Separate from HQ `.metisbak`. Holds private/public keys and a small CLI
//! snapshot harvested over USB serial only — never over LoRa.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use argon2::Argon2;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use rand::RngCore;
use serde::{Deserialize, Serialize};

use crate::db::unix_now;

const MAGIC: &[u8; 8] = b"METISND1";
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;
const FORMAT: &str = "metis-node-kit";
const VERSION: u32 = 1;
const BAUD: u32 = 115_200;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeKitPayload {
    pub format: String,
    pub version: u32,
    pub created_at: i64,
    pub app_version: String,
    pub source: String,
    pub public_key: String,
    pub private_key: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub role_hint: String,
    #[serde(default)]
    pub board: String,
    #[serde(default)]
    pub radio: String,
    #[serde(default)]
    pub lat: String,
    #[serde(default)]
    pub lon: String,
    #[serde(default)]
    pub advert_interval: String,
    #[serde(default)]
    pub path_hash_mode: String,
    #[serde(default)]
    pub guest_password: String,
    /// Optional claimed_infra pubkey link (same as public_key when claimed).
    #[serde(default)]
    pub claimed_pubkey: String,
    /// Optional operator-supplied admin password (never scraped from the device).
    #[serde(default)]
    pub admin_password: String,
}

#[derive(Debug, Clone)]
pub struct NodeKitMeta {
    pub path: PathBuf,
    pub file_name: String,
    pub created_at: i64,
    pub name: String,
    pub public_key_prefix: String,
}

#[derive(Debug, thiserror::Error)]
pub enum NodeKitError {
    #[error("{0}")]
    Msg(String),
    #[error("wrong passphrase or damaged kit")]
    Decrypt,
    #[error("unsupported kit format")]
    Format,
}

impl From<String> for NodeKitError {
    fn from(value: String) -> Self {
        Self::Msg(value)
    }
}

impl From<&str> for NodeKitError {
    fn from(value: &str) -> Self {
        Self::Msg(value.to_string())
    }
}

fn derive_key(passphrase: &str, salt: &[u8]) -> Result<[u8; KEY_LEN], NodeKitError> {
    if passphrase.trim().is_empty() {
        return Err(NodeKitError::Msg("passphrase required".into()));
    }
    if salt.len() < 8 {
        return Err(NodeKitError::Msg("salt too short".into()));
    }
    let mut key = [0u8; KEY_LEN];
    Argon2::default()
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|e| NodeKitError::Msg(format!("key derive failed: {e}")))?;
    Ok(key)
}

fn encrypt_payload(passphrase: &str, plaintext: &[u8]) -> Result<Vec<u8>, NodeKitError> {
    let mut salt = [0u8; SALT_LEN];
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut salt);
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let key = derive_key(passphrase, &salt)?;
    let cipher = ChaCha20Poly1305::new_from_slice(&key)
        .map_err(|e| NodeKitError::Msg(format!("cipher: {e}")))?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|_| NodeKitError::Msg("encrypt failed".into()))?;
    let mut out = Vec::with_capacity(MAGIC.len() + SALT_LEN + NONCE_LEN + ciphertext.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

fn decrypt_payload(passphrase: &str, file: &[u8]) -> Result<Vec<u8>, NodeKitError> {
    if file.len() < MAGIC.len() + SALT_LEN + NONCE_LEN + 16 {
        return Err(NodeKitError::Format);
    }
    if &file[..MAGIC.len()] != MAGIC {
        return Err(NodeKitError::Format);
    }
    let salt = &file[MAGIC.len()..MAGIC.len() + SALT_LEN];
    let nonce_bytes = &file[MAGIC.len() + SALT_LEN..MAGIC.len() + SALT_LEN + NONCE_LEN];
    let ciphertext = &file[MAGIC.len() + SALT_LEN + NONCE_LEN..];
    let key = derive_key(passphrase, salt)?;
    let cipher = ChaCha20Poly1305::new_from_slice(&key)
        .map_err(|e| NodeKitError::Msg(format!("cipher: {e}")))?;
    let nonce = Nonce::from_slice(nonce_bytes);
    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| NodeKitError::Decrypt)
}

pub fn kits_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("node-kits")
}

fn sanitize_label(raw: &str) -> String {
    let s: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() {
        "node".into()
    } else {
        s.chars().take(40).collect()
    }
}

pub fn default_kit_path(data_dir: &Path, name: &str, public_key: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let prefix: String = public_key
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .take(8)
        .collect::<String>()
        .to_ascii_lowercase();
    let label = sanitize_label(if name.trim().is_empty() { "node" } else { name });
    kits_dir(data_dir).join(format!("{label}-{prefix}-{stamp}.metisnode"))
}

pub fn encrypt_kit_bytes(
    payload: &NodeKitPayload,
    passphrase: &str,
) -> Result<Vec<u8>, NodeKitError> {
    if payload.format != FORMAT {
        return Err(NodeKitError::Msg("invalid kit format field".into()));
    }
    validate_keys(&payload.public_key, &payload.private_key)?;
    let json = serde_json::to_vec(payload).map_err(|e| NodeKitError::Msg(e.to_string()))?;
    encrypt_payload(passphrase, &json)
}

pub fn decrypt_kit_bytes(file: &[u8], passphrase: &str) -> Result<NodeKitPayload, NodeKitError> {
    let plain = decrypt_payload(passphrase, file)?;
    let payload: NodeKitPayload =
        serde_json::from_slice(&plain).map_err(|_| NodeKitError::Format)?;
    if payload.format != FORMAT || payload.version != VERSION {
        return Err(NodeKitError::Format);
    }
    validate_keys(&payload.public_key, &payload.private_key)?;
    Ok(payload)
}

fn validate_keys(public_key: &str, private_key: &str) -> Result<(), NodeKitError> {
    let pub_h = normalize_hex(public_key);
    let prv_h = normalize_hex(private_key);
    if pub_h.len() < 32 {
        return Err(NodeKitError::Msg("public key missing or too short".into()));
    }
    if prv_h.len() < 32 {
        return Err(NodeKitError::Msg("private key missing or too short".into()));
    }
    Ok(())
}

pub fn normalize_hex(raw: &str) -> String {
    raw.chars()
        .filter(|c| c.is_ascii_hexdigit())
        .collect::<String>()
        .to_ascii_lowercase()
}

pub fn write_kit_file(
    data_dir: &Path,
    payload: &NodeKitPayload,
    passphrase: &str,
) -> Result<PathBuf, NodeKitError> {
    let path = default_kit_path(data_dir, &payload.name, &payload.public_key);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| NodeKitError::Msg(e.to_string()))?;
    }
    let bytes = encrypt_kit_bytes(payload, passphrase)?;
    fs::write(&path, bytes).map_err(|e| NodeKitError::Msg(e.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(path)
}

pub fn read_kit_file(path: &Path, passphrase: &str) -> Result<NodeKitPayload, NodeKitError> {
    let bytes = fs::read(path).map_err(|e| NodeKitError::Msg(e.to_string()))?;
    decrypt_kit_bytes(&bytes, passphrase)
}

/// Peek metadata without passphrase (filename + encrypted file mtime only).
pub fn list_kit_entries(data_dir: &Path) -> Vec<NodeKitMeta> {
    let dir = kits_dir(data_dir);
    let mut entries = fs::read_dir(&dir)
        .ok()
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.extension()
                        .and_then(|x| x.to_str())
                        .is_some_and(|x| x.eq_ignore_ascii_case("metisnode"))
                })
                .map(|path| {
                    let file_name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("kit.metisnode")
                        .to_string();
                    let created_at = created_at_for(&path, &file_name);
                    let (name, prefix) = parse_name_prefix(&file_name);
                    NodeKitMeta {
                        path,
                        file_name,
                        created_at,
                        name,
                        public_key_prefix: prefix,
                    }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    entries.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then_with(|| b.file_name.cmp(&a.file_name))
    });
    entries
}

fn parse_name_prefix(file_name: &str) -> (String, String) {
    let stem = file_name.trim_end_matches(".metisnode");
    let parts: Vec<&str> = stem.rsplitn(3, '-').collect();
    // rsplitn: [stamp, prefix, name...] when three parts
    if parts.len() == 3 {
        let stamp = parts[0];
        let prefix = parts[1];
        let name = parts[2];
        if stamp.parse::<u64>().is_ok() && prefix.len() >= 4 {
            return (name.replace('-', " "), prefix.to_string());
        }
    }
    (stem.to_string(), String::new())
}

fn created_at_for(path: &Path, file_name: &str) -> i64 {
    let stem = file_name.trim_end_matches(".metisnode");
    if let Some(stamp) = stem.rsplit('-').next() {
        if let Ok(secs) = stamp.parse::<i64>() {
            if secs > 1_000_000_000 {
                return secs;
            }
        }
    }
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn delete_kit(path: &Path) -> Result<(), NodeKitError> {
    if !path.is_file() {
        return Err(NodeKitError::Msg("Kit file not found.".into()));
    }
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| NodeKitError::Msg("Invalid kit path.".into()))?;
    if !name.ends_with(".metisnode") {
        return Err(NodeKitError::Msg("Not a Metis node kit.".into()));
    }
    let parent_ok = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        == Some("node-kits");
    if !parent_ok {
        return Err(NodeKitError::Msg(
            "Can only delete kits in data/node-kits.".into(),
        ));
    }
    fs::remove_file(path).map_err(|e| NodeKitError::Msg(e.to_string()))
}

pub fn export_kit_with_dialog(source: &Path) -> Result<PathBuf, NodeKitError> {
    if !source.is_file() {
        return Err(NodeKitError::Msg("Kit file not found.".into()));
    }
    let suggested = source
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("node.metisnode");
    let dest = rfd::FileDialog::new()
        .set_title("Save Metis node kit")
        .set_file_name(suggested)
        .add_filter("Metis node kit", &["metisnode"])
        .save_file()
        .ok_or_else(|| NodeKitError::Msg("Save cancelled.".into()))?;
    fs::copy(source, &dest).map_err(|e| NodeKitError::Msg(e.to_string()))?;
    Ok(dest)
}

pub fn import_kit_with_dialog(data_dir: &Path) -> Result<PathBuf, NodeKitError> {
    let source = rfd::FileDialog::new()
        .set_title("Open Metis node kit")
        .add_filter("Metis node kit", &["metisnode"])
        .pick_file()
        .ok_or_else(|| NodeKitError::Msg("Open cancelled.".into()))?;
    if !source.is_file() {
        return Err(NodeKitError::Msg("Kit file not found.".into()));
    }
    let header = fs::read(&source).map_err(|e| NodeKitError::Msg(e.to_string()))?;
    if header.len() < MAGIC.len() || &header[..MAGIC.len()] != MAGIC {
        return Err(NodeKitError::Msg("Not a Metis node kit file.".into()));
    }
    let dir = kits_dir(data_dir);
    fs::create_dir_all(&dir).map_err(|e| NodeKitError::Msg(e.to_string()))?;
    let base = source
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| n.ends_with(".metisnode"))
        .unwrap_or("imported.metisnode");
    let mut dest = dir.join(base);
    if dest.exists() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let stem = base.trim_end_matches(".metisnode");
        dest = dir.join(format!("{stem}-import-{stamp}.metisnode"));
    }
    fs::copy(&source, &dest).map_err(|e| NodeKitError::Msg(e.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&dest, fs::Permissions::from_mode(0o600));
    }
    Ok(dest)
}

pub fn new_payload_shell(source: &str) -> NodeKitPayload {
    NodeKitPayload {
        format: FORMAT.into(),
        version: VERSION,
        created_at: unix_now(),
        app_version: env!("CARGO_PKG_VERSION").into(),
        source: source.into(),
        public_key: String::new(),
        private_key: String::new(),
        name: String::new(),
        role_hint: String::new(),
        board: String::new(),
        radio: String::new(),
        lat: String::new(),
        lon: String::new(),
        advert_interval: String::new(),
        path_hash_mode: String::new(),
        guest_password: String::new(),
        claimed_pubkey: String::new(),
        admin_password: String::new(),
    }
}

/// Public metadata for LAN list API (no secrets).
pub fn kit_list_json(data_dir: &Path) -> Vec<serde_json::Value> {
    list_kit_entries(data_dir)
        .into_iter()
        .map(|e| {
            serde_json::json!({
                "id": e.file_name,
                "name": e.name,
                "public_key_prefix": e.public_key_prefix,
                "created_at": e.created_at,
                "file_name": e.file_name,
            })
        })
        .collect()
}

pub fn kit_path_by_name(data_dir: &Path, file_name: &str) -> Result<PathBuf, NodeKitError> {
    let base = Path::new(file_name)
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| NodeKitError::Msg("Invalid kit id.".into()))?;
    if !base.ends_with(".metisnode") || base.contains("..") {
        return Err(NodeKitError::Msg("Invalid kit id.".into()));
    }
    let path = kits_dir(data_dir).join(base);
    if !path.is_file() {
        return Err(NodeKitError::Msg("Kit not found.".into()));
    }
    Ok(path)
}

// --- USB serial harvest / restore ------------------------------------------------

struct SerialSession {
    port: Box<dyn serialport::SerialPort>,
    buf: Vec<u8>,
}

impl SerialSession {
    fn open(port_name: &str) -> Result<Self, NodeKitError> {
        let port = serialport::new(port_name, BAUD)
            .timeout(Duration::from_millis(200))
            .open()
            .map_err(|e| NodeKitError::Msg(format!("Open {port_name}: {e}")))?;
        let mut s = Self {
            port,
            buf: Vec::new(),
        };
        // Clear stale input.
        let _ = s.drain_for(Duration::from_millis(150));
        Ok(s)
    }

    fn drain_for(&mut self, dur: Duration) -> String {
        let deadline = Instant::now() + dur;
        let mut tmp = [0u8; 512];
        while Instant::now() < deadline {
            match self.port.read(&mut tmp) {
                Ok(n) if n > 0 => self.buf.extend_from_slice(&tmp[..n]),
                _ => thread::sleep(Duration::from_millis(20)),
            }
        }
        String::from_utf8_lossy(&self.buf).into_owned()
    }

    fn write_line(&mut self, line: &str) -> Result<(), NodeKitError> {
        let mut data = line.as_bytes().to_vec();
        if !data.ends_with(b"\n") {
            data.push(b'\n');
        }
        self.port
            .write_all(&data)
            .map_err(|e| NodeKitError::Msg(format!("serial write: {e}")))?;
        let _ = self.port.flush();
        Ok(())
    }

    fn cmd(&mut self, command: &str, wait: Duration) -> Result<String, NodeKitError> {
        self.buf.clear();
        self.write_line(command)?;
        let raw = self.drain_for(wait);
        Ok(raw)
    }
}

fn first_hex_blob(text: &str, min_len: usize) -> Option<String> {
    let mut best = String::new();
    let mut cur = String::new();
    for c in text.chars() {
        if c.is_ascii_hexdigit() {
            cur.push(c.to_ascii_lowercase());
        } else {
            if cur.len() >= min_len && cur.len() > best.len() {
                best = cur.clone();
            }
            cur.clear();
        }
    }
    if cur.len() >= min_len && cur.len() > best.len() {
        best = cur;
    }
    if best.is_empty() {
        None
    } else {
        Some(best)
    }
}

fn strip_cli_noise(raw: &str) -> String {
    raw.lines()
        .map(str::trim)
        .filter(|l| {
            !l.is_empty()
                && !l.starts_with('>')
                && !l.eq_ignore_ascii_case("ok")
                && !l.starts_with("→")
        })
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

fn parse_named_value(raw: &str, cmd: &str) -> String {
    let cleaned = strip_cli_noise(raw);
    let cmd_l = cmd.to_ascii_lowercase();
    for line in raw.lines().chain(std::iter::once(cleaned.as_str())) {
        let t = line.trim();
        let lower = t.to_ascii_lowercase();
        if lower.starts_with(&cmd_l) {
            continue;
        }
        if let Some(rest) = lower.strip_prefix("name:") {
            return rest.trim().to_string();
        }
        if t.contains(':') {
            if let Some((_, v)) = t.split_once(':') {
                let v = v.trim();
                if !v.is_empty() && !v.eq_ignore_ascii_case("ok") {
                    return v.to_string();
                }
            }
        }
    }
    if !cleaned.is_empty()
        && !cleaned.to_ascii_lowercase().contains("unknown")
        && cleaned.len() < 120
    {
        return cleaned;
    }
    String::new()
}

/// Harvest identity + settings from a USB MeshCore CLI port.
pub fn harvest_from_usb(
    port_name: &str,
    source: &str,
    role_hint: &str,
    claimed_pubkey: &str,
) -> Result<NodeKitPayload, NodeKitError> {
    let mut s = SerialSession::open(port_name)?;
    let mut payload = new_payload_shell(source);
    payload.role_hint = role_hint.trim().to_string();
    payload.claimed_pubkey = claimed_pubkey.trim().to_ascii_lowercase();

    let name_raw = s.cmd("get name", Duration::from_millis(800))?;
    payload.name = parse_named_value(&name_raw, "get name");

    let pub_raw = s.cmd("get public.key", Duration::from_millis(1200))?;
    payload.public_key = first_hex_blob(&pub_raw, 32).ok_or_else(|| {
        NodeKitError::Msg("Could not read public key (is the node in CLI mode on USB?).".into())
    })?;

    let prv_raw = s.cmd("get prv.key", Duration::from_millis(1500))?;
    payload.private_key = first_hex_blob(&prv_raw, 32).ok_or_else(|| {
        NodeKitError::Msg("Could not read private key. Confirm USB CLI and firmware support.".into())
    })?;

    if let Ok(raw) = s.cmd("board", Duration::from_millis(600)) {
        let v = strip_cli_noise(&raw);
        if !v.is_empty() && v.len() < 80 {
            payload.board = v;
        }
    }
    if let Ok(raw) = s.cmd("get radio", Duration::from_millis(800)) {
        let v = strip_cli_noise(&raw);
        if !v.is_empty() {
            payload.radio = v.chars().take(160).collect();
        }
    }
    if let Ok(raw) = s.cmd("get lat", Duration::from_millis(500)) {
        payload.lat = parse_named_value(&raw, "get lat");
    }
    if let Ok(raw) = s.cmd("get lon", Duration::from_millis(500)) {
        payload.lon = parse_named_value(&raw, "get lon");
    }
    if let Ok(raw) = s.cmd("get advert.interval", Duration::from_millis(500)) {
        payload.advert_interval = parse_named_value(&raw, "get advert.interval");
    }
    if let Ok(raw) = s.cmd("get path.hash.mode", Duration::from_millis(500)) {
        payload.path_hash_mode = parse_named_value(&raw, "get path.hash.mode");
    }
    if let Ok(raw) = s.cmd("get guest.password", Duration::from_millis(500)) {
        let v = parse_named_value(&raw, "get guest.password");
        if !v.is_empty() && !v.eq_ignore_ascii_case("unknown") {
            payload.guest_password = v;
        }
    }

    if payload.claimed_pubkey.is_empty() {
        payload.claimed_pubkey = payload.public_key.clone();
    }
    validate_keys(&payload.public_key, &payload.private_key)?;
    Ok(payload)
}

/// Restore identity (and best-effort settings) over USB serial.
pub fn restore_to_usb(port_name: &str, kit: &NodeKitPayload) -> Result<String, NodeKitError> {
    validate_keys(&kit.public_key, &kit.private_key)?;
    let mut s = SerialSession::open(port_name)?;
    let prv = normalize_hex(&kit.private_key);
    let set_raw = s.cmd(
        &format!("set prv.key {prv}"),
        Duration::from_millis(2000),
    )?;
    let set_l = set_raw.to_ascii_lowercase();
    if !(set_l.contains("ok") || set_l.contains("set")) && set_raw.trim().is_empty() {
        // Some firmwares ACK quietly; continue and verify after reboot.
    } else if set_l.contains("error") || set_l.contains("fail") || set_l.contains("denied") {
        return Err(NodeKitError::Msg(
            "set prv.key failed — check USB console and firmware.".into(),
        ));
    }

    let _ = s.cmd("reboot", Duration::from_millis(500));
    // Port often drops on reboot; reopen after delay.
    drop(s);
    thread::sleep(Duration::from_secs(3));
    let mut s = SerialSession::open(port_name)?;
    let _ = s.drain_for(Duration::from_millis(500));

    if !kit.name.trim().is_empty() {
        let _ = s.cmd(
            &format!("set name {}", kit.name.trim()),
            Duration::from_millis(800),
        );
    }
    if !kit.lat.trim().is_empty() {
        let _ = s.cmd(
            &format!("set lat {}", kit.lat.trim()),
            Duration::from_millis(500),
        );
    }
    if !kit.lon.trim().is_empty() {
        let _ = s.cmd(
            &format!("set lon {}", kit.lon.trim()),
            Duration::from_millis(500),
        );
    }
    if !kit.advert_interval.trim().is_empty() {
        let _ = s.cmd(
            &format!("set advert.interval {}", kit.advert_interval.trim()),
            Duration::from_millis(500),
        );
    }
    if !kit.guest_password.trim().is_empty() {
        let _ = s.cmd(
            &format!("set guest.password {}", kit.guest_password.trim()),
            Duration::from_millis(500),
        );
    }
    if !kit.admin_password.trim().is_empty() {
        let _ = s.cmd(
            &format!("password {}", kit.admin_password.trim()),
            Duration::from_millis(800),
        );
    }

    let pub_raw = s.cmd("get public.key", Duration::from_millis(1500))?;
    let got = first_hex_blob(&pub_raw, 32).unwrap_or_default();
    let expect = normalize_hex(&kit.public_key);
    if got.is_empty() {
        return Ok(
            "Private key sent and reboot issued; could not re-read public key yet — verify with get public.key."
                .into(),
        );
    }
    if got != expect {
        return Err(NodeKitError::Msg(format!(
            "Public key mismatch after restore (got {}…, expected {}…). Re-check kit passphrase / port.",
            &got[..got.len().min(8)],
            &expect[..expect.len().min(8)]
        )));
    }
    Ok(format!(
        "Identity restored — public key matches ({}…).",
        &got[..got.len().min(8)]
    ))
}

pub fn backup_usb_to_disk(
    data_dir: &Path,
    port_name: &str,
    passphrase: &str,
    source: &str,
    role_hint: &str,
    claimed_pubkey: &str,
) -> Result<(PathBuf, NodeKitPayload), NodeKitError> {
    let mut payload = harvest_from_usb(port_name, source, role_hint, claimed_pubkey)?;
    // Don't leave private key in the returned copy used by UI — caller should drop.
    let path = write_kit_file(data_dir, &payload, passphrase)?;
    payload.private_key.clear();
    Ok((path, payload))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_encrypt() {
        let mut p = new_payload_shell("desktop");
        p.public_key = "aa".repeat(32);
        p.private_key = "bb".repeat(32);
        p.name = "Tower".into();
        let bytes = encrypt_kit_bytes(&p, "correct horse").unwrap();
        assert!(bytes.starts_with(MAGIC));
        let back = decrypt_kit_bytes(&bytes, "correct horse").unwrap();
        assert_eq!(back.name, "Tower");
        assert_eq!(back.public_key, p.public_key);
        assert!(decrypt_kit_bytes(&bytes, "wrong").is_err());
    }

    #[test]
    fn write_list_delete() {
        let dir = std::env::temp_dir().join(format!("metis-kit-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut p = new_payload_shell("desktop");
        p.public_key = "abcd".repeat(16);
        p.private_key = "ef01".repeat(16);
        p.name = "R1".into();
        let path = write_kit_file(&dir, &p, "secret-pass").unwrap();
        assert!(path.exists());
        let list = list_kit_entries(&dir);
        assert_eq!(list.len(), 1);
        delete_kit(&path).unwrap();
        assert!(list_kit_entries(&dir).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn first_hex_finds_key() {
        let sample = "> get public.key\r\nAABBCCDDEEFF00112233445566778899aabbccddeeff00112233445566778899\r\n>";
        let h = first_hex_blob(sample, 32).unwrap();
        assert_eq!(h.len(), 64);
    }
}
