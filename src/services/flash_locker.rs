//! Opt-in LAN firmware locker over HTTPS (PIN session).
//!
//! Serves curated locker builds for download and browser Web Serial flashing.
//! Separate from the read-only Local API. No radio, secrets, or enroll routes.

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::Write;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use rand::RngCore;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;

use crate::db::Db;
use crate::firmware::{self, IndexAsset};

const PIN_FILE: &str = "flash-locker.pin";
const TLS_CRT: &str = "flash-locker.tls.crt";
const TLS_KEY: &str = "flash-locker.tls.key";
const COOKIE_NAME: &str = "mmhq_flash";
const DEFAULT_BIND: &str = "0.0.0.0";
/// Standard HTTPS. Ports below 1024 need CAP_NET_BIND_SERVICE (or root) on Linux.
const DEFAULT_PORT: u16 = 443;
const HTTP_REDIRECT_PORT: u16 = 80;
const DEFAULT_SESSION_MINS: u32 = 60;
const RATE_WINDOW: Duration = Duration::from_secs(60);
const PIN_FAIL_LIMIT: u32 = 10;
const REQUEST_LIMIT: u32 = 180;

const INDEX_HTML: &str = include_str!("../../assets/flash_locker/index.html");
const APP_CSS: &str = include_str!("../../assets/flash_locker/app.css");
const APP_JS: &str = include_str!("../../assets/flash_locker/app.js");
const VENDOR_JSZIP: &str = include_str!("../../assets/flash_locker/vendor/jszip.min.js");
const VENDOR_NRF_DFU: &str = include_str!("../../assets/flash_locker/vendor/nrf_dfu.js");

#[derive(Debug, Clone)]
pub struct FlashLockerConfig {
    pub enabled: bool,
    pub bind: String,
    pub port: u16,
    pub session_mins: u32,
}

#[derive(Debug, Clone, Default)]
struct LockerStatus {
    listening: bool,
    last_error: String,
    bind_addr: String,
    cert_fingerprint: String,
}

#[derive(Clone)]
struct Session {
    expires: Instant,
}

fn status() -> &'static Mutex<LockerStatus> {
    static S: OnceLock<Mutex<LockerStatus>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(LockerStatus::default()))
}

fn config_gen() -> &'static Mutex<u64> {
    static G: OnceLock<Mutex<u64>> = OnceLock::new();
    G.get_or_init(|| Mutex::new(0))
}

fn sessions() -> &'static Mutex<HashMap<String, Session>> {
    static S: OnceLock<Mutex<HashMap<String, Session>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

type RateBuckets = HashMap<IpAddr, VecDeque<Instant>>;

fn pin_fail_buckets() -> &'static Mutex<RateBuckets> {
    static B: OnceLock<Mutex<RateBuckets>> = OnceLock::new();
    B.get_or_init(|| Mutex::new(HashMap::new()))
}

fn req_buckets() -> &'static Mutex<RateBuckets> {
    static B: OnceLock<Mutex<RateBuckets>> = OnceLock::new();
    B.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn bump_flash_locker_config() {
    if let Ok(mut g) = config_gen().lock() {
        *g = g.wrapping_add(1);
    }
}

pub fn clamp_session_mins(raw: u32) -> u32 {
    raw.clamp(15, 480)
}

fn setting(db: &Db, key: &str) -> String {
    db.setting(key)
        .ok()
        .flatten()
        .unwrap_or_default()
        .trim()
        .to_string()
}

pub fn load_flash_locker_config(db: &Db) -> FlashLockerConfig {
    let bind_raw = setting(db, "flash_locker_bind");
    let bind = if bind_raw.is_empty() {
        DEFAULT_BIND.to_string()
    } else {
        bind_raw
    };
    let mins = setting(db, "flash_locker_session_mins")
        .parse::<u32>()
        .unwrap_or(DEFAULT_SESSION_MINS);
    FlashLockerConfig {
        enabled: crate::services::setting_bool(db, "flash_locker_enabled", false),
        bind,
        port: setting(db, "flash_locker_port")
            .parse()
            .unwrap_or(DEFAULT_PORT),
        session_mins: clamp_session_mins(mins),
    }
}

fn write_secret_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut opts = fs::OpenOptions::new();
        opts.create(true).write(true).truncate(true).mode(0o600);
        let mut file = opts.open(path).map_err(|e| e.to_string())?;
        file.write_all(bytes).map_err(|e| e.to_string())?;
    }
    #[cfg(not(unix))]
    {
        fs::write(path, bytes).map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn pin_path(data_dir: &Path) -> PathBuf {
    data_dir.join(PIN_FILE)
}

pub fn tls_crt_path(data_dir: &Path) -> PathBuf {
    data_dir.join(TLS_CRT)
}

pub fn tls_key_path(data_dir: &Path) -> PathBuf {
    data_dir.join(TLS_KEY)
}

pub fn pin_is_set(data_dir: &Path) -> bool {
    pin_path(data_dir)
        .is_file()
        .then(|| fs::read_to_string(pin_path(data_dir)).ok())
        .flatten()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
}

/// Hash and store PIN (Argon2id). Never logs the PIN.
pub fn set_pin(data_dir: &Path, pin: &str) -> Result<(), String> {
    let pin = pin.trim();
    if pin.len() < 4 || pin.len() > 64 {
        return Err("PIN must be 4–64 characters.".into());
    }
    let salt = SaltString::generate(&mut rand::thread_rng());
    let hash = Argon2::default()
        .hash_password(pin.as_bytes(), &salt)
        .map_err(|e| format!("PIN hash failed: {e}"))?
        .to_string();
    write_secret_file(&pin_path(data_dir), hash.as_bytes())?;
    bump_flash_locker_config();
    Ok(())
}

pub fn verify_pin(data_dir: &Path, pin: &str) -> bool {
    let Ok(stored) = fs::read_to_string(pin_path(data_dir)) else {
        return false;
    };
    let stored = stored.trim();
    if stored.is_empty() {
        return false;
    }
    let Ok(parsed) = PasswordHash::new(stored) else {
        return false;
    };
    Argon2::default()
        .verify_password(pin.trim().as_bytes(), &parsed)
        .is_ok()
}

fn allow_request(window: &mut VecDeque<Instant>, now: Instant, limit: u32) -> bool {
    while let Some(front) = window.front() {
        if now.duration_since(*front) > RATE_WINDOW {
            window.pop_front();
        } else {
            break;
        }
    }
    if window.len() as u32 >= limit {
        return false;
    }
    window.push_back(now);
    true
}

fn take_rate_slot(buckets: &Mutex<RateBuckets>, ip: IpAddr, limit: u32) -> bool {
    let Ok(mut map) = buckets.lock() else {
        return true;
    };
    let now = Instant::now();
    let entry = map.entry(ip).or_default();
    allow_request(entry, now, limit)
}

fn purge_sessions() {
    let Ok(mut map) = sessions().lock() else {
        return;
    };
    let now = Instant::now();
    map.retain(|_, s| s.expires > now);
}

fn active_session_count() -> usize {
    purge_sessions();
    sessions().lock().map(|m| m.len()).unwrap_or(0)
}

fn create_session(session_mins: u32) -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let id = hex::encode(bytes);
    let expires = Instant::now() + Duration::from_secs(u64::from(session_mins) * 60);
    if let Ok(mut map) = sessions().lock() {
        map.insert(id.clone(), Session { expires });
    }
    id
}

