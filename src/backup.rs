//! Passphrase-encrypted HQ backup (`.metisbak`).
//!
//! Envelope: magic `METISHQ1` + salt + nonce + ChaCha20-Poly1305 ciphertext.
//! Plaintext is versioned JSON with a SQLite snapshot (base64) and channel secrets.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use argon2::Argon2;
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use rand::RngCore;
use serde::{Deserialize, Serialize};

use crate::db::{unix_now, Db};
use crate::secrets::Secrets;

const MAGIC: &[u8; 8] = b"METISHQ1";
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;
const FORMAT: &str = "metis-hq-backup";
const VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupPayload {
    pub format: String,
    pub version: u32,
    pub created_at: i64,
    pub app_version: String,
    pub hq_name: String,
    /// Channel name → 32-char hex of 16-byte secret.
    pub secrets: BTreeMap<String, String>,
    /// Consistent SQLite snapshot (base64).
    pub db_base64: String,
}

#[derive(Debug, Clone)]
pub struct BackupPreview {
    pub created_at: i64,
    pub hq_name: String,
    pub app_version: String,
    pub node_count: i64,
    pub message_count: i64,
    pub channel_count: i64,
    pub secret_count: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error("{0}")]
    Msg(String),
    #[error("wrong passphrase or damaged backup")]
    Decrypt,
    #[error("unsupported backup format")]
    Format,
}

impl From<String> for BackupError {
    fn from(value: String) -> Self {
        Self::Msg(value)
    }
}

impl From<&str> for BackupError {
    fn from(value: &str) -> Self {
        Self::Msg(value.to_string())
    }
}

fn derive_key(passphrase: &str, salt: &[u8]) -> Result<[u8; KEY_LEN], BackupError> {
    if passphrase.trim().is_empty() {
        return Err(BackupError::Msg("passphrase required".into()));
    }
    if salt.len() < 8 {
        return Err(BackupError::Msg("salt too short".into()));
    }
    let mut key = [0u8; KEY_LEN];
    Argon2::default()
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|e| BackupError::Msg(format!("key derive failed: {e}")))?;
    Ok(key)
}

fn encrypt_payload(passphrase: &str, plaintext: &[u8]) -> Result<Vec<u8>, BackupError> {
    let mut salt = [0u8; SALT_LEN];
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut salt);
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let key = derive_key(passphrase, &salt)?;
    let cipher = ChaCha20Poly1305::new_from_slice(&key)
        .map_err(|e| BackupError::Msg(format!("cipher: {e}")))?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|_| BackupError::Msg("encrypt failed".into()))?;
    let mut out = Vec::with_capacity(MAGIC.len() + SALT_LEN + NONCE_LEN + ciphertext.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

fn decrypt_payload(passphrase: &str, file: &[u8]) -> Result<Vec<u8>, BackupError> {
    if file.len() < MAGIC.len() + SALT_LEN + NONCE_LEN + 16 {
        return Err(BackupError::Format);
    }
    if &file[..MAGIC.len()] != MAGIC {
        return Err(BackupError::Format);
    }
    let salt = &file[MAGIC.len()..MAGIC.len() + SALT_LEN];
    let nonce_bytes = &file[MAGIC.len() + SALT_LEN..MAGIC.len() + SALT_LEN + NONCE_LEN];
    let ciphertext = &file[MAGIC.len() + SALT_LEN + NONCE_LEN..];
    let key = derive_key(passphrase, salt)?;
    let cipher = ChaCha20Poly1305::new_from_slice(&key)
        .map_err(|e| BackupError::Msg(format!("cipher: {e}")))?;
    let nonce = Nonce::from_slice(nonce_bytes);
    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| BackupError::Decrypt)
}

fn collect_secrets(db: &Db, secrets: &Secrets) -> Result<BTreeMap<String, String>, BackupError> {
    let mut map = BTreeMap::new();
    for name in db
        .private_channel_names()
        .map_err(|e| BackupError::Msg(e.to_string()))?
    {
        if let Some(bytes) = secrets
            .load(&name)
            .map_err(|e| BackupError::Msg(e))?
        {
            map.insert(name, hex::encode(bytes));
        }
    }
    Ok(map)
}

