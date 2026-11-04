//! Optional map.meshcore.io uploader (E4).
//!
//! When enabled + authorized, rebuilds overheard infra ADVERTs (types 2/3/4 with
//! GPS) from companion LogData and POSTs them signed by the primary companion.
//! Private keys are never written to SQLite, the keyring, or backups.
//!
//! API host moved from map.meshcore.dev → map.meshcore.io (dev still 307s).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use meshcore_rs::events::{LogData, MeshPacketHeader};
use meshcore_rs::packets::{PayloadType, RouteType};
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use crate::db::Db;
use crate::radio::RadioHandle;
use crate::services::{setting_bool, setting_i64, set_setting_bool, HTTP_USER_AGENT};

const DEFAULT_API_URL: &str = "https://map.meshcore.io/api/v1/uploader/node";
/// Legacy host; nginx 307s to `.io`. Rewrite stored settings + follow POST redirects.
const LEGACY_API_HOST: &str = "map.meshcore.dev";
const DEFAULT_INTERVAL_SECS: i64 = 3600;
const MAX_SEEN: usize = 5_000;
const QUEUE_CAP: usize = 64;

/// Contact / advert types uploaded (repeater / room / sensor). CHAT (1) skipped.
const INFRA_TYPES: [u8; 3] = [2, 3, 4];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RadioParams {
    pub freq: u32,
    pub cr: u8,
    pub sf: u8,
    pub bw: u32,
}

#[derive(Debug, Clone)]
pub struct UploadJob {
    pub packet_hex: String,
    pub pubkey_hex: String,
    pub advert_ts: u32,
    pub params: RadioParams,
    pub name: Option<String>,
}

#[derive(Debug, Clone)]
struct StatusSnap {
    line: String,
    updated: Instant,
}

fn status_snap() -> &'static Mutex<StatusSnap> {
    static SNAP: OnceLock<Mutex<StatusSnap>> = OnceLock::new();
    SNAP.get_or_init(|| {
        Mutex::new(StatusSnap {
            line: "Off".into(),
            updated: Instant::now(),
        })
    })
}

fn set_status(line: impl Into<String>) {
    let mut guard = status_snap().lock().unwrap_or_else(|e| e.into_inner());
    guard.line = line.into();
    guard.updated = Instant::now();
}

/// Settings → Data status line.
pub fn map_upload_status_line(db: &Db) -> String {
    if !setting_bool(db, "svc_map_upload_enabled", false) {
        return "Off".into();
    }
    if !setting_bool(db, "svc_map_upload_authorized", false) {
        return "Need authorize — primary companion signs; private key stays on the radio.".into();
    }
    let snap = status_snap().lock().unwrap_or_else(|e| e.into_inner());
    if snap.line.is_empty() {
        "Idle".into()
    } else {
        snap.line.clone()
    }
}

fn upload_tx() -> &'static Mutex<Option<UnboundedSender<UploadJob>>> {
    static TX: OnceLock<Mutex<Option<UnboundedSender<UploadJob>>>> = OnceLock::new();
    TX.get_or_init(|| Mutex::new(None))
}

fn seen_map() -> &'static Mutex<HashMap<String, u32>> {
    static SEEN: OnceLock<Mutex<HashMap<String, u32>>> = OnceLock::new();
    SEEN.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Optional RAM-only expanded/seed private key while authorized. Never persisted.
fn ram_private_key() -> &'static Mutex<Option<[u8; 64]>> {
    static KEY: OnceLock<Mutex<Option<[u8; 64]>>> = OnceLock::new();
    KEY.get_or_init(|| Mutex::new(None))
}

pub fn clear_ram_private_key() {
    *ram_private_key().lock().unwrap_or_else(|e| e.into_inner()) = None;
}

pub fn store_ram_private_key(key: [u8; 64]) {
    *ram_private_key().lock().unwrap_or_else(|e| e.into_inner()) = Some(key);
}

/// Clear authorization + RAM key (disable / disconnect).
pub fn revoke_map_upload_auth(db: &Db) {
    set_setting_bool(db, "svc_map_upload_authorized", false);
    clear_ram_private_key();
    set_status("Need authorize");
}