fn session_valid(id: &str) -> Option<Duration> {
    purge_sessions();
    let Ok(map) = sessions().lock() else {
        return None;
    };
    let sess = map.get(id)?;
    let now = Instant::now();
    if sess.expires <= now {
        return None;
    }
    Some(sess.expires.saturating_duration_since(now))
}

fn clear_session(id: &str) {
    if let Ok(mut map) = sessions().lock() {
        map.remove(id);
    }
}

fn parse_cookie(headers: &str, name: &str) -> Option<String> {
    for line in headers.lines() {
        let line = line.trim();
        let Some((n, v)) = line.split_once(':') else {
            continue;
        };
        if !n.eq_ignore_ascii_case("cookie") {
            continue;
        }
        for part in v.split(';') {
            let part = part.trim();
            let Some((k, val)) = part.split_once('=') else {
                continue;
            };
            if k.trim() == name {
                let val = val.trim();
                if !val.is_empty() {
                    return Some(val.to_string());
                }
            }
        }
    }
    None
}

fn fingerprint_sha256(der: &[u8]) -> String {
    let dig = Sha256::digest(der);
    dig.iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// Short fingerprint for Settings (first 8 groups).
pub fn cert_fingerprint_short(data_dir: &Path) -> String {
    let full = status()
        .lock()
        .ok()
        .map(|s| s.cert_fingerprint.clone())
        .unwrap_or_default();
    if !full.is_empty() {
        return shorten_fingerprint(&full);
    }
    let Ok(pem) = fs::read_to_string(tls_crt_path(data_dir)) else {
        return String::new();
    };
    let Ok(ders) = rustls_pemfile::certs(&mut pem.as_bytes()).collect::<Result<Vec<_>, _>>() else {
        return String::new();
    };
    let Some(der) = ders.first() else {
        return String::new();
    };
    shorten_fingerprint(&fingerprint_sha256(der.as_ref()))
}

fn shorten_fingerprint(full: &str) -> String {
    let parts: Vec<&str> = full.split(':').collect();
    if parts.len() <= 8 {
        return full.to_string();
    }
    format!("{}…{}", parts[..4].join(":"), parts[parts.len() - 4..].join(":"))
}

pub fn primary_lan_ip() -> Option<String> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("1.1.1.1:80").ok()?;
    let ip = socket.local_addr().ok()?.ip();
    if ip.is_loopback() {
        return None;
    }
    Some(ip.to_string())
}

/// HTTPS origin for `host` (omit `:443`).
pub fn https_origin(host: &str, port: u16) -> String {
    if port == 443 {
        format!("https://{host}")
    } else {
        format!("https://{host}:{port}")
    }
}

pub fn flash_locker_urls(db: &Db) -> Vec<String> {
    let cfg = load_flash_locker_config(db);
    let mut urls = Vec::new();
    if let Some(ip) = primary_lan_ip() {
        urls.push(format!("{}/", https_origin(&ip, cfg.port)));
    }
    if cfg.bind == "127.0.0.1" || cfg.bind == "::1" {
        urls.push(format!("{}/", https_origin("127.0.0.1", cfg.port)));
    } else if !urls.iter().any(|u| u.contains("127.0.0.1")) {
        urls.push(format!("{}/", https_origin("127.0.0.1", cfg.port)));
    }
    urls
}

fn format_bind_error(bind: &str, port: u16, err: &std::io::Error) -> String {
    let base = format!("bind {bind}:{port}: {err}");
    if err.kind() == std::io::ErrorKind::PermissionDenied && port < 1024 {
        format!(
            "{base} — HQ can request permission (Polkit), or set Port to 8443."
        )
    } else {
        base
    }
}

fn elevate_attempted() -> &'static AtomicBool {
    static F: OnceLock<AtomicBool> = OnceLock::new();
    F.get_or_init(|| AtomicBool::new(false))
}

/// Path of this process binary (for setcap / restart).
pub fn flash_locker_exe_path() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("current exe: {e}"))?;
    Ok(exe.canonicalize().unwrap_or(exe))
}

/// Linux: ask Polkit (`pkexec`) to grant `cap_net_bind_service` on this binary so
/// ports 80/443 work without running HQ as root. Takes effect after restart.
#[cfg(target_os = "linux")]
pub fn grant_low_port_capability() -> Result<(), String> {
    let exe = flash_locker_exe_path()?;
    let exe_s = exe
        .to_str()
        .ok_or_else(|| "Executable path is not valid UTF-8.".to_string())?;
    let pkexec = if Path::new("/usr/bin/pkexec").is_file() {
        "/usr/bin/pkexec"
    } else {
        "pkexec"
    };
    let setcap = if Path::new("/usr/sbin/setcap").is_file() {
        "/usr/sbin/setcap"
    } else if Path::new("/sbin/setcap").is_file() {
        "/sbin/setcap"
    } else {
        "setcap"
    };
    let output = std::process::Command::new(pkexec)
        .args([setcap, "cap_net_bind_service=+ep", exe_s])
        .output()
        .map_err(|e| {
            format!(
                "Could not open permission prompt ({e}). Install policykit-1, or run: \
                 sudo setcap 'cap_net_bind_service=+ep' {exe_s}"
            )
        })?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let detail = format!("{stdout}\n{stderr}").trim().to_string();
    if detail.contains("dismissed")
        || detail.contains("Not authorized")
        || output.status.code() == Some(126)
        || output.status.code() == Some(127)
    {
        return Err(
            "Permission request cancelled or denied. Use Port 8443, or approve the prompt next time."
                .into(),
        );
    }
    Err(if detail.is_empty() {
        format!(
            "setcap failed (exit {:?}). Try: sudo setcap 'cap_net_bind_service=+ep' {exe_s}",
            output.status.code()
        )
    } else {
        format!("setcap failed: {detail}")
    })
}

#[cfg(not(target_os = "linux"))]
pub fn grant_low_port_capability() -> Result<(), String> {
    Err(
        "Automatic low-port access is only on Linux. Use Port 8443, or run HQ as administrator."
            .into(),
    )
}

/// Re-launch this process (needed after setcap — file caps apply at exec).
pub fn restart_hq_process() -> Result<(), String> {
    let exe = flash_locker_exe_path()?;
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    std::process::Command::new(&exe)
        .args(&args)
        .spawn()
        .map_err(|e| format!("Could not restart HQ: {e}"))?;
    std::process::exit(0);
}

/// Grant low-port capability then restart so :80/:443 bind works.
pub fn grant_low_port_and_restart() -> Result<(), String> {
    grant_low_port_capability()?;
    restart_hq_process()
}

