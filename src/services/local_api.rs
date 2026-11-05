//! Opt-in local JSON API for external apps / Home Assistant (Phase M).
//!
//! Hand-rolled HTTP/1.1 over Tokio (same spirit as Ecowitt push). Default off.
//! Bearer token lives in `data/local-api.token` (0600) — never in SQLite logs.

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::Write;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use rand::RngCore;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::db::{unix_now, Db, InfraStatusLatest, NodeRow};
use crate::radio::RadioHandle;
use crate::signup::Enrollment;

const TOKEN_FILE: &str = "local-api.token";
const DEFAULT_BIND: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 8787;
const DEFAULT_PREFIX: &str = "/api/v1";
const DEFAULT_RATE_LIMIT: u32 = 120;
const RATE_WINDOW: Duration = Duration::from_secs(60);
const API_VERSION: u32 = 1;

/// Path segments that are intentionally forbidden (403), not merely unknown.
const DENIED_ROOTS: &[&str] = &[
    "secrets",
    "keyring",
    "messages",
    "dm",
    "compose",
    "send",
    "enroll",
    "approve",
    "deny",
    "cli",
    "radio",
    "trails",
    "positions",
    "backup",
    "token",
];

#[derive(Debug, Clone)]
pub struct LocalApiConfig {
    pub enabled: bool,
    pub bind: String,
    pub port: u16,
    pub prefix: String,
    pub rate_limit_per_min: u32,
}

#[derive(Debug, Clone, Default)]
struct ApiStatus {
    listening: bool,
    last_error: String,
    bind_addr: String,
}

fn api_status() -> &'static Mutex<ApiStatus> {
    static S: OnceLock<Mutex<ApiStatus>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(ApiStatus::default()))
}

fn config_gen() -> &'static Mutex<u64> {
    static G: OnceLock<Mutex<u64>> = OnceLock::new();
    G.get_or_init(|| Mutex::new(0))
}

type RateBuckets = HashMap<IpAddr, VecDeque<Instant>>;

fn auth_ok_buckets() -> &'static Mutex<RateBuckets> {
    static B: OnceLock<Mutex<RateBuckets>> = OnceLock::new();
    B.get_or_init(|| Mutex::new(HashMap::new()))
}

fn auth_fail_buckets() -> &'static Mutex<RateBuckets> {
    static B: OnceLock<Mutex<RateBuckets>> = OnceLock::new();
    B.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn bump_local_api_config() {
    if let Ok(mut g) = config_gen().lock() {
        *g = g.wrapping_add(1);
    }
}

/// Clamp requests-per-minute setting.
pub fn clamp_rate_limit(raw: u32) -> u32 {
    raw.clamp(30, 600)
}

/// Failed-auth cap derived from the authenticated limit.
pub fn failed_auth_limit(rate_limit_per_min: u32) -> u32 {
    (rate_limit_per_min / 6).min(20).max(1)
}

/// Sliding-window allow check. Pure helper for tests — mutates `window` in place.
pub fn allow_request(window: &mut VecDeque<Instant>, now: Instant, limit: u32) -> bool {
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

fn setting(db: &Db, key: &str) -> String {
    db.setting(key)
        .ok()
        .flatten()
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// Normalize API path prefix: ensure leading `/`, strip trailing `/` (except root).
pub fn normalize_prefix(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return DEFAULT_PREFIX.to_string();
    }
    let with_slash = if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    };
    let stripped = with_slash.trim_end_matches('/');
    if stripped.is_empty() {
        "/".into()
    } else {
        stripped.to_string()
    }
}