/// Build encrypted `.metisbak` bytes from the live HQ database + secrets.
pub fn create_backup_bytes(
    db: &Db,
    secrets: &Secrets,
    passphrase: &str,
) -> Result<Vec<u8>, BackupError> {
    let db_bytes = db
        .sqlite_snapshot_bytes()
        .map_err(|e| BackupError::Msg(e.to_string()))?;
    let hq_name = db
        .setting("hq_name")
        .ok()
        .flatten()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "Metis HQ".into());
    let payload = BackupPayload {
        format: FORMAT.into(),
        version: VERSION,
        created_at: unix_now(),
        app_version: env!("CARGO_PKG_VERSION").into(),
        hq_name,
        secrets: collect_secrets(db, secrets)?,
        db_base64: B64.encode(db_bytes),
    };
    let json = serde_json::to_vec(&payload).map_err(|e| BackupError::Msg(e.to_string()))?;
    encrypt_payload(passphrase, &json)
}

pub fn write_backup_file(
    db: &Db,
    secrets: &Secrets,
    passphrase: &str,
    path: &Path,
) -> Result<PathBuf, BackupError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| BackupError::Msg(e.to_string()))?;
    }
    let bytes = create_backup_bytes(db, secrets, passphrase)?;
    fs::write(path, bytes).map_err(|e| BackupError::Msg(e.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    Ok(path.to_path_buf())
}

pub fn default_backup_path(data_dir: &Path) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    data_dir
        .join("backups")
        .join(format!("metis-{stamp}.metisbak"))
}

#[derive(Debug, Clone)]
pub struct BackupEntry {
    pub path: PathBuf,
    /// Best-effort creation time (unix seconds) from filename stamp or mtime.
    pub created_at: i64,
    pub file_name: String,
}

impl BackupEntry {
    /// Dropdown / list label without locale formatting (caller can stamp via timefmt).
    pub fn path_string(&self) -> String {
        self.path.display().to_string()
    }
}

fn created_at_for(path: &Path) -> i64 {
    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
        if let Some(stamp) = stem.strip_prefix("metis-") {
            if let Ok(secs) = stamp.parse::<i64>() {
                if secs > 0 {
                    return secs;
                }
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

pub fn list_backup_entries(data_dir: &Path) -> Vec<BackupEntry> {
    let dir = data_dir.join("backups");
    let mut entries = fs::read_dir(&dir)
        .ok()
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.extension()
                        .and_then(|x| x.to_str())
                        .is_some_and(|x| x.eq_ignore_ascii_case("metisbak"))
                })
                .map(|path| {
                    let file_name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("backup.metisbak")
                        .to_string();
                    let created_at = created_at_for(&path);
                    BackupEntry {
                        path,
                        created_at,
                        file_name,
                    }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    entries.sort_by(|a, b| b.created_at.cmp(&a.created_at).then_with(|| b.file_name.cmp(&a.file_name)));
    entries
}

/// Copy a backup to a path chosen by the operator (native save dialog).
pub fn export_backup_with_dialog(source: &Path) -> Result<PathBuf, BackupError> {
    if !source.is_file() {
        return Err(BackupError::Msg("Backup file not found.".into()));
    }
    let suggested = source
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("metis-backup.metisbak");
    let dest = rfd::FileDialog::new()
        .set_title("Save Metis HQ backup")
        .set_file_name(suggested)
        .add_filter("Metis backup", &["metisbak"])
        .save_file()
        .ok_or_else(|| BackupError::Msg("Save cancelled.".into()))?;
    fs::copy(source, &dest).map_err(|e| BackupError::Msg(e.to_string()))?;
    Ok(dest)
}

fn backups_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("backups")
}

/// Pick an external `.metisbak` and copy it into `data/backups/` for restore.
pub fn import_backup_with_dialog(data_dir: &Path) -> Result<PathBuf, BackupError> {
    let source = rfd::FileDialog::new()
        .set_title("Open Metis HQ backup")
        .add_filter("Metis backup", &["metisbak"])
        .pick_file()
        .ok_or_else(|| BackupError::Msg("Open cancelled.".into()))?;
    if !source.is_file() {
        return Err(BackupError::Msg("Backup file not found.".into()));
    }
    let header = fs::read(&source).map_err(|e| BackupError::Msg(e.to_string()))?;
    if header.len() < MAGIC.len() || &header[..MAGIC.len()] != MAGIC {
        return Err(BackupError::Msg("Not a Metis backup file.".into()));
    }
    let dir = backups_dir(data_dir);
    fs::create_dir_all(&dir).map_err(|e| BackupError::Msg(e.to_string()))?;
    let base = source
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| n.ends_with(".metisbak"))
        .unwrap_or("imported.metisbak");
    let mut dest = dir.join(base);
    if dest.exists() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let stem = base.trim_end_matches(".metisbak");
        dest = dir.join(format!("{stem}-import-{stamp}.metisbak"));
    }
    fs::copy(&source, &dest).map_err(|e| BackupError::Msg(e.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&dest, fs::Permissions::from_mode(0o600));
    }
    Ok(dest)
}