pub fn flash_locker_status_line(db: &Db) -> String {
    let cfg = load_flash_locker_config(db);
    if !cfg.enabled {
        return "Flash locker off".into();
    }
    if !pin_is_set(&crate::data_dir()) {
        return format!(
            "Flash locker enabled on {}:{} but no PIN — set a PIN before sharing",
            cfg.bind, cfg.port
        );
    }
    let st = status().lock().ok();
    let listening = st.as_ref().map(|s| s.listening).unwrap_or(false);
    let err = st
        .as_ref()
        .map(|s| s.last_error.clone())
        .unwrap_or_default();
    let n = active_session_count();
    if !err.is_empty() && !listening {
        return format!("Flash locker error: {err}");
    }
    if listening {
        format!(
            "Flash locker listening on {}:{} · {n} session(s)",
            cfg.bind, cfg.port
        )
    } else {
        format!("Flash locker starting on {}:{}…", cfg.bind, cfg.port)
    }
}

/// Generate (or regenerate) a self-signed TLS cert for the locker.
pub fn ensure_tls_cert(data_dir: &Path, force: bool) -> Result<String, String> {
    let crt = tls_crt_path(data_dir);
    let key = tls_key_path(data_dir);
    if !force && crt.is_file() && key.is_file() {
        let pem = fs::read_to_string(&crt).map_err(|e| e.to_string())?;
        let ders = rustls_pemfile::certs(&mut pem.as_bytes())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let der = ders.first().ok_or_else(|| "TLS cert empty".to_string())?;
        let fp = fingerprint_sha256(der.as_ref());
        if let Ok(mut st) = status().lock() {
            st.cert_fingerprint = fp.clone();
        }
        return Ok(fp);
    }

    let mut names = vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "::1".to_string(),
    ];
    if let Some(ip) = primary_lan_ip() {
        if !names.contains(&ip) {
            names.push(ip);
        }
    }

    let mut params = rcgen::CertificateParams::new(names).map_err(|e| e.to_string())?;
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "Metis Mesh HQ Flash Locker");
    params.not_before = rcgen::date_time_ymd(2024, 1, 1);
    params.not_after = rcgen::date_time_ymd(2035, 1, 1);
    let key_pair = rcgen::KeyPair::generate().map_err(|e| e.to_string())?;
    let cert = params.self_signed(&key_pair).map_err(|e| e.to_string())?;
    let cert_pem = cert.pem();
    let key_pem = key_pair.serialize_pem();
    write_secret_file(&crt, cert_pem.as_bytes())?;
    write_secret_file(&key, key_pem.as_bytes())?;
    let fp = fingerprint_sha256(cert.der());
    if let Ok(mut st) = status().lock() {
        st.cert_fingerprint = fp.clone();
    }
    bump_flash_locker_config();
    Ok(fp)
}

fn load_tls_acceptor(data_dir: &Path) -> Result<TlsAcceptor, String> {
    ensure_tls_cert(data_dir, false)?;
    let crt_pem = fs::read(tls_crt_path(data_dir)).map_err(|e| e.to_string())?;
    let key_pem = fs::read(tls_key_path(data_dir)).map_err(|e| e.to_string())?;
    let certs = rustls_pemfile::certs(&mut crt_pem.as_slice())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    if certs.is_empty() {
        return Err("No certificates in TLS file".into());
    }
    let key = rustls_pemfile::private_key(&mut key_pem.as_slice())
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "No private key in TLS file".to_string())?;
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| e.to_string())?;
    Ok(TlsAcceptor::from(std::sync::Arc::new(config)))
}

fn asset_id(asset: &IndexAsset) -> String {
    if asset.sha256.len() >= 16 {
        asset.sha256.chars().take(16).collect()
    } else {
        let mut h = Sha256::new();
        h.update(asset.tag.as_bytes());
        h.update(b"|");
        h.update(asset.name.as_bytes());
        hex::encode(h.finalize())[..16].to_string()
    }
}

fn guess_platform(name: &str) -> &'static str {
    let n = name.to_ascii_lowercase();
    if n.contains("nrf") || n.ends_with(".uf2") || n.ends_with(".zip") && n.contains("nrf") {
        "nrf52"
    } else if n.contains("esp32c3") {
        "esp32c3"
    } else if n.contains("esp32s3") {
        "esp32s3"
    } else if n.contains("esp32") || n.ends_with(".bin") {
        "esp32"
    } else {
        "unknown"
    }
}

/// Whether this asset can be flashed over Web Serial for a given board platform.
fn asset_web_serial(asset: &firmware::IndexAsset, device_platform: Option<&str>) -> bool {
    let name_l = asset.name.to_ascii_lowercase();
    let plat = device_platform
        .unwrap_or("")
        .to_ascii_lowercase();
    let guessed = guess_platform(&asset.name);
    if name_l.ends_with(".uf2") {
        return false; // UF2 is drag-drop only
    }
    if plat.starts_with("esp") || (!plat.contains("nrf") && guessed.starts_with("esp")) {
        return name_l.ends_with(".bin");
    }
    if plat.contains("nrf") || guessed.starts_with("nrf") {
        // Adafruit DFU package (.zip with manifest.json) — same as MeshCore flasher.
        // Skip erase/bootloader CDN extras (those are UF2 drag-drop / wipe tools).
        return name_l.ends_with(".zip")
            && asset.family != "erase"
            && asset.family != "bootloader";
    }
    // Unknown board: ESP-style bins only.
    name_l.ends_with(".bin") && guessed.starts_with("esp")
}

fn asset_json(asset: &firmware::IndexAsset) -> Value {
    asset_json_for(asset, None)
}

fn asset_json_for(asset: &firmware::IndexAsset, device_platform: Option<&str>) -> Value {
    let name_l = asset.name.to_ascii_lowercase();
    let uf2 = name_l.ends_with(".uf2");
    let platform = match device_platform.filter(|p| !p.trim().is_empty()) {
        Some(p) => p,
        None => guess_platform(&asset.name),
    };
    let web_serial = asset_web_serial(asset, device_platform);
    let flash_kind = if name_l.ends_with(".zip") && web_serial {
        "nrf_dfu"
    } else if web_serial {
        "esp_slip"
    } else {
        "download"
    };
    json!({
        "id": asset_id(asset),
        "family": asset.family,
        "tag": asset.tag,
        "name": asset.name,
        "bytes": asset.bytes,
        "sha256": asset.sha256,
        "platform": platform,
        "uf2": uf2,
        "flash_offset": "0x10000",
        "erase_offset": "0x0",
        "web_serial": web_serial,
        "flash_kind": flash_kind,
        "github_verified": asset.github_verified,
    })
}

fn asset_file_exists(asset: &firmware::IndexAsset) -> bool {
    let path = Path::new(&asset.path);
    if path.is_file() {
        return true;
    }
    // Index may still hold a stale absolute path after the data dir moved; try tag/name.
    let by_name = firmware::firmware_root()
        .join("releases")
        .join(
            asset
                .tag
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect::<String>(),
        )
        .join(&asset.name);
    by_name.is_file()
}