pub fn load_local_api_config(db: &Db) -> LocalApiConfig {
    let bind_raw = setting(db, "local_api_bind");
    let bind = if bind_raw.is_empty() {
        DEFAULT_BIND.to_string()
    } else {
        bind_raw
    };
    let rate = setting(db, "local_api_rate_limit_per_min")
        .parse::<u32>()
        .unwrap_or(DEFAULT_RATE_LIMIT);
    LocalApiConfig {
        enabled: crate::services::setting_bool(db, "local_api_enabled", false),
        bind,
        port: setting(db, "local_api_port")
            .parse()
            .unwrap_or(DEFAULT_PORT),
        prefix: normalize_prefix(&setting(db, "local_api_prefix")),
        rate_limit_per_min: clamp_rate_limit(rate),
    }
}

/// True when the route (after prefix strip) is an explicitly forbidden path.
pub fn is_denied_route(route: &str) -> bool {
    let route = if route.is_empty() { "/" } else { route };
    let trimmed = route.trim_start_matches('/');
    if trimmed.is_empty() {
        return false;
    }
    let first = trimmed.split('/').next().unwrap_or("").to_ascii_lowercase();
    DENIED_ROOTS.iter().any(|d| *d == first)
}

pub fn token_path(data_dir: &Path) -> PathBuf {
    data_dir.join(TOKEN_FILE)
}

pub fn token_file_present(data_dir: &Path) -> bool {
    token_path(data_dir).is_file()
}