pub fn api_url(db: &Db) -> String {
    let raw = db
        .setting("svc_map_upload_api_url")
        .ok()
        .flatten()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_API_URL.into());
    normalize_map_api_url(&raw)
}

/// Point legacy `.dev` endpoints at `.io` (ureq does not follow 307 on POST bodies).
pub fn normalize_map_api_url(url: &str) -> String {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return DEFAULT_API_URL.into();
    }
    if trimmed.contains(LEGACY_API_HOST) {
        return trimmed.replace(LEGACY_API_HOST, "map.meshcore.io");
    }
    trimmed.to_string()
}

pub fn reupload_interval_secs(db: &Db) -> i64 {
    setting_i64(db, "svc_map_upload_interval_secs", DEFAULT_INTERVAL_SECS).clamp(60, 7 * 24 * 3600)
}

/// Rebuild on-air mesh packet bytes from a LogData header + inner payload.
pub fn rebuild_mesh_packet(header: &MeshPacketHeader, payload: &[u8]) -> Option<Vec<u8>> {
    if header.path_hash_size < 1 || header.path_hash_size > 4 {
        return None;
    }
    let expected_path = header.path_len as usize * header.path_hash_size as usize;
    if header.path.len() != expected_path {
        return None;
    }
    let route = header.route_type as u8 & 0x03;
    let ptype = header.payload_type as u8 & 0x0f;
    let ver = header.payload_version & 0x03;
    let header_byte = (ver << 6) | (ptype << 2) | route;
    let path_byte = ((header.path_hash_size - 1) << 6) | (header.path_len & 0x3f);

    let mut out = Vec::with_capacity(1 + 4 + 1 + header.path.len() + payload.len());
    out.push(header_byte);
    if matches!(
        header.route_type,
        RouteType::TransportFlood | RouteType::TransportDirect
    ) {
        let code = header.transport_code?;
        out.extend_from_slice(&code);
    } else if header.transport_code.is_some() {
        // Non-transport routes must not carry a transport code.
        return None;
    }
    out.push(path_byte);
    out.extend_from_slice(&header.path);
    out.extend_from_slice(payload);
    Some(out)
}

/// Compact JSON body `data` string (Python `separators=(',', ':')` key order).
pub fn build_upload_data_string(params: &RadioParams, packet_hex: &str) -> String {
    // Key order must match meshcore-bot: freq, cr, sf, bw then links.
    format!(
        r#"{{"params":{{"freq":{},"cr":{},"sf":{},"bw":{}}},"links":["meshcore://{}"]}}"#,
        params.freq, params.cr, params.sf, params.bw, packet_hex
    )
}