pub fn delete_backup(path: &Path) -> Result<(), BackupError> {
    if !path.is_file() {
        return Err(BackupError::Msg("Backup file not found.".into()));
    }
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| BackupError::Msg("Invalid backup path.".into()))?;
    if !name.ends_with(".metisbak") {
        return Err(BackupError::Msg("Not a Metis backup file.".into()));
    }
    // Only allow deletes under …/backups/
    let parent_ok = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        == Some("backups");
    if !parent_ok {
        return Err(BackupError::Msg("Can only delete backups in data/backups.".into()));
    }
    fs::remove_file(path).map_err(|e| BackupError::Msg(e.to_string()))
}

const SCHEDULE_PASS_FILE: &str = "backup-schedule.pass";

pub fn schedule_pass_path(data_dir: &Path) -> PathBuf {
    data_dir.join(SCHEDULE_PASS_FILE)
}

pub fn save_schedule_passphrase(data_dir: &Path, passphrase: &str) -> Result<(), BackupError> {
    let passphrase = passphrase.trim();
    if passphrase.is_empty() {
        return Err(BackupError::Msg("Passphrase required for scheduled backups.".into()));
    }
    fs::create_dir_all(data_dir).map_err(|e| BackupError::Msg(e.to_string()))?;
    let path = schedule_pass_path(data_dir);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut opts = fs::OpenOptions::new();
        opts.create(true).write(true).truncate(true).mode(0o600);
        use std::io::Write;
        let mut file = opts.open(&path).map_err(|e| BackupError::Msg(e.to_string()))?;
        file.write_all(passphrase.as_bytes())
            .map_err(|e| BackupError::Msg(e.to_string()))?;
    }
    #[cfg(not(unix))]
    {
        fs::write(&path, passphrase).map_err(|e| BackupError::Msg(e.to_string()))?;
    }
    Ok(())
}

pub fn load_schedule_passphrase(data_dir: &Path) -> Option<String> {
    fs::read_to_string(schedule_pass_path(data_dir))
        .ok()
        .map(|s| s.trim_end_matches(['\r', '\n']).to_string())
        .filter(|s| !s.trim().is_empty())
}

pub fn clear_schedule_passphrase(data_dir: &Path) {
    let _ = fs::remove_file(schedule_pass_path(data_dir));
}

/// `off` | `daily` | `weekly` | `monthly`
pub fn schedule_interval_secs(schedule: &str) -> Option<i64> {
    match schedule.trim().to_ascii_lowercase().as_str() {
        "daily" => Some(24 * 60 * 60),
        "weekly" => Some(7 * 24 * 60 * 60),
        "monthly" => Some(30 * 24 * 60 * 60),
        _ => None,
    }
}

/// Run from the services poller when due. No-op when schedule is off or passphrase missing.
pub fn maybe_run_scheduled_backup(db: &Db, secrets: &Secrets, data_dir: &Path) {
    let schedule = db
        .setting("backup_schedule")
        .ok()
        .flatten()
        .unwrap_or_else(|| "off".into());
    let Some(interval) = schedule_interval_secs(&schedule) else {
        return;
    };
    let last = db
        .setting("backup_last_at")
        .ok()
        .flatten()
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);
    let now = unix_now();
    if last > 0 && now.saturating_sub(last) < interval {
        return;
    }
    let Some(pass) = load_schedule_passphrase(data_dir) else {
        hq_log!(debug, "scheduled backup skipped — no saved passphrase");
        return;
    };
    let path = default_backup_path(data_dir);
    match write_backup_file(db, secrets, &pass, &path) {
        Ok(saved) => {
            let _ = db.set_setting("backup_last_at", &now.to_string());
            let _ = db.audit("backup_scheduled", "", &saved.display().to_string());
            hq_log!(info, "scheduled backup → {}", saved.display());
        }
        Err(err) => hq_log!(warn, "scheduled backup failed: {err}"),
    }
}

pub fn decrypt_backup_file(path: &Path, passphrase: &str) -> Result<BackupPayload, BackupError> {
    let file = fs::read(path).map_err(|e| BackupError::Msg(e.to_string()))?;
    let plain = decrypt_payload(passphrase, &file)?;
    let payload: BackupPayload =
        serde_json::from_slice(&plain).map_err(|_| BackupError::Format)?;
    if payload.format != FORMAT || payload.version != VERSION {
        return Err(BackupError::Format);
    }
    Ok(payload)
}