fn role_matchers(role: &firmware::RoleChoice) -> (Vec<regex::Regex>, Vec<String>) {
    let mut regexes = Vec::new();
    for pattern in role.patterns.values() {
        if let Ok(re) = regex::Regex::new(pattern) {
            regexes.push(re);
        }
    }
    let static_names: Vec<String> = role
        .static_versions
        .iter()
        .flat_map(|ver| ver.files.iter().map(|(_, n)| n.clone()))
        .collect();
    (regexes, static_names)
}

fn role_matches_precompiled(
    regexes: &[regex::Regex],
    static_names: &[String],
    asset: &firmware::IndexAsset,
) -> bool {
    if !regexes.is_empty() {
        return regexes.iter().any(|re| re.is_match(&asset.name));
    }
    static_names
        .iter()
        .any(|n| n.eq_ignore_ascii_case(&asset.name))
}

fn catalog_json() -> Value {
    let index = firmware::load_index();
    let on_disk: Vec<&firmware::IndexAsset> = index
        .assets
        .iter()
        .filter(|a| asset_file_exists(a) && !a.sha256.trim().is_empty())
        .collect();

    let mut flat_assets: Vec<Value> = on_disk.iter().map(|a| asset_json(a)).collect();
    flat_assets.sort_by(|a, b| {
        let an = a.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let bn = b.get("name").and_then(|v| v.as_str()).unwrap_or("");
        an.cmp(bn)
    });

    // Device → role → version tree (MeshCore flasher-style browse).
    let mut devices = Vec::new();
    let mut makers: Vec<Value> = Vec::new();
    let mut catalog_error = String::new();
    match firmware::load_catalog() {
        Ok(catalog) => {
            let offers = firmware::community_offers(&catalog);
            let mut maker_labels: std::collections::BTreeMap<String, String> =
                std::collections::BTreeMap::new();
            for offer in &offers {
                maker_labels
                    .entry(offer.maker_id.clone())
                    .or_insert_with(|| offer.maker_label.clone());
            }
            for (id, label) in &maker_labels {
                makers.push(json!({ "id": id, "label": label }));
            }

            for device in firmware::community_devices(&catalog) {
                let maker_label = maker_labels
                    .get(&device.maker)
                    .cloned()
                    .unwrap_or_else(|| device.maker.clone());
                let boot_uf2 = firmware::preferred_bootloader_uf2(&device.bootloader);
                let mut bootloader_assets = Vec::new();
                for name in &device.bootloader {
                    if let Some(asset) = on_disk.iter().copied().find(|a| a.name == *name) {
                        bootloader_assets.push(asset_json(asset));
                    }
                }
                let mut erase_assets = Vec::new();
                if let Some(name) = device.erase.as_ref() {
                    let mut erase_names = vec![name.clone()];
                    if name.to_ascii_lowercase().ends_with(".zip") {
                        erase_names.push(format!("{}uf2", &name[..name.len() - 3]));
                    }
                    for ename in erase_names {
                        if let Some(asset) = on_disk.iter().copied().find(|a| a.name == ename) {
                            erase_assets.push(asset_json(asset));
                        }
                    }
                }
                let erase_asset = erase_assets
                    .iter()
                    .find(|a| {
                        a.get("name")
                            .and_then(|v| v.as_str())
                            .map(|n| n.to_ascii_lowercase().ends_with(".uf2"))
                            .unwrap_or(false)
                    })
                    .cloned()
                    .or_else(|| erase_assets.first().cloned());
                let mut roles_out = Vec::new();
                let mut device_has_web = false;
                let mut device_has_dl = false;
                for role in &device.roles {
                    let (regexes, static_names) = role_matchers(role);
                    let matched: Vec<&firmware::IndexAsset> = on_disk
                        .iter()
                        .copied()
                        .filter(|a| role_matches_precompiled(&regexes, &static_names, a))
                        .collect();
                    if matched.is_empty() {
                        continue;
                    }
                    let mut by_tag: std::collections::BTreeMap<String, Vec<Value>> =
                        std::collections::BTreeMap::new();
                    for asset in matched {
                        let aj = asset_json_for(asset, Some(device.platform.as_str()));
                        if aj.get("web_serial").and_then(|v| v.as_bool()) == Some(true) {
                            device_has_web = true;
                        } else {
                            device_has_dl = true;
                        }
                        by_tag.entry(asset.tag.clone()).or_default().push(aj);
                    }
                    let mut versions = Vec::new();
                    for (tag, assets) in by_tag.into_iter().rev() {
                        let version = firmware::short_version_label(&tag);
                        versions.push(json!({
                            "tag": tag,
                            "version": version,
                            "assets": assets,
                        }));
                    }
                    versions.sort_by(|a, b| {
                        let at = a.get("tag").and_then(|v| v.as_str()).unwrap_or("");
                        let bt = b.get("tag").and_then(|v| v.as_str()).unwrap_or("");
                        bt.cmp(at)
                    });
                    let any_web = versions.iter().any(|v| {
                        v.get("assets")
                            .and_then(|a| a.as_array())
                            .map(|arr| {
                                arr.iter().any(|x| {
                                    x.get("web_serial").and_then(|b| b.as_bool()) == Some(true)
                                })
                            })
                            .unwrap_or(false)
                    });
                    let notice_key = role.notice.clone().unwrap_or_default();
                    let notice_text = if notice_key.is_empty() {
                        String::new()
                    } else {
                        firmware::notice_plain(
                            &catalog,
                            &notice_key,
                            boot_uf2.as_deref().unwrap_or("OTAFIX bootloader"),
                        )
                    };
                    let boot_dl = bootloader_assets
                        .iter()
                        .find(|a| {
                            a.get("name")
                                .and_then(|v| v.as_str())
                                .map(|n| n.to_ascii_lowercase().ends_with(".uf2"))
                                .unwrap_or(false)
                        })
                        .or_else(|| bootloader_assets.first())
                        .cloned();
                    roles_out.push(json!({
                        "role": role.role,
                        "label": role.label,
                        "pill": firmware::role_pill(&role.role),
                        "tooltip": role.tooltip,
                        "web_serial": any_web,
                        "notice_key": if notice_key.is_empty() {
                            Value::Null
                        } else {
                            json!(notice_key)
                        },
                        "notice_text": notice_text,
                        "otafix_blog": "https://blog.meshcore.io/2026/04/06/otafix-bootloader",
                        "bootloader_asset": boot_dl,
                        "versions": versions,
                    }));
                }
                if roles_out.is_empty() {
                    continue;
                }
                devices.push(json!({
                    "name": device.name,
                    "maker": device.maker,
                    "maker_label": maker_label,
                    "platform": device.platform,
                    "web_serial": device_has_web,
                    "download_only": device_has_dl && !device_has_web,
                    "bootloader_assets": bootloader_assets,
                    "erase_asset": erase_asset,
                    "erase_assets": erase_assets,
                    "roles": roles_out,
                }));
            }
            devices.sort_by(|a, b| {
                let am = a.get("maker_label").and_then(|v| v.as_str()).unwrap_or("");
                let bm = b.get("maker_label").and_then(|v| v.as_str()).unwrap_or("");
                let an = a.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let bn = b.get("name").and_then(|v| v.as_str()).unwrap_or("");
                am.cmp(bm).then(an.cmp(bn))
            });
        }
        Err(err) => {
            catalog_error = err;
        }
    }

    // If the flasher catalog is missing/unparsed but binaries exist, expose a flat
    // "Files" board so the locker is still usable (download / Web Serial).
    if devices.is_empty() && !flat_assets.is_empty() {
        let mut by_tag: std::collections::BTreeMap<String, Vec<Value>> =
            std::collections::BTreeMap::new();
        let mut any_web = false;
        for asset in &flat_assets {
            if asset.get("web_serial").and_then(|v| v.as_bool()) == Some(true) {
                any_web = true;
            }
            let tag = asset
                .get("tag")
                .and_then(|v| v.as_str())
                .unwrap_or("local")
                .to_string();
            by_tag.entry(tag).or_default().push(asset.clone());
        }
        let mut versions = Vec::new();
        for (tag, assets) in by_tag.into_iter().rev() {
            let version = firmware::short_version_label(&tag);
            versions.push(json!({
                "tag": tag,
                "version": version,
                "assets": assets,
            }));
        }
        versions.sort_by(|a, b| {
            let at = a.get("tag").and_then(|v| v.as_str()).unwrap_or("");
            let bt = b.get("tag").and_then(|v| v.as_str()).unwrap_or("");
            bt.cmp(at)
        });
        devices.push(json!({
            "name": "Trusted files",
            "maker": "local",
            "maker_label": "On this HQ",
            "platform": "mixed",
            "web_serial": any_web,
            "download_only": !any_web,
            "roles": [json!({
                "role": "files",
                "label": "Downloaded firmware",
                "pill": "Files",
                "tooltip": if catalog_error.is_empty() {
                    "Catalog had no device matches for the files on disk. Pick a version and flash or download."
                } else {
                    "Firmware catalog unavailable; showing raw trusted files."
                },
                "web_serial": any_web,
                "versions": versions,
            })],
        }));
        makers.push(json!({ "id": "local", "label": "On this HQ" }));
    }

    json!({
        "updated_at": index.updated_at,
        "count": flat_assets.len(),
        "device_count": devices.len(),
        "makers": makers,
        "devices": devices,
        "assets": flat_assets,
        "catalog_error": if catalog_error.is_empty() {
            Value::Null
        } else {
            json!(catalog_error)
        },
    })
}