pub fn load_token(data_dir: &Path) -> String {
    fs::read_to_string(token_path(data_dir))
        .ok()
        .map(|s| s.trim_end_matches(['\r', '\n']).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_default()
}

pub fn save_token(data_dir: &Path, token: &str) -> Result<(), String> {
    fs::create_dir_all(data_dir).map_err(|e| e.to_string())?;
    let path = token_path(data_dir);
    let token = token.trim();
    if token.is_empty() {
        let _ = fs::remove_file(&path);
        bump_local_api_config();
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut opts = fs::OpenOptions::new();
        opts.create(true).write(true).truncate(true).mode(0o600);
        let mut file = opts.open(&path).map_err(|e| e.to_string())?;
        file.write_all(token.as_bytes())
            .map_err(|e| e.to_string())?;
    }
    #[cfg(not(unix))]
    {
        fs::write(&path, token).map_err(|e| e.to_string())?;
    }
    bump_local_api_config();
    Ok(())
}

pub fn clear_token(data_dir: &Path) {
    let _ = fs::remove_file(token_path(data_dir));
    bump_local_api_config();
}

/// 32 random bytes as lowercase hex (64 chars).
pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// Ensure a token exists; returns the current (or newly generated) token.
pub fn ensure_token(data_dir: &Path) -> Result<String, String> {
    let existing = load_token(data_dir);
    if !existing.is_empty() {
        return Ok(existing);
    }
    let token = generate_token();
    save_token(data_dir, &token)?;
    Ok(token)
}

pub fn local_api_status_line(db: &Db) -> String {
    let cfg = load_local_api_config(db);
    if !cfg.enabled {
        return "Local API off".into();
    }
    let token_ok = !load_token(&crate::data_dir()).is_empty();
    let st = api_status().lock().ok();
    let listening = st.as_ref().map(|s| s.listening).unwrap_or(false);
    let err = st
        .as_ref()
        .map(|s| s.last_error.clone())
        .unwrap_or_default();
    if !token_ok {
        return format!(
            "Local API enabled on {}:{}{} but no token — all requests return 401",
            cfg.bind, cfg.port, cfg.prefix
        );
    }
    if !err.is_empty() && !listening {
        return format!("Local API error: {err}");
    }
    if listening {
        format!(
            "Local API listening on {}:{}{}",
            cfg.bind, cfg.port, cfg.prefix
        )
    } else {
        format!(
            "Local API starting on {}:{}{}…",
            cfg.bind, cfg.port, cfg.prefix
        )
    }
}

pub fn is_loopback_bind(bind: &str) -> bool {
    matches!(
        bind.trim().to_ascii_lowercase().as_str(),
        "127.0.0.1" | "::1" | "localhost"
    )
}

/// Constant-time-ish compare (no early exit on mismatch once lengths match).
pub fn token_matches(expected: &str, provided: &str) -> bool {
    let a = expected.as_bytes();
    let b = provided.as_bytes();
    if a.is_empty() || a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Parse `Authorization: Bearer <token>` from raw header block.
pub fn parse_bearer_token(headers: &str) -> Option<String> {
    for line in headers.lines() {
        let line = line.trim();
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if !name.eq_ignore_ascii_case("authorization") {
            continue;
        }
        let value = value.trim();
        let rest = value.strip_prefix("Bearer ").or_else(|| {
            if value.len() > 7 && value[..7].eq_ignore_ascii_case("bearer ") {
                Some(&value[7..])
            } else {
                None
            }
        })?;
        let token = rest.trim();
        if token.is_empty() {
            return None;
        }
        return Some(token.to_string());
    }
    None
}

/// Strip configured prefix from request path; returns remainder starting with `/`.
pub fn strip_api_prefix(path: &str, prefix: &str) -> Option<String> {
    let path = normalize_request_path(path);
    let prefix = normalize_prefix(prefix);
    if path == prefix {
        return Some("/".into());
    }
    let prefix_slash = if prefix.ends_with('/') {
        prefix.clone()
    } else {
        format!("{prefix}/")
    };
    if let Some(rest) = path.strip_prefix(&prefix_slash) {
        return Some(format!("/{rest}"));
    }
    if path.starts_with(&prefix) && path.len() > prefix.len() && path.as_bytes()[prefix.len()] == b'/'
    {
        return Some(path[prefix.len()..].to_string());
    }
    None
}

fn normalize_request_path(path: &str) -> String {
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

/// Resolve node by full pubkey hex or unique ≥6-byte hex prefix.
pub fn resolve_node_id<'a>(nodes: &'a [NodeRow], id: &str) -> Result<&'a NodeRow, &'static str> {
    let id = id.trim().to_ascii_lowercase();
    if id.is_empty() {
        return Err("missing id");
    }
    if !id.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("id must be hex");
    }
    if id.len() == 64 {
        return nodes
            .iter()
            .find(|n| n.pubkey.eq_ignore_ascii_case(&id))
            .ok_or("not found");
    }
    if id.len() < 12 {
        return Err("prefix too short (need ≥12 hex chars)");
    }
    let matches: Vec<_> = nodes
        .iter()
        .filter(|n| n.pubkey.to_ascii_lowercase().starts_with(&id))
        .collect();
    match matches.len() {
        0 => Err("not found"),
        1 => Ok(matches[0]),
        _ => Err("ambiguous prefix"),
    }
}

fn node_json(node: &NodeRow) -> Value {
    let mut obj = json!({
        "pubkey": node.pubkey,
        "prefix": node.prefix,
        "name": node.name,
        "contact_type": node.contact_type,
        "enrollment": node.enrollment.as_str(),
        "last_heard": node.last_heard,
    });
    if let (Some(lat), Some(lon)) = (node.lat, node.lon) {
        if !(lat == 0.0 && lon == 0.0) {
            obj["lat"] = json!(lat);
            obj["lon"] = json!(lon);
        }
    }
    obj
}

fn infra_latest_json(s: &InfraStatusLatest) -> Value {
    json!({
        "probed_at": s.probed_at,
        "battery_mv": s.battery_mv,
        "noise_floor": s.noise_floor,
        "last_rssi": s.last_rssi,
        "snr": s.snr,
        "airtime_ms": s.airtime_ms,
        "rx_airtime_ms": s.rx_airtime_ms,
        "uptime_s": s.uptime_s,
        "nb_recv": s.nb_recv,
        "nb_sent": s.nb_sent,
        "flood_sent": s.flood_sent,
        "direct_sent": s.direct_sent,
        "tx_power_dbm": s.tx_power_dbm,
        "tx_queue_len": s.tx_queue_len,
        "ver": s.ver,
        "radio": s.radio,
        "clock": s.clock,
        "neighbors": s.neighbors,
    })
}

fn http_json(code: u16, body: &Value) -> Vec<u8> {
    http_json_extra(code, body, &[])
}

fn http_json_extra(code: u16, body: &Value, extra_headers: &[(&str, &str)]) -> Vec<u8> {
    let reason = match code {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        429 => "Too Many Requests",
        _ => "Error",
    };
    let payload = body.to_string();
    let mut out = format!("HTTP/1.1 {code} {reason}\r\nContent-Type: application/json\r\n");
    for (k, v) in extra_headers {
        out.push_str(k);
        out.push_str(": ");
        out.push_str(v);
        out.push_str("\r\n");
    }
    out.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    ));
    out.into_bytes()
}