pub fn preview_backup(payload: &BackupPayload) -> Result<BackupPreview, BackupError> {
    let db_bytes = B64
        .decode(payload.db_base64.as_bytes())
        .map_err(|e| BackupError::Msg(format!("db decode: {e}")))?;
    let (nodes, messages, channels) = Db::count_snapshot(&db_bytes)
        .map_err(|e| BackupError::Msg(e.to_string()))?;
    Ok(BackupPreview {
        created_at: payload.created_at,
        hq_name: payload.hq_name.clone(),
        app_version: payload.app_version.clone(),
        node_count: nodes,
        message_count: messages,
        channel_count: channels,
        secret_count: payload.secrets.len(),
    })
}

/// Replace live DB contents and re-store channel secrets. Caller should audit.
pub fn apply_restore(
    db: &Db,
    secrets: &Secrets,
    payload: &BackupPayload,
) -> Result<(), BackupError> {
    let db_bytes = B64
        .decode(payload.db_base64.as_bytes())
        .map_err(|e| BackupError::Msg(format!("db decode: {e}")))?;
    db.restore_sqlite_bytes(&db_bytes)
        .map_err(|e| BackupError::Msg(e.to_string()))?;
    db.ensure_defaults()
        .map_err(|e| BackupError::Msg(e.to_string()))?;
    for (name, hex_secret) in &payload.secrets {
        let bytes = hex::decode(hex_secret.trim()).map_err(|e| BackupError::Msg(e.to_string()))?;
        let arr: [u8; 16] = bytes
            .try_into()
            .map_err(|_| BackupError::Msg(format!("bad secret length for {name}")))?;
        let kind = secrets
            .store(name, &arr)
            .map_err(|e| BackupError::Msg(e))?;
        let _ = db.set_setting("secret_store", kind.as_str());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_pair() -> (Db, Secrets, PathBuf) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "metis-bak-{}-{}-{}",
            std::process::id(),
            unix_now(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        // File-backed like production: restore re-applies WAL pragmas, which
        // in-memory SQLite cannot honor (journal_mode stays "memory").
        let db = Db::open(&dir.join("metis.db")).unwrap();
        db.ensure_defaults().unwrap();
        let secrets = Secrets::file_only(&dir);
        (db, secrets, dir)
    }

    #[test]
    fn empty_passphrase_rejected() {
        let (db, secrets, dir) = test_pair();
        assert!(create_backup_bytes(&db, &secrets, "").is_err());
        assert!(create_backup_bytes(&db, &secrets, "   ").is_err());
        assert!(save_schedule_passphrase(&dir, "").is_err());
        assert!(save_schedule_passphrase(&dir, "  ").is_err());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn round_trip_backup_restore() {
        let (db, secrets, dir) = test_pair();
        let secret = Secrets::generate();
        secrets.store("METIS-INFO", &secret).unwrap();
        let _ = db.set_setting("hq_name", "Test HQ");
        // Private channel row for secret inventory (Public occupies slot 0 in defaults).
        let _ = db.create_channel("BACKUP-TEST", "private", &secrets);

        let bytes = create_backup_bytes(&db, &secrets, "correct horse").unwrap();
        assert!(bytes.starts_with(MAGIC));

        let path = dir.join("t.metisbak");
        fs::write(&path, &bytes).unwrap();
        assert!(decrypt_backup_file(&path, "wrong").is_err());

        let payload = decrypt_backup_file(&path, "correct horse").unwrap();
        assert_eq!(payload.hq_name, "Test HQ");
        assert_eq!(
            payload.secrets.get("METIS-INFO").map(String::as_str),
            Some(hex::encode(secret).as_str())
        );
        assert!(payload.secrets.contains_key("BACKUP-TEST"));
        let preview = preview_backup(&payload).unwrap();
        assert!(preview.secret_count >= 1);

        let (db2, secrets2, dir2) = test_pair();
        apply_restore(&db2, &secrets2, &payload).unwrap();
        assert_eq!(secrets2.load("METIS-INFO").unwrap(), Some(secret));
        assert_eq!(
            db2.setting("hq_name").unwrap().as_deref(),
            Some("Test HQ")
        );
        let _ = fs::remove_dir_all(dir);
        let _ = fs::remove_dir_all(dir2);
    }
}