fn find_asset_by_id(id: &str) -> Option<IndexAsset> {
    let id = id.trim().to_ascii_lowercase();
    if id.is_empty() {
        return None;
    }
    firmware::load_index()
        .assets
        .into_iter()
        .find(|a| asset_id(a).eq_ignore_ascii_case(&id))
}

fn http_response(
    code: u16,
    content_type: &str,
    body: &[u8],
    extra: &[(&str, &str)],
) -> Vec<u8> {
    let reason = match code {
        200 => "OK",
        204 => "No Content",
        302 => "Found",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        429 => "Too Many Requests",
        _ => "Error",
    };
    let mut out = format!("HTTP/1.1 {code} {reason}\r\nContent-Type: {content_type}\r\n");
    for (k, v) in extra {
        out.push_str(k);
        out.push_str(": ");
        out.push_str(v);
        out.push_str("\r\n");
    }
    out.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    ));
    let mut bytes = out.into_bytes();
    bytes.extend_from_slice(body);
    bytes
}

fn http_json(code: u16, body: &Value) -> Vec<u8> {
    let payload = body.to_string();
    http_response(code, "application/json; charset=utf-8", payload.as_bytes(), &[])
}

fn http_json_extra(code: u16, body: &Value, extra: &[(&str, &str)]) -> Vec<u8> {
    let payload = body.to_string();
    http_response(
        code,
        "application/json; charset=utf-8",
        payload.as_bytes(),
        extra,
    )
}

fn html_page(authed: bool) -> Vec<u8> {
    let mut page = INDEX_HTML.to_string();
    page = page.replace("__AUTHED__", if authed { "1" } else { "0" });
    // Server-side panel visibility so a hung catalog fetch still shows the shell.
    page = page.replace("__LOGIN_HIDDEN__", if authed { "hidden" } else { "" });
    page = page.replace("__LOCKER_HIDDEN__", if authed { "" } else { "hidden" });
    http_response(200, "text/html; charset=utf-8", page.as_bytes(), &[])
}

fn parse_form_pin(body: &str) -> Option<String> {
    // application/x-www-form-urlencoded or JSON {"pin":"..."}
    let trimmed = body.trim();
    if trimmed.starts_with('{') {
        let v: Value = serde_json::from_str(trimmed).ok()?;
        return v
            .get("pin")
            .and_then(|p| p.as_str())
            .map(|s| s.to_string());
    }
    for part in trimmed.split('&') {
        let Some((k, v)) = part.split_once('=') else {
            continue;
        };
        if k == "pin" {
            return Some(urlencoding_decode(v));
        }
    }
    None
}

fn urlencoding_decode(s: &str) -> String {
    let mut out = String::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = &s[i + 1..i + 3];
                if let Ok(v) = u8::from_str_radix(hex, 16) {
                    out.push(v as char);
                    i += 3;
                } else {
                    out.push('%');
                    i += 1;
                }
            }
            c => {
                out.push(c as char);
                i += 1;
            }
        }
    }
    out
}

fn normalize_path(path: &str) -> String {
    let p = path.split('?').next().unwrap_or(path).trim();
    if p.is_empty() {
        return "/".into();
    }
    if p.starts_with('/') {
        p.trim_end_matches('/').to_string()
    } else {
        format!("/{}", p.trim_end_matches('/'))
    }
}

fn hq_display_name(db: &Db) -> String {
    let name = db.hq_advert_name().unwrap_or_default();
    let name = name.trim();
    if !name.is_empty() {
        return name.to_string();
    }
    db.setting("hq_name")
        .ok()
        .flatten()
        .unwrap_or_else(|| "Metis Mesh HQ".into())
}