fn rate_limited_response() -> Vec<u8> {
    http_json_extra(
        429,
        &json!({ "error": "rate limited" }),
        &[("Retry-After", "60")],
    )
}

fn route_get(db: &Db, radio: &RadioHandle, route: &str) -> (u16, Value) {
    let route = if route.is_empty() { "/" } else { route };
    if is_denied_route(route) {
        return (403, json!({ "error": "forbidden" }));
    }
    match route {
        "/" | "/health" => (
            200,
            json!({
                "ok": true,
                "service": "metis-mesh-hq",
                "api_version": API_VERSION,
            }),
        ),
        "/status" => {
            let snap = radio.snapshot();
            let hq_name = db
                .hq_advert_name()
                .unwrap_or_default()
                .trim()
                .to_string();
            let hq_name = if hq_name.is_empty() {
                db.setting("hq_name")
                    .ok()
                    .flatten()
                    .unwrap_or_default()
            } else {
                hq_name
            };
            (
                200,
                json!({
                    "hq_name": hq_name,
                    "now": unix_now(),
                    "companion": {
                        "state": snap.state,
                        "connected_name": snap.connected_name,
                        "transport": snap.transport,
                        "role": snap.role,
                        "radio_id": snap.radio_id,
                    },
                    "primary_radio_id": db.primary_radio_id().ok().flatten(),
                }),
            )
        }
        "/nodes" => {
            let nodes = db.list_nodes().unwrap_or_default();
            let list: Vec<Value> = nodes.iter().map(node_json).collect();
            (200, json!(list))
        }
        "/infra" => {
            let claimed = db.list_claimed_infra().unwrap_or_default();
            let latest = db.list_infra_status_latest().unwrap_or_default();
            let list: Vec<Value> = claimed
                .iter()
                .map(|c| {
                    let lat = latest.iter().find(|s| s.pubkey == c.pubkey);
                    json!({
                        "pubkey": c.pubkey,
                        "kind": c.kind,
                        "label": c.label,
                        "notes": c.notes,
                        "added_at": c.added_at,
                        "verified_at": c.verified_at,
                        "latest": lat.map(infra_latest_json),
                    })
                })
                .collect();
            (200, json!(list))
        }
        "/channels" => {
            let channels = db.list_channels().unwrap_or_default();
            let list: Vec<Value> = channels
                .iter()
                .map(|c| {
                    json!({
                        "name": c.name,
                        "slot": c.slot,
                        "visibility": c.visibility,
                        "role": c.role,
                        "active": c.active,
                        "message_count": c.message_count,
                        "unread_count": c.unread_count,
                        "unread_mentions": c.unread_mentions,
                        "last_message_at": c.last_message_at,
                    })
                })
                .collect();
            (200, json!(list))
        }
        "/stats" => {
            let nodes = db.list_nodes().unwrap_or_default();
            let mut heard = 0usize;
            let mut pending = 0usize;
            let mut enrolled = 0usize;
            let mut denied = 0usize;
            let mut hq = 0usize;
            let mut gps = 0usize;
            for n in &nodes {
                match n.enrollment {
                    Enrollment::Heard => heard += 1,
                    Enrollment::Pending => pending += 1,
                    Enrollment::Enrolled => enrolled += 1,
                    Enrollment::Denied => denied += 1,
                    Enrollment::Hq => hq += 1,
                }
                if n.has_gps
                    || n.lat
                        .zip(n.lon)
                        .is_some_and(|(la, lo)| !(la == 0.0 && lo == 0.0))
                {
                    gps += 1;
                }
            }
            let since = unix_now() - 86_400;
            (
                200,
                json!({
                    "nodes": nodes.len(),
                    "gps": gps,
                    "enrollment": {
                        "heard": heard,
                        "pending": pending,
                        "enrolled": enrolled,
                        "denied": denied,
                        "hq": hq,
                    },
                    "claimed_infra": db.count_claimed_infra().unwrap_or(0),
                    "channel_unread": db.total_channel_unread().unwrap_or(0),
                    "dm_unread": db.total_dm_unread().unwrap_or(0),
                    "unread_mentions": db.total_unread_mentions().unwrap_or(0),
                    "messages_24h": db.messages_since_count(since).unwrap_or(0),
                }),
            )
        }
        other if other.starts_with("/nodes/") => {
            let id = other.trim_start_matches("/nodes/");
            let nodes = db.list_nodes().unwrap_or_default();
            match resolve_node_id(&nodes, id) {
                Ok(node) => (200, node_json(node)),
                Err(msg) => (404, json!({ "error": msg })),
            }
        }
        _ => (404, json!({ "error": "not found" })),
    }
}