pub fn sha256_bytes(data: &str) -> [u8; 32] {
    let digest = Sha256::digest(data.as_bytes());
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

fn gps_ok(lat_micro: Option<i32>, lon_micro: Option<i32>) -> bool {
    match (lat_micro, lon_micro) {
        (Some(lat), Some(lon)) if lat != 0 || lon != 0 => {
            let lat_f = lat as f64 / 1_000_000.0;
            let lon_f = lon as f64 / 1_000_000.0;
            (-90.0..=90.0).contains(&lat_f) && (-180.0..=180.0).contains(&lon_f)
        }
        _ => false,
    }
}

fn is_infra_type(adv_type: u8) -> bool {
    INFRA_TYPES.contains(&(adv_type & 0x0f))
}

/// Rate-limit gate using advert timestamps (bot-compatible).
pub fn rate_limit_allows(pubkey_hex: &str, advert_ts: u32, interval_secs: i64) -> bool {
    let mut map = seen_map().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(&last) = map.get(pubkey_hex) {
        if advert_ts <= last {
            return false;
        }
        if (advert_ts as i64) < (last as i64).saturating_add(interval_secs) {
            return false;
        }
    }
    map.insert(pubkey_hex.to_string(), advert_ts);
    prune_seen_locked(&mut map, advert_ts, interval_secs);
    true
}

fn prune_seen_locked(map: &mut HashMap<String, u32>, now_ts: u32, interval_secs: i64) {
    let cutoff = (now_ts as i64).saturating_sub(interval_secs.saturating_mul(2)).max(0) as u32;
    map.retain(|_, ts| *ts >= cutoff);
    if map.len() > MAX_SEEN {
        let mut entries: Vec<(String, u32)> = map.iter().map(|(k, v)| (k.clone(), *v)).collect();
        entries.sort_by_key(|(_, ts)| *ts);
        let drop_n = map.len() - MAX_SEEN;
        for (k, _) in entries.into_iter().take(drop_n) {
            map.remove(&k);
        }
    }
}

/// Filter a LogData event into an upload job, or None to drop.
pub fn job_from_log_data(log: &LogData, params: RadioParams) -> Option<UploadJob> {
    let header = log.header.as_ref()?;
    if header.payload_type != PayloadType::Advert {
        return None;
    }
    let advert = log.advertisement.as_ref()?;
    if !is_infra_type(advert.adv_type) {
        return None;
    }
    if !gps_ok(advert.lat, advert.lon) {
        return None;
    }
    let packet = rebuild_mesh_packet(header, &log.payload)?;
    let packet_hex = hex::encode_upper(packet);
    let pubkey_hex = hex::encode(advert.public_key);
    Some(UploadJob {
        packet_hex,
        pubkey_hex,
        advert_ts: advert.timestamp,
        params,
        name: advert.name.clone(),
    })
}

/// Called from the radio session on LogData. Non-blocking enqueue.
pub fn try_enqueue(db: &Db, log: &LogData, params: RadioParams) {
    if !setting_bool(db, "svc_map_upload_enabled", false) {
        return;
    }
    if !setting_bool(db, "svc_map_upload_authorized", false) {
        return;
    }
    let Some(job) = job_from_log_data(log, params) else {
        return;
    };
    let interval = reupload_interval_secs(db);
    if !rate_limit_allows(&job.pubkey_hex, job.advert_ts, interval) {
        return;
    }
    let tx_guard = upload_tx().lock().unwrap_or_else(|e| e.into_inner());
    let Some(tx) = tx_guard.as_ref() else {
        return;
    };
    if let Err(err) = tx.send(job) {
        hq_log!(debug, "map upload queue: {err}");
    }
}

/// Spawn the upload worker on the services runtime. Safe to call once.
pub fn spawn_map_upload_worker(db: std::sync::Arc<Db>, radio: RadioHandle) {
    let (tx, rx) = unbounded_channel();
    {
        let mut guard = upload_tx().lock().unwrap_or_else(|e| e.into_inner());
        *guard = Some(tx);
    }
    tokio::spawn(async move {
        worker_loop(db, radio, rx).await;
    });
}

async fn worker_loop(db: std::sync::Arc<Db>, radio: RadioHandle, mut rx: UnboundedReceiver<UploadJob>) {
    let mut pending: Vec<UploadJob> = Vec::new();
    loop {
        tokio::select! {
            job = rx.recv() => {
                match job {
                    None => break,
                    Some(job) => {
                        if pending.len() < QUEUE_CAP {
                            pending.push(job);
                        } else {
                            hq_log!(debug, "map upload queue full; dropping");
                        }
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(200)), if !pending.is_empty() => {
                let job = pending.remove(0);
                process_job(&db, &radio, job).await;
            }
        }
    }
}

async fn process_job(db: &Db, radio: &RadioHandle, job: UploadJob) {
    if !setting_bool(db, "svc_map_upload_enabled", false)
        || !setting_bool(db, "svc_map_upload_authorized", false)
    {
        return;
    }
    let data = build_upload_data_string(&job.params, &job.packet_hex);
    let hash = sha256_bytes(&data);
    let dry_run = setting_bool(db, "svc_map_upload_dry_run", false);
    let prefix: String = job.pubkey_hex.chars().take(12).collect();
    let name = job.name.as_deref().unwrap_or("?");

    if dry_run {
        hq_log!(
            info,
            "map upload dry-run {prefix}… ({name}) data_len={} hash={}",
            data.len(),
            hex::encode(&hash[..8])
        );
        set_status(format!("Dry-run · last {prefix}…"));
        let _ = db.audit("svc_map_upload", &prefix, "dry_run");
        return;
    }

    let pubkey = {
        let snap = radio.snapshot();
        snap.public_key.clone()
    };
    if pubkey.is_empty() {
        set_status("Error · no primary companion pubkey");
        return;
    }

    let signature = match sign_upload_hash(radio, &hash).await {
        Ok(sig) => sig,
        Err(err) => {
            hq_log!(warn, "map upload sign: {err}");
            set_status(format!("Error · sign failed ({err})"));
            // Do not leave a half-broken authorized state when companion cannot sign.
            if err.contains("cannot sign") || err.contains("Disabled") {
                revoke_map_upload_auth(db);
                set_status("Unauthorized · companion cannot sign — enable private-key/sign support in firmware");
            }
            return;
        }
    };

    let body = json!({
        "data": data,
        "signature": hex::encode(&signature),
        "publicKey": pubkey.to_ascii_lowercase(),
    });
    let url = api_url(db);
    match post_upload(&url, &body) {
        Ok(status) => {
            hq_log!(info, "map upload ok {prefix}… http={status}");
            set_status(format!("Idle · last upload {prefix}… (HTTP {status})"));
            let _ = db.audit(
                "svc_map_upload",
                &prefix,
                &format!("http={status}"),
            );
        }
        Err(err) => {
            hq_log!(warn, "map upload POST: {err}");
            set_status(format!("Error · {err}"));
            let _ = db.audit("svc_map_upload", &prefix, &format!("err={err}"));
        }
    }
}

fn post_upload(url: &str, body: &serde_json::Value) -> Result<u16, String> {
    let payload = serde_json::to_string(body).map_err(|e| e.to_string())?;
    // ureq does not auto-follow 307/308 when the request has a body — follow manually.
    let mut next = normalize_map_api_url(url);
    for _ in 0..5 {
        let resp = ureq::post(&next)
            .set("User-Agent", HTTP_USER_AGENT)
            .set("Content-Type", "application/json")
            .set("Accept", "application/json")
            .timeout(Duration::from_secs(15))
            .send_string(&payload)
            .map_err(|e| e.to_string())?;
        let status = resp.status();
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            let location = resp
                .header("Location")
                .or_else(|| resp.header("location"))
                .map(|s| s.to_string())
                .ok_or_else(|| format!("HTTP {status}: redirect without Location"))?;
            next = resolve_redirect_url(&next, &location);
            continue;
        }
        if !(200..300).contains(&status) {
            let text = resp.into_string().unwrap_or_default();
            let brief: String = text.chars().take(120).collect();
            return Err(format!("HTTP {status}: {brief}"));
        }
        return Ok(status);
    }
    Err("too many redirects".into())
}

fn resolve_redirect_url(current: &str, location: &str) -> String {
    let loc = location.trim();
    if loc.starts_with("https://") || loc.starts_with("http://") {
        return normalize_map_api_url(loc);
    }
    // Relative Location — rare for this API, but keep POST working.
    if loc.starts_with('/') {
        if let Some((scheme, rest)) = current.split_once("://") {
            let host = rest.split('/').next().unwrap_or("");
            return normalize_map_api_url(&format!("{scheme}://{host}{loc}"));
        }
    }
    normalize_map_api_url(loc)
}

async fn sign_upload_hash(radio: &RadioHandle, hash: &[u8; 32]) -> Result<Vec<u8>, String> {
    // Prefer on-radio sign (hash never leaves after digest; key stays on device).
    match radio.sign_bytes(hash.to_vec()).await {
        Ok(sig) if sig.len() == 64 => return Ok(sig),
        Ok(sig) => {
            return Err(format!("companion returned {}-byte signature", sig.len()));
        }
        Err(err) => {
            // Fall back to RAM key if Authorize already exported one.
            if let Some(sig) = try_sign_with_ram_key(hash) {
                return Ok(sig);
            }
            Err(err)
        }
    }
}

fn try_sign_with_ram_key(hash: &[u8; 32]) -> Option<Vec<u8>> {
    let guard = ram_private_key().lock().unwrap_or_else(|e| e.into_inner());
    let key = guard.as_ref()?;
    // MeshCore export is typically seed (32) || pubkey (32). Use seed with dalek.
    sign_ed25519_seed(&key[..32], hash)
}

fn sign_ed25519_seed(seed32: &[u8], message: &[u8]) -> Option<Vec<u8>> {
    use ed25519_dalek::{Signer, SigningKey};
    let seed: [u8; 32] = seed32.try_into().ok()?;
    let signing = SigningKey::from_bytes(&seed);
    let sig = signing.sign(message);
    Some(sig.to_bytes().to_vec())
}

/// Operator Authorize: prove the primary companion can sign, then set the flag.
pub async fn authorize_map_upload(db: &Db, radio: &RadioHandle) -> Result<(), String> {
    let snap = radio.snapshot();
    if snap.state != "connected" || snap.public_key.is_empty() {
        return Err("Connect the primary companion before authorizing map uploads.".into());
    }
    // Probe: sign a fixed 32-byte challenge (SHA-256 of a constant).
    let challenge = sha256_bytes("metis-hq-map-upload-auth");
    match radio.sign_bytes(challenge.to_vec()).await {
        Ok(sig) if sig.len() == 64 => {
            clear_ram_private_key();
            set_setting_bool(db, "svc_map_upload_authorized", true);
            set_status("Authorized · idle");
            let prefix: String = snap.public_key.chars().take(12).collect();
            let _ = db.audit("svc_map_upload_auth", &prefix, "companion_sign");
            return Ok(());
        }
        Ok(_) => {}
        Err(err) => {
            hq_log!(debug, "map upload authorize sign: {err}");
        }
    }
    // Fallback: one-shot export into RAM only (never DB / keyring / backup).
    match radio.export_private_key_ram().await {
        Ok(key) => {
            store_ram_private_key(key);
            // Prove local sign works.
            let Some(sig) = try_sign_with_ram_key(&challenge) else {
                clear_ram_private_key();
                return Err("companion private key export unusable for signing".into());
            };
            if sig.len() != 64 {
                clear_ram_private_key();
                return Err("local sign produced unexpected signature length".into());
            }
            set_setting_bool(db, "svc_map_upload_authorized", true);
            set_status("Authorized · RAM key (cleared on disable/disconnect)");
            let prefix: String = snap.public_key.chars().take(12).collect();
            let _ = db.audit("svc_map_upload_auth", &prefix, "ram_export");
            Ok(())
        }
        Err(err) => {
            revoke_map_upload_auth(db);
            Err(format!(
                "companion cannot sign — enable private-key/sign support in firmware ({err})"
            ))
        }
    }
}

/// Radio session helper: build params from live status snapshot fields.
pub fn radio_params_from_mhz(freq_mhz: Option<f64>, bw_khz: Option<f64>, sf: Option<u8>, cr: Option<u8>) -> RadioParams {
    RadioParams {
        freq: freq_mhz.map(|m| (m * 1000.0).round() as u32).unwrap_or(0),
        bw: bw_khz.map(|k| (k * 1000.0).round() as u32).unwrap_or(0),
        sf: sf.unwrap_or(0),
        cr: cr.unwrap_or(0),
    }
}

pub fn upload_enabled(db: &Db) -> bool {
    setting_bool(db, "svc_map_upload_enabled", false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use meshcore_rs::events::MeshPacketHeader;
    use meshcore_rs::packets::{PayloadType, RouteType};
    use meshcore_rs::parsing::parse_mesh_packet_header;

    fn sample_header_flood() -> MeshPacketHeader {
        MeshPacketHeader {
            route_type: RouteType::Flood,
            payload_type: PayloadType::Advert,
            payload_version: 0,
            transport_code: None,
            path_len: 2,
            path_hash_size: 1,
            path: vec![0xAA, 0xBB],
        }
    }

    #[test]
    fn rebuild_round_trips_parse() {
        let header = sample_header_flood();
        let payload = vec![0x01, 0x02, 0x03, 0x04];
        let bytes = rebuild_mesh_packet(&header, &payload).expect("rebuild");
        let (parsed, rest) = parse_mesh_packet_header(&bytes).expect("parse");
        assert_eq!(parsed.route_type, RouteType::Flood);
        assert_eq!(parsed.payload_type, PayloadType::Advert);
        assert_eq!(parsed.path_len, 2);
        assert_eq!(parsed.path_hash_size, 1);
        assert_eq!(parsed.path, vec![0xAA, 0xBB]);
        assert_eq!(rest, payload.as_slice());
    }

    #[test]
    fn rebuild_with_transport_code() {
        let header = MeshPacketHeader {
            route_type: RouteType::TransportFlood,
            payload_type: PayloadType::Advert,
            payload_version: 1,
            transport_code: Some([0x11, 0x22, 0x33, 0x44]),
            path_len: 1,
            path_hash_size: 2,
            path: vec![0x01, 0x02],
        };
        let payload = vec![0x99];
        let bytes = rebuild_mesh_packet(&header, &payload).unwrap();
        let (parsed, rest) = parse_mesh_packet_header(&bytes).unwrap();
        assert_eq!(parsed.transport_code, Some([0x11, 0x22, 0x33, 0x44]));
        assert_eq!(parsed.path_hash_size, 2);
        assert_eq!(rest, &[0x99]);
    }

    #[test]
    fn filter_skips_chat_and_zero_gps() {
        // job_from_log_data needs full LogData — test helpers directly.
        assert!(!is_infra_type(1));
        assert!(is_infra_type(2));
        assert!(is_infra_type(3));
        assert!(is_infra_type(4));
        assert!(!gps_ok(None, None));
        assert!(!gps_ok(Some(0), Some(0)));
        assert!(gps_ok(Some(37_774_900), Some(-122_419_400)));
    }

    #[test]
    fn rate_limit_blocks_replay_and_early_reupload() {
        seen_map().lock().unwrap().clear();
        assert!(rate_limit_allows("aa", 1_000, 3600));
        assert!(!rate_limit_allows("aa", 1_000, 3600)); // replay
        assert!(!rate_limit_allows("aa", 1_500, 3600)); // too soon
        assert!(rate_limit_allows("aa", 1_000 + 3600, 3600));
        assert!(rate_limit_allows("bb", 50, 3600)); // other key
    }

    #[test]
    fn legacy_dev_host_rewrites_to_io() {
        assert_eq!(
            normalize_map_api_url("https://map.meshcore.dev/api/v1/uploader/node"),
            "https://map.meshcore.io/api/v1/uploader/node"
        );
        assert_eq!(
            normalize_map_api_url("https://map.meshcore.io/api/v1/uploader/node"),
            "https://map.meshcore.io/api/v1/uploader/node"
        );
        assert_eq!(
            resolve_redirect_url(
                "https://map.meshcore.dev/api/v1/uploader/node",
                "https://map.meshcore.io/api/v1/uploader/node"
            ),
            "https://map.meshcore.io/api/v1/uploader/node"
        );
    }

    #[test]
    fn compact_json_shape() {
        let params = RadioParams {
            freq: 915_000_000,
            cr: 8,
            sf: 9,
            bw: 250_000,
        };
        let s = build_upload_data_string(&params, "DEADBEEF");
        assert_eq!(
            s,
            r#"{"params":{"freq":915000000,"cr":8,"sf":9,"bw":250000},"links":["meshcore://DEADBEEF"]}"#
        );
        let hash = sha256_bytes(&s);
        assert_eq!(hash.len(), 32);
    }

    #[test]
    fn params_from_mhz_scales_like_selfinfo() {
        // SelfInfo radio_freq stored as mhz = value/1000 → recover Hz.
        let p = radio_params_from_mhz(Some(915_000.0), Some(250.0), Some(9), Some(8));
        assert_eq!(p.freq, 915_000_000);
        assert_eq!(p.bw, 250_000);
        assert_eq!(p.sf, 9);
        assert_eq!(p.cr, 8);
    }
}