async fn handle_tls_client(
    stream: tokio_rustls::server::TlsStream<TcpStream>,
    cfg: FlashLockerConfig,
    db: std::sync::Arc<Db>,
    data_dir: PathBuf,
) {
    let peer_ip = stream
        .get_ref()
        .0
        .peer_addr()
        .map(|a| a.ip())
        .unwrap_or_else(|_| IpAddr::from([127, 0, 0, 1]));

    if !take_rate_slot(req_buckets(), peer_ip, REQUEST_LIMIT) {
        let mut stream = stream;
        let _ = stream
            .write_all(&http_json(429, &json!({ "error": "rate limited" })))
            .await;
        return;
    }

    let mut stream = stream;
    let mut buf = vec![0u8; 256 * 1024];
    let n = match tokio::time::timeout(Duration::from_secs(20), stream.read(&mut buf)).await {
        Ok(Ok(n)) => n,
        _ => return,
    };
    if n == 0 {
        return;
    }
    let raw = String::from_utf8_lossy(&buf[..n]);
    let Some(header_end) = raw.find("\r\n\r\n") else {
        let _ = stream
            .write_all(&http_json(400, &json!({ "error": "bad request" })))
            .await;
        return;
    };
    let head = &raw[..header_end];
    let body_start = header_end + 4;
    let mut body = raw[body_start..].to_string();
    let content_length = head.lines().find_map(|line| {
        let (n, v) = line.split_once(':')?;
        if n.eq_ignore_ascii_case("content-length") {
            v.trim().parse::<usize>().ok()
        } else {
            None
        }
    });
    if let Some(want) = content_length {
        while body.len() < want && body.len() < 64 * 1024 {
            let mut more = vec![0u8; 16 * 1024];
            match tokio::time::timeout(Duration::from_secs(5), stream.read(&mut more)).await {
                Ok(Ok(0)) => break,
                Ok(Ok(m)) => body.push_str(&String::from_utf8_lossy(&more[..m])),
                _ => break,
            }
        }
        if body.len() > want {
            body.truncate(want);
        }
    }

    let mut lines = head.lines();
    let Some(req_line) = lines.next() else {
        return;
    };
    let mut parts = req_line.split_whitespace();
    let method = parts.next().unwrap_or("GET").to_ascii_uppercase();
    let path_raw = parts.next().unwrap_or("/");
    let path = normalize_path(path_raw);
    let headers = head;

    let cookie = parse_cookie(headers, COOKIE_NAME);
    let remaining = cookie
        .as_ref()
        .and_then(|id| session_valid(id));
    let authed = remaining.is_some();

    let response = match (method.as_str(), path.as_str()) {
        ("GET", "/" | "") => html_page(authed),
        ("GET", "/app.css") => {
            http_response(200, "text/css; charset=utf-8", APP_CSS.as_bytes(), &[])
        }
        ("GET", "/app.js") => http_response(
            200,
            "application/javascript; charset=utf-8",
            APP_JS.as_bytes(),
            &[],
        ),
        ("GET", "/vendor/jszip.min.js") => http_response(
            200,
            "application/javascript; charset=utf-8",
            VENDOR_JSZIP.as_bytes(),
            &[],
        ),
        ("GET", "/vendor/nrf_dfu.js") => http_response(
            200,
            "application/javascript; charset=utf-8",
            VENDOR_NRF_DFU.as_bytes(),
            &[],
        ),
        ("GET", "/api/status") => {
            let fp = status()
                .lock()
                .ok()
                .map(|s| shorten_fingerprint(&s.cert_fingerprint))
                .unwrap_or_default();
            let secs = remaining.map(|d| d.as_secs()).unwrap_or(0);
            http_json(
                200,
                &json!({
                    "hq_name": hq_display_name(&db),
                    "authed": authed,
                    "session_remaining_secs": secs,
                    "cert_fingerprint": fp,
                    "asset_count": catalog_json()
                        .get("count")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                }),
            )
        }
        ("POST", "/login") => {
            if !take_rate_slot(pin_fail_buckets(), peer_ip, PIN_FAIL_LIMIT) {
                http_json(429, &json!({ "error": "too many PIN attempts" }))
            } else if !pin_is_set(&data_dir) {
                http_json(403, &json!({ "error": "PIN not configured on HQ" }))
            } else if let Some(pin) = parse_form_pin(&body) {
                if verify_pin(&data_dir, &pin) {
                    let id = create_session(cfg.session_mins);
                    let _ = db.audit("flash_locker_login_ok", "", &peer_ip.to_string());
                    let cookie = format!(
                        "{COOKIE_NAME}={id}; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age={}",
                        cfg.session_mins * 60
                    );
                    http_json_extra(
                        200,
                        &json!({ "ok": true }),
                        &[("Set-Cookie", &cookie)],
                    )
                } else {
                    let _ = db.audit("flash_locker_login_fail", "", &peer_ip.to_string());
                    http_json(401, &json!({ "error": "invalid PIN" }))
                }
            } else {
                let _ = db.audit("flash_locker_login_fail", "", &peer_ip.to_string());
                http_json(401, &json!({ "error": "missing pin" }))
            }
        }
        ("POST", "/logout") => {
            if let Some(id) = cookie {
                clear_session(&id);
            }
            let clear = format!("{COOKIE_NAME}=; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age=0");
            http_json_extra(200, &json!({ "ok": true }), &[("Set-Cookie", &clear)])
        }
        ("GET", "/api/catalog") => {
            if !authed {
                http_json(401, &json!({ "error": "login required" }))
            } else {
                // Build off the async worker so a heavy catalog cannot stall TLS accepts.
                let hq_name = hq_display_name(&db);
                let session_mins = cfg.session_mins;
                let cat = tokio::task::spawn_blocking(move || {
                    let mut cat = catalog_json();
                    if let Some(obj) = cat.as_object_mut() {
                        obj.insert("hq_name".into(), json!(hq_name));
                        obj.insert("session_mins".into(), json!(session_mins));
                        // Flat asset dump is large and unused by the board UI.
                        obj.remove("assets");
                    }
                    cat
                })
                .await
                .unwrap_or_else(|err| json!({ "error": format!("catalog build failed: {err}") }));
                if cat.get("error").is_some() && cat.get("devices").is_none() {
                    http_json(500, &cat)
                } else {
                    http_json(200, &cat)
                }
            }
        }
        (m, p) if m == "GET" && p.starts_with("/api/firmware/") => {
            if !authed {
                http_json(401, &json!({ "error": "login required" }))
            } else {
                let id = p.trim_start_matches("/api/firmware/");
                match find_asset_by_id(id) {
                    Some(asset) => {
                        let path = PathBuf::from(&asset.path);
                        match firmware::ensure_flash_path_trusted(&path) {
                            Ok(()) => match fs::read(&path) {
                                Ok(bytes) => {
                                    let _ = db.audit(
                                        "flash_locker_download",
                                        &asset.name,
                                        &peer_ip.to_string(),
                                    );
                                    let disp = format!(
                                        "attachment; filename=\"{}\"",
                                        asset.name.replace('"', "")
                                    );
                                    http_response(
                                        200,
                                        "application/octet-stream",
                                        &bytes,
                                        &[("Content-Disposition", &disp)],
                                    )
                                }
                                Err(err) => {
                                    http_json(500, &json!({ "error": err.to_string() }))
                                }
                            },
                            Err(err) => http_json(403, &json!({ "error": err })),
                        }
                    }
                    None => http_json(404, &json!({ "error": "not found" })),
                }
            }
        }
        ("GET", "/api/node-kits") => {
            if !authed {
                http_json(401, &json!({ "error": "login required" }))
            } else {
                let dir = data_dir.clone();
                let list = tokio::task::spawn_blocking(move || crate::node_kit::kit_list_json(&dir))
                    .await
                    .unwrap_or_default();
                http_json(200, &json!({ "kits": list }))
            }
        }
        ("POST", "/api/node-kit") => {
            if !authed {
                http_json(401, &json!({ "error": "login required" }))
            } else {
                match serde_json::from_str::<Value>(&body) {
                    Ok(v) => {
                        let passphrase = v
                            .get("passphrase")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string();
                        if passphrase.trim().is_empty() {
                            http_json(400, &json!({ "error": "passphrase required" }))
                        } else {
                            let mut payload = crate::node_kit::new_payload_shell("lan-locker");
                            payload.public_key = v
                                .get("public_key")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .to_string();
                            payload.private_key = v
                                .get("private_key")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .to_string();
                            payload.name = v
                                .get("name")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .to_string();
                            payload.role_hint = v
                                .get("role_hint")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .to_string();
                            payload.board = v
                                .get("board")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .to_string();
                            payload.radio = v
                                .get("radio")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .to_string();
                            payload.lat = v
                                .get("lat")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .to_string();
                            payload.lon = v
                                .get("lon")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .to_string();
                            payload.advert_interval = v
                                .get("advert_interval")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .to_string();
                            payload.path_hash_mode = v
                                .get("path_hash_mode")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .to_string();
                            payload.guest_password = v
                                .get("guest_password")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .to_string();
                            payload.claimed_pubkey = v
                                .get("claimed_pubkey")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .to_string();
                            let dir = data_dir.clone();
                            let peer = peer_ip.to_string();
                            let result = tokio::task::spawn_blocking(move || {
                                crate::node_kit::write_kit_file(&dir, &payload, &passphrase)
                            })
                            .await;
                            match result {
                                Ok(Ok(path)) => {
                                    let name = path
                                        .file_name()
                                        .and_then(|n| n.to_str())
                                        .unwrap_or("kit.metisnode")
                                        .to_string();
                                    let _ = db.audit("flash_locker_node_kit", &name, &peer);
                                    http_json(
                                        200,
                                        &json!({
                                            "ok": true,
                                            "id": name,
                                            "file_name": name,
                                        }),
                                    )
                                }
                                Ok(Err(err)) => {
                                    http_json(400, &json!({ "error": err.to_string() }))
                                }
                                Err(err) => {
                                    http_json(500, &json!({ "error": err.to_string() }))
                                }
                            }
                        }
                    }
                    Err(_) => http_json(400, &json!({ "error": "invalid JSON" })),
                }
            }
        }
        ("POST", "/api/node-kit/unlock") => {
            if !authed {
                http_json(401, &json!({ "error": "login required" }))
            } else if !take_rate_slot(pin_fail_buckets(), peer_ip, PIN_FAIL_LIMIT) {
                http_json(429, &json!({ "error": "too many unlock attempts" }))
            } else {
                match serde_json::from_str::<Value>(&body) {
                    Ok(v) => {
                        let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
                        let passphrase = v
                            .get("passphrase")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string();
                        let dir = data_dir.clone();
                        let id_for_job = id.clone();
                        let result = tokio::task::spawn_blocking(move || {
                            let path = crate::node_kit::kit_path_by_name(&dir, &id_for_job)?;
                            crate::node_kit::read_kit_file(&path, &passphrase)
                        })
                        .await;
                        match result {
                            Ok(Ok(kit)) => {
                                let _ = db.audit(
                                    "flash_locker_node_kit_unlock",
                                    &id,
                                    &peer_ip.to_string(),
                                );
                                // Return kit fields for in-browser restore only — never log body.
                                http_json(
                                    200,
                                    &json!({
                                        "ok": true,
                                        "public_key": kit.public_key,
                                        "private_key": kit.private_key,
                                        "name": kit.name,
                                        "lat": kit.lat,
                                        "lon": kit.lon,
                                        "advert_interval": kit.advert_interval,
                                        "guest_password": kit.guest_password,
                                        "admin_password": kit.admin_password,
                                    }),
                                )
                            }
                            Ok(Err(crate::node_kit::NodeKitError::Decrypt)) => {
                                http_json(401, &json!({ "error": "wrong passphrase" }))
                            }
                            Ok(Err(err)) => {
                                http_json(400, &json!({ "error": err.to_string() }))
                            }
                            Err(err) => {
                                http_json(500, &json!({ "error": err.to_string() }))
                            }
                        }
                    }
                    Err(_) => http_json(400, &json!({ "error": "invalid JSON" })),
                }
            }
        }
        ("GET" | "POST" | "PUT" | "DELETE", _) => {
            http_json(404, &json!({ "error": "not found" }))
        }
        _ => http_json(405, &json!({ "error": "method not allowed" })),
    };

    let _ = stream.write_all(&response).await;
}