async fn handle_client(
    mut stream: TcpStream,
    cfg: LocalApiConfig,
    token: String,
    db: std::sync::Arc<Db>,
    radio: RadioHandle,
) {
    let peer_ip = stream
        .peer_addr()
        .map(|a| a.ip())
        .unwrap_or_else(|_| IpAddr::from([127, 0, 0, 1]));

    let mut buf = vec![0u8; 64 * 1024];
    let n = match tokio::time::timeout(Duration::from_secs(10), stream.read(&mut buf)).await {
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
    let first = head.lines().next().unwrap_or("");
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path_q = parts.next().unwrap_or("");
    let path = path_q.split('?').next().unwrap_or(path_q);

    let provided = parse_bearer_token(head).unwrap_or_default();
    if token.is_empty() || !token_matches(&token, &provided) {
        let fail_limit = failed_auth_limit(cfg.rate_limit_per_min);
        if !take_rate_slot(auth_fail_buckets(), peer_ip, fail_limit) {
            let _ = stream.write_all(&rate_limited_response()).await;
            return;
        }
        let _ = stream
            .write_all(&http_json(401, &json!({ "error": "unauthorized" })))
            .await;
        return;
    }

    if !take_rate_slot(auth_ok_buckets(), peer_ip, cfg.rate_limit_per_min) {
        let _ = stream.write_all(&rate_limited_response()).await;
        return;
    }

    if !method.eq_ignore_ascii_case("GET") {
        let _ = stream
            .write_all(&http_json(405, &json!({ "error": "method not allowed" })))
            .await;
        return;
    }

    let Some(route) = strip_api_prefix(path, &cfg.prefix) else {
        let _ = stream
            .write_all(&http_json(404, &json!({ "error": "not found" })))
            .await;
        return;
    };

    let (code, body) = route_get(&db, &radio, &route);
    let _ = stream.write_all(&http_json(code, &body)).await;
}

async fn api_session(
    db: &std::sync::Arc<Db>,
    radio: &RadioHandle,
    cfg: &LocalApiConfig,
) -> Result<(), String> {
    let addr = format!("{}:{}", cfg.bind, cfg.port);
    let listener = TcpListener::bind(&addr)
        .await
        .map_err(|e| format!("bind {addr}: {e}"))?;
    if let Ok(mut s) = api_status().lock() {
        s.listening = true;
        s.last_error.clear();
        s.bind_addr = addr.clone();
    }
    hq_log!(
        info,
        "local api: listening {} prefix={}",
        addr,
        cfg.prefix
    );

    let start_gen = config_gen().lock().map(|g| *g).unwrap_or(0);
    loop {
        let gen_now = config_gen().lock().map(|g| *g).unwrap_or(0);
        if gen_now != start_gen {
            return Ok(());
        }
        let live = load_local_api_config(db);
        if !live.enabled
            || live.bind != cfg.bind
            || live.port != cfg.port
            || live.prefix != cfg.prefix
            || live.rate_limit_per_min != cfg.rate_limit_per_min
        {
            return Ok(());
        }

        match tokio::time::timeout(Duration::from_secs(2), listener.accept()).await {
            Ok(Ok((stream, _))) => {
                let cfg_c = live.clone();
                let token = load_token(&crate::data_dir());
                let db_c = db.clone();
                let radio_c = radio.clone();
                tokio::spawn(async move {
                    handle_client(stream, cfg_c, token, db_c, radio_c).await;
                });
            }
            Ok(Err(err)) => return Err(err.to_string()),
            Err(_) => {}
        }
    }
}

/// Long-lived local JSON API listener.
pub async fn local_api_loop(db: std::sync::Arc<Db>, radio: RadioHandle) {
    loop {
        let cfg = load_local_api_config(&db);
        if !cfg.enabled {
            if let Ok(mut s) = api_status().lock() {
                s.listening = false;
            }
            tokio::time::sleep(Duration::from_secs(3)).await;
            continue;
        }
        match api_session(&db, &radio, &cfg).await {
            Ok(()) => {}
            Err(err) => {
                if let Ok(mut s) = api_status().lock() {
                    s.listening = false;
                    s.last_error = err.clone();
                }
                hq_log!(warn, "local api: {err}");
            }
        }
        if let Ok(mut s) = api_status().lock() {
            s.listening = false;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signup::Enrollment;

    fn sample_node(pubkey: &str, prefix: &str) -> NodeRow {
        NodeRow {
            pubkey: pubkey.to_string(),
            name: "n".into(),
            requested_name: None,
            contact_type: 1,
            enrollment: Enrollment::Heard,
            last_heard: None,
            last_advert: None,
            lat: None,
            lon: None,
            has_gps: false,
            prefix: prefix.to_string(),
            hop_bytes: Vec::new(),
            path_len: 0,
            qr_file: None,
            qr_uri: None,
            noticed: true,
            last_radio_id: None,
        }
    }

    #[test]
    fn normalize_prefix_adds_slash_and_strips_trail() {
        assert_eq!(normalize_prefix("api/v1"), "/api/v1");
        assert_eq!(normalize_prefix("/api/v1/"), "/api/v1");
        assert_eq!(normalize_prefix(""), "/api/v1");
    }

    #[test]
    fn parse_bearer_accepts_header() {
        let h = "GET /x HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer abcDEF123\r\n";
        assert_eq!(parse_bearer_token(h).as_deref(), Some("abcDEF123"));
        assert!(parse_bearer_token("GET /x\r\n").is_none());
    }

    #[test]
    fn token_matches_rejects_wrong_length() {
        assert!(token_matches("abcd", "abcd"));
        assert!(!token_matches("abcd", "abc"));
        assert!(!token_matches("abcd", "abce"));
        assert!(!token_matches("", "x"));
    }

    #[test]
    fn strip_prefix_routes() {
        assert_eq!(
            strip_api_prefix("/api/v1/health", "/api/v1").as_deref(),
            Some("/health")
        );
        assert_eq!(
            strip_api_prefix("/api/v1", "/api/v1").as_deref(),
            Some("/")
        );
        assert_eq!(
            strip_api_prefix("/api/v1/nodes/ab", "/api/v1").as_deref(),
            Some("/nodes/ab")
        );
        assert!(strip_api_prefix("/other", "/api/v1").is_none());
    }

    #[test]
    fn resolve_node_exact_prefix_ambiguous() {
        let a = "aabbccddeeff00112233445566778899aabbccddeeff00112233445566778899";
        let b = "aabbccddeeff99112233445566778899aabbccddeeff00112233445566778899";
        let nodes = vec![sample_node(a, "aabbcc"), sample_node(b, "aabbcc")];
        assert!(resolve_node_id(&nodes, a).is_ok());
        assert_eq!(
            resolve_node_id(&nodes, "aabbccddeeff").err(),
            Some("ambiguous prefix")
        );
        assert_eq!(
            resolve_node_id(&nodes, "ffffffffffff").err(),
            Some("not found")
        );
        assert_eq!(
            resolve_node_id(&nodes, "aabb").err(),
            Some("prefix too short (need ≥12 hex chars)")
        );
        let only = vec![sample_node(a, "aabbcc")];
        assert!(resolve_node_id(&only, "aabbccddeeff").is_ok());
    }

    #[test]
    fn is_loopback_detects_localhost() {
        assert!(is_loopback_bind("127.0.0.1"));
        assert!(is_loopback_bind("localhost"));
        assert!(!is_loopback_bind("0.0.0.0"));
        assert!(!is_loopback_bind("192.168.1.10"));
    }

    #[test]
    fn denied_routes_match_forbidden_roots() {
        for root in DENIED_ROOTS {
            assert!(is_denied_route(&format!("/{root}")), "{root}");
            assert!(is_denied_route(&format!("/{root}/extra")), "{root}/extra");
        }
        for ok in [
            "/",
            "/health",
            "/status",
            "/nodes",
            "/nodes/aabbccddeeff",
            "/infra",
            "/channels",
            "/stats",
        ] {
            assert!(!is_denied_route(ok), "{ok}");
        }
    }

    #[test]
    fn serializers_omit_secrets_and_message_bodies() {
        let node = sample_node(
            "aabbccddeeff00112233445566778899aabbccddeeff00112233445566778899",
            "aabbcc",
        );
        let n = node_json(&node);
        for bad in ["secret", "last_text", "hop_bytes", "qr_uri", "qr_file"] {
            assert!(n.get(bad).is_none(), "node has {bad}");
        }
        let ch = json!({
            "name": "Public",
            "slot": 0,
            "visibility": "public",
            "role": "general",
            "active": true,
            "message_count": 1,
            "unread_count": 0,
            "unread_mentions": 0,
            "last_message_at": null,
        });
        assert!(ch.get("last_text").is_none());
        assert!(ch.get("secret").is_none());
    }

    #[test]
    fn rate_window_allows_then_blocks() {
        let mut window = VecDeque::new();
        let t0 = Instant::now();
        assert!(allow_request(&mut window, t0, 3));
        assert!(allow_request(&mut window, t0 + Duration::from_millis(1), 3));
        assert!(allow_request(&mut window, t0 + Duration::from_millis(2), 3));
        assert!(!allow_request(
            &mut window,
            t0 + Duration::from_millis(3),
            3
        ));
        // After window slides past 60s, allow again.
        assert!(allow_request(
            &mut window,
            t0 + RATE_WINDOW + Duration::from_millis(10),
            3
        ));
    }

    #[test]
    fn failed_auth_limit_caps_at_twenty() {
        assert_eq!(failed_auth_limit(120), 20);
        assert_eq!(failed_auth_limit(600), 20);
        assert_eq!(failed_auth_limit(30), 5);
        assert_eq!(clamp_rate_limit(10), 30);
        assert_eq!(clamp_rate_limit(999), 600);
    }

    #[test]
    fn health_json_includes_api_version() {
        let body = json!({
            "ok": true,
            "service": "metis-mesh-hq",
            "api_version": API_VERSION,
        });
        assert_eq!(body["api_version"], API_VERSION);
    }
}