/// Cleartext HTTP on port 80 → redirect to the configured HTTPS origin.
async fn http_redirect_session(bind: String, https_port: u16, start_gen: u64) {
    let addr = format!("{bind}:{HTTP_REDIRECT_PORT}");
    let listener = match TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(err) => {
            let msg = format_bind_error(&bind, HTTP_REDIRECT_PORT, &err);
            hq_log!(warn, "flash locker HTTP→HTTPS redirect: {msg}");
            if let Ok(mut st) = status().lock() {
                if st.last_error.is_empty() {
                    st.last_error = format!("HTTP :{HTTP_REDIRECT_PORT} redirect unavailable — {msg}");
                }
            }
            return;
        }
    };
    hq_log!(
        info,
        "flash locker HTTP redirect on http://{addr} → HTTPS :{https_port}"
    );
    loop {
        if *config_gen().lock().unwrap_or_else(|e| e.into_inner()) != start_gen {
            break;
        }
        match tokio::time::timeout(Duration::from_secs(2), listener.accept()).await {
            Ok(Ok((mut tcp, _))) => {
                let https_port = https_port;
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let n = match tokio::time::timeout(
                        Duration::from_secs(5),
                        tcp.read(&mut buf),
                    )
                    .await
                    {
                        Ok(Ok(n)) => n,
                        _ => return,
                    };
                    if n == 0 {
                        return;
                    }
                    let raw = String::from_utf8_lossy(&buf[..n]);
                    let host = raw
                        .lines()
                        .find_map(|line| {
                            let (n, v) = line.split_once(':')?;
                            if n.eq_ignore_ascii_case("host") {
                                Some(v.trim().to_string())
                            } else {
                                None
                            }
                        })
                        .unwrap_or_else(|| {
                            primary_lan_ip().unwrap_or_else(|| "localhost".into())
                        });
                    // Strip any existing port from Host.
                    let host_only = host
                        .rsplit_once(':')
                        .and_then(|(h, p)| {
                            if p.chars().all(|c| c.is_ascii_digit()) {
                                Some(h)
                            } else {
                                None
                            }
                        })
                        .unwrap_or(host.as_str());
                    let location = format!("{}/", https_origin(host_only, https_port));
                    let body = format!("Redirecting to {location}\n");
                    let resp = format!(
                        "HTTP/1.1 301 Moved Permanently\r\n\
                         Location: {location}\r\n\
                         Content-Type: text/plain; charset=utf-8\r\n\
                         Content-Length: {}\r\n\
                         Connection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = tcp.write_all(resp.as_bytes()).await;
                });
            }
            Ok(Err(_)) => break,
            Err(_) => {}
        }
    }
}

async fn locker_session(
    db: &std::sync::Arc<Db>,
    cfg: FlashLockerConfig,
) -> Result<(), String> {
    let data_dir = crate::data_dir();
    let acceptor = load_tls_acceptor(&data_dir)?;
    let addr = format!("{}:{}", cfg.bind, cfg.port);
    let listener = TcpListener::bind(&addr).await.map_err(|e| {
        format_bind_error(&cfg.bind, cfg.port, &e)
    })?;
    if let Ok(mut st) = status().lock() {
        st.listening = true;
        st.last_error.clear();
        st.bind_addr = addr.clone();
    }
    hq_log!(
        info,
        "flash locker listening on {}",
        https_origin(&cfg.bind, cfg.port)
    );

    let start_gen = *config_gen().lock().unwrap_or_else(|e| e.into_inner());
    // Best-effort :80 → HTTPS redirect (same bind). Failure is logged, not fatal.
    {
        let bind = cfg.bind.clone();
        let https_port = cfg.port;
        tokio::spawn(async move {
            http_redirect_session(bind, https_port, start_gen).await;
        });
    }

    loop {
        if *config_gen().lock().unwrap_or_else(|e| e.into_inner()) != start_gen {
            break;
        }
        let live = load_flash_locker_config(db);
        if !live.enabled
            || live.bind != cfg.bind
            || live.port != cfg.port
            || live.session_mins != cfg.session_mins
        {
            break;
        }
        match tokio::time::timeout(Duration::from_secs(2), listener.accept()).await {
            Ok(Ok((tcp, _))) => {
                let acceptor = acceptor.clone();
                let cfg = cfg.clone();
                let db = db.clone();
                let data_dir = data_dir.clone();
                tokio::spawn(async move {
                    match acceptor.accept(tcp).await {
                        Ok(tls) => handle_tls_client(tls, cfg, db, data_dir).await,
                        Err(err) => {
                            // Browsers reject the self-signed locker cert until the
                            // operator clicks through — expected, not worth logging.
                            let msg = err.to_string();
                            if msg.contains("CertificateUnknown")
                                || msg.contains("UnknownCA")
                                || msg.contains("BadCertificate")
                            {
                                return;
                            }
                            hq_log!(debug, "flash locker TLS accept: {err}");
                        }
                    }
                });
            }
            Ok(Err(err)) => {
                return Err(format!("accept: {err}"));
            }
            Err(_) => {}
        }
    }
    if let Ok(mut st) = status().lock() {
        st.listening = false;
    }
    Ok(())
}

pub async fn flash_locker_loop(db: std::sync::Arc<Db>) {
    loop {
        let cfg = load_flash_locker_config(&db);
        if !cfg.enabled {
            if let Ok(mut st) = status().lock() {
                st.listening = false;
                st.last_error.clear();
            }
            tokio::time::sleep(Duration::from_secs(3)).await;
            continue;
        }
        let data_dir = crate::data_dir();
        if let Err(err) = ensure_tls_cert(&data_dir, false) {
            if let Ok(mut st) = status().lock() {
                st.listening = false;
                st.last_error = err.clone();
            }
            hq_log!(warn, "flash locker TLS: {err}");
            tokio::time::sleep(Duration::from_secs(5)).await;
            continue;
        }
        match locker_session(&db, cfg.clone()).await {
            Ok(()) => {
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            Err(err) => {
                let privileged = cfg.port < 1024 && err.contains("Permission denied");
                if let Ok(mut st) = status().lock() {
                    st.listening = false;
                    st.last_error = err.clone();
                }
                hq_log!(warn, "flash locker session: {err}");
                if privileged
                    && !elevate_attempted().swap(true, Ordering::SeqCst)
                    && cfg!(target_os = "linux")
                {
                    hq_log!(
                        info,
                        "flash locker: requesting low-port permission (Polkit)…"
                    );
                    if let Ok(mut st) = status().lock() {
                        st.last_error =
                            "Requesting permission for ports 80/443 — approve the system dialog…"
                                .into();
                    }
                    let grant = tokio::task::spawn_blocking(grant_low_port_capability).await;
                    match grant {
                        Ok(Ok(())) => {
                            hq_log!(
                                info,
                                "flash locker: capability granted — restarting HQ so it takes effect"
                            );
                            if let Ok(mut st) = status().lock() {
                                st.last_error =
                                    "Permission granted — restarting HQ to bind 80/443…".into();
                            }
                            // File capabilities apply at exec; re-launch.
                            let _ = restart_hq_process();
                        }
                        Ok(Err(e)) => {
                            if let Ok(mut st) = status().lock() {
                                st.last_error = e.clone();
                            }
                            hq_log!(warn, "flash locker elevate: {e}");
                        }
                        Err(e) => {
                            hq_log!(warn, "flash locker elevate join: {e}");
                        }
                    }
                }
                tokio::time::sleep(Duration::from_secs(if privileged { 20 } else { 2 })).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn session_mins_clamped() {
        assert_eq!(clamp_session_mins(1), 15);
        assert_eq!(clamp_session_mins(60), 60);
        assert_eq!(clamp_session_mins(9999), 480);
    }

    #[test]
    fn pin_round_trip() {
        use std::time::{SystemTime, UNIX_EPOCH};
        let dir = std::env::temp_dir().join(format!(
            "mmhq-pin-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        set_pin(&dir, "1234").unwrap();
        assert!(verify_pin(&dir, "1234"));
        assert!(!verify_pin(&dir, "9999"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rate_limit_blocks() {
        let mut w = VecDeque::new();
        let now = Instant::now();
        for _ in 0..3 {
            assert!(allow_request(&mut w, now, 3));
        }
        assert!(!allow_request(&mut w, now, 3));
    }

    #[test]
    fn asset_id_stable() {
        let a = IndexAsset {
            family: "heltec".into(),
            tag: "v1.0".into(),
            name: "fw.bin".into(),
            bytes: 10,
            sha256: "abcdef0123456789deadbeef".into(),
            path: "/tmp/x".into(),
            github_verified: false,
        };
        assert_eq!(asset_id(&a), "abcdef0123456789");
    }

    #[test]
    fn https_origin_omits_default_port() {
        assert_eq!(https_origin("192.168.0.11", 443), "https://192.168.0.11");
        assert_eq!(https_origin("192.168.0.11", 8443), "https://192.168.0.11:8443");
    }

    #[test]
    fn catalog_json_lists_devices_when_firmware_present() {
        let index = firmware::load_index();
        if index.assets.is_empty() {
            return;
        }
        let started = Instant::now();
        let cat = catalog_json();
        assert!(
            started.elapsed().as_secs() < 8,
            "catalog_json too slow: {:?}",
            started.elapsed()
        );
        let count = cat.get("count").and_then(|v| v.as_u64()).unwrap_or(0);
        let devices = cat
            .get("devices")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        assert!(
            count == 0 || devices > 0,
            "expected devices when count={count}, got device_count={devices}, err={:?}",
            cat.get("catalog_error")
        );
    }
}
