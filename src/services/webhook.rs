//! Opt-in outbound webhooks for mesh → external automation (Local API Integrations).
//!
//! Operator-defined connections select which live channel/DM RX (and operator channel
//! TX from HQ) are POSTed as JSON to a configured URL. HMAC-SHA256 when a secret
//! file is present. Never forwards secrets; blocked senders and connect-queue drain
//! are skipped.

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use crate::db::Db;
use crate::services::{setting_bool, HTTP_USER_AGENT};
use crate::signup::Enrollment;

const SECRET_FILE: &str = "webhook.hmac";
const DEFAULT_RATE_PER_MIN: u32 = 30;
const DEFAULT_TIMEOUT_SECS: u32 = 12;
const QUEUE_CAP: usize = 64;
const RECENT_CAP: usize = 12;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WebhookHeader {
    pub key: String,
    pub value: String,
    /// Mask in the UI; value still stored in rules JSON (SQLite / backup).
    #[serde(default)]
    pub secret: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WebhookField {
    pub key: String,
    /// String value; supports `{{text}}` / `{{command}}` placeholders. Merged into JSON body.
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WebhookRule {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// `channel` and/or `direct`. Empty → both.
    #[serde(default)]
    pub kinds: Vec<String>,
    /// Channel names (case-insensitive). `"*"` matches any. Empty → any channel.
    #[serde(default)]
    pub channels: Vec<String>,
    /// Enrollment labels: heard / pending / enrolled / denied / hq / `*` / `any`.
    /// Empty → enrolled only (safe default).
    #[serde(default)]
    pub enrollment: Vec<String>,
    /// Optional allowlist of sender pubkeys / key prefixes (hex, case-insensitive).
    /// Empty → any sender that passes enrollment. Non-empty → sender must match one entry.
    #[serde(default)]
    pub allowed_senders: Vec<String>,
    /// If non-empty, text must start with this prefix (after trim).
    #[serde(default)]
    pub text_prefix: String,
    /// POST body shape: `full` | `text` | `command` | `custom`.
    #[serde(default = "default_payload_mode")]
    pub payload_mode: String,
    /// When `payload_mode` is `custom`: JSON template with `{{text}}`, `{{command}}`, …
    #[serde(default)]
    pub payload_template: String,
    /// Destination URL for this connection (`http://` or `https://`).
    #[serde(default)]
    pub url: String,
    /// Extra HTTP headers on the POST (values may use `{{…}}` placeholders).
    #[serde(default)]
    pub headers: Vec<WebhookHeader>,
    /// Extra JSON object fields merged into the POST body.
    #[serde(default)]
    pub fields: Vec<WebhookField>,
}

fn default_true() -> bool {
    true
}

fn default_payload_mode() -> String {
    "full".into()
}

pub fn default_payload_template() -> String {
    "{\n  \"command\": \"{{command}}\",\n  \"text\": \"{{text}}\",\n  \"from\": \"{{from.name}}\",\n  \"channel\": \"{{channel}}\"\n}\n"
        .into()
}

/// HQ mesh RX placeholders usable in custom body fields / header values.
/// `(token, suggested_json_key, short_label)`.
pub fn webhook_hq_tokens() -> &'static [(&'static str, &'static str, &'static str)] {
    &[
        ("{{command}}", "command", "Command (after text prefix)"),
        ("{{text}}", "text", "Full message text"),
        ("{{body}}", "body", "Message body"),
        ("{{kind}}", "kind", "channel or direct"),
        ("{{channel}}", "channel", "Channel name"),
        ("{{from.name}}", "from_name", "Sender name"),
        ("{{from.pubkey}}", "from_pubkey", "Sender public key"),
        ("{{from.prefix}}", "from_prefix", "Sender key prefix"),
        ("{{from.enrollment}}", "enrollment", "heard / pending / enrolled / …"),
        ("{{hop_count}}", "hop_count", "Hop count"),
        ("{{timestamp}}", "timestamp", "Unix seconds (event time)"),
        ("{{ts}}", "ts", "Unix seconds (alias)"),
        ("{{message_id}}", "message_id", "HQ message id"),
        ("{{rule.id}}", "rule_id", "Matching rule id"),
        ("{{rule.name}}", "rule_name", "Matching rule name"),
    ]
}

/// Materialize empty `kinds` to both (matcher treats empty as both; wizard must stay explicit).
pub fn ensure_explicit_kinds(rule: &mut WebhookRule) {
    if rule.kinds.is_empty() {
        rule.kinds = vec!["channel".into(), "direct".into()];
    }
}

impl WebhookRule {
    pub fn default_hash_commands() -> Self {
        Self {
            id: "bot-cmds".into(),
            name: "Bot commands (!…)".into(),
            // Off until the operator sets a per-connection POST URL.
            enabled: false,
            kinds: vec!["channel".into(), "direct".into()],
            channels: vec!["*".into()],
            enrollment: vec!["enrolled".into()],
            allowed_senders: Vec::new(),
            text_prefix: "!".into(),
            payload_mode: default_payload_mode(),
            payload_template: String::new(),
            url: String::new(),
            headers: Vec::new(),
            fields: Vec::new(),
        }
    }

    pub fn blank() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            enabled: true,
            kinds: vec!["channel".into(), "direct".into()],
            channels: vec!["*".into()],
            enrollment: vec!["enrolled".into()],
            allowed_senders: Vec::new(),
            text_prefix: "!".into(),
            payload_mode: default_payload_mode(),
            payload_template: String::new(),
            url: String::new(),
            headers: Vec::new(),
            fields: Vec::new(),
        }
    }
}

/// Normalize a pasted/picked sender id (trim, strip `0x`, lowercase).
pub fn normalize_sender_id(raw: &str) -> String {
    raw.trim()
        .trim_start_matches("0x")
        .trim_start_matches("0X")
        .to_ascii_lowercase()
}

/// True when URL is usable for an outbound POST.
pub fn connection_url_ok(url: &str) -> bool {
    let u = url.trim();
    u.starts_with("http://") || u.starts_with("https://")
}

/// Short host/path label for connection cards.
pub fn connection_url_label(url: &str) -> String {
    let u = url.trim();
    if u.is_empty() {
        return "(no URL)".into();
    }
    let rest = u
        .strip_prefix("https://")
        .or_else(|| u.strip_prefix("http://"))
        .unwrap_or(u);
    if rest.len() <= 48 {
        rest.to_string()
    } else {
        format!("{}…", rest.chars().take(47).collect::<String>())
    }
}

/// Stable id from a display name (`Hash commands` → `hash-commands`).
pub fn slug_rule_id(name: &str) -> String {
    let mut out = String::new();
    for ch in name.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if ch == ' ' || ch == '-' || ch == '_' {
            if !out.ends_with('-') {
                out.push('-');
            }
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "rule".into()
    } else {
        out.chars().take(48).collect()
    }
}

#[derive(Debug, Clone)]
pub struct WebhookConfig {
    pub enabled: bool,
    pub rate_per_min: u32,
    pub timeout_secs: u32,
    pub rules: Vec<WebhookRule>,
    pub secret: String,
}

/// One outbound attempt (live match or operator Test POST). In-memory only.
#[derive(Debug, Clone)]
pub struct DeliveryRecord {
    pub at_unix: i64,
    pub rule_id: String,
    pub rule_name: String,
    pub url_label: String,
    pub http: u16,
    pub ok: bool,
    pub detail: String,
    /// `"live"` or `"test"`.
    pub kind: &'static str,
}

#[derive(Debug, Clone)]
pub struct MeshEvent {
    pub message_id: i64,
    pub kind: &'static str,
    pub channel: Option<String>,
    pub channel_idx: Option<i64>,
    pub from_pubkey: Option<String>,
    pub from_name: Option<String>,
    pub from_prefix: String,
    pub enrollment: Enrollment,
    pub text: String,
    pub body: String,
    pub hop_count: Option<i64>,
    /// Unix seconds when HQ stamped this event (RX or local TX / test).
    pub timestamp: i64,
}

#[derive(Debug, Clone)]
struct DeliveryJob {
    rule_id: String,
    rule_name: String,
    url: String,
    text_prefix: String,
    payload_mode: String,
    payload_template: String,
    headers: Vec<WebhookHeader>,
    fields: Vec<WebhookField>,
    event: MeshEvent,
    /// Operator Test POST: skip master enable + rate limit.
    is_test: bool,
}

#[derive(Debug, Clone, Default)]
struct RuntimeStatus {
    last_ok: Option<Instant>,
    last_error: String,
    last_http: u16,
    delivered: u64,
    dropped: u64,
    recent: VecDeque<DeliveryRecord>,
}

fn status() -> &'static Mutex<RuntimeStatus> {
    static S: OnceLock<Mutex<RuntimeStatus>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(RuntimeStatus::default()))
}

fn config_gen() -> &'static Mutex<u64> {
    static G: OnceLock<Mutex<u64>> = OnceLock::new();
    G.get_or_init(|| Mutex::new(0))
}

fn delivery_tx() -> &'static Mutex<Option<UnboundedSender<DeliveryJob>>> {
    static TX: OnceLock<Mutex<Option<UnboundedSender<DeliveryJob>>>> = OnceLock::new();
    TX.get_or_init(|| Mutex::new(None))
}

fn rate_window() -> &'static Mutex<VecDeque<Instant>> {
    static W: OnceLock<Mutex<VecDeque<Instant>>> = OnceLock::new();
    W.get_or_init(|| Mutex::new(VecDeque::new()))
}

pub fn bump_webhook_config() {
    if let Ok(mut g) = config_gen().lock() {
        *g = g.wrapping_add(1);
    }
}

fn setting(db: &Db, key: &str) -> String {
    db.setting(key)
        .ok()
        .flatten()
        .unwrap_or_default()
        .trim()
        .to_string()
}

pub fn clamp_webhook_rate(raw: u32) -> u32 {
    raw.clamp(1, 120)
}

pub fn clamp_webhook_timeout(raw: u32) -> u32 {
    raw.clamp(3, 60)
}

fn open_webhooks_tab_flag() -> &'static AtomicBool {
    static F: AtomicBool = AtomicBool::new(false);
    &F
}

/// Dashboard chip / jump: open Settings → Services → Webhooks on next Settings paint.
pub fn request_open_webhooks_tab() {
    open_webhooks_tab_flag().store(true, Ordering::SeqCst);
}

/// Consume a pending deep-link request (true once).
pub fn take_open_webhooks_tab() -> bool {
    open_webhooks_tab_flag().swap(false, Ordering::SeqCst)
}

pub fn webhook_secret_path(data_dir: &Path) -> PathBuf {
    data_dir.join(SECRET_FILE)
}

pub fn save_webhook_secret(data_dir: &Path, secret: &str) -> Result<(), String> {
    fs::create_dir_all(data_dir).map_err(|e| e.to_string())?;
    let path = webhook_secret_path(data_dir);
    let secret = secret.trim();
    if secret.is_empty() {
        let _ = fs::remove_file(&path);
        bump_webhook_config();
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut opts = fs::OpenOptions::new();
        opts.create(true).write(true).truncate(true).mode(0o600);
        let mut file = opts.open(&path).map_err(|e| e.to_string())?;
        file.write_all(secret.as_bytes())
            .map_err(|e| e.to_string())?;
    }
    #[cfg(not(unix))]
    {
        fs::write(&path, secret).map_err(|e| e.to_string())?;
    }
    bump_webhook_config();
    Ok(())
}

pub fn load_webhook_secret(data_dir: &Path) -> String {
    fs::read_to_string(webhook_secret_path(data_dir))
        .ok()
        .map(|s| s.trim_end_matches(['\r', '\n']).to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_default()
}

pub fn clear_webhook_secret(data_dir: &Path) {
    let _ = fs::remove_file(webhook_secret_path(data_dir));
    bump_webhook_config();
}

pub fn webhook_secret_present(data_dir: &Path) -> bool {
    webhook_secret_path(data_dir).is_file()
}

pub fn generate_webhook_secret() -> String {
    let mut bytes = [0u8; 32];
    RngCore::fill_bytes(&mut rand::thread_rng(), &mut bytes);
    hex::encode(bytes)
}

pub fn default_rules_json() -> String {
    "[]".into()
}

pub fn parse_rules_json(raw: &str) -> Result<Vec<WebhookRule>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let rules: Vec<WebhookRule> =
        serde_json::from_str(trimmed).map_err(|e| format!("Invalid rules JSON: {e}"))?;
    validate_rules(&rules)?;
    Ok(rules)
}

fn validate_rules(rules: &[WebhookRule]) -> Result<(), String> {
    if rules.len() > 32 {
        return Err("At most 32 webhook connections.".into());
    }
    for rule in rules {
        if rule.id.trim().is_empty() {
            return Err("Each connection needs a non-empty id.".into());
        }
        if rule.id.len() > 64 {
            return Err("Connection id is too long (max 64).".into());
        }
        if rule.text_prefix.len() > 32 {
            return Err("text_prefix is too long (max 32).".into());
        }
        if rule.url.len() > 512 {
            return Err(format!(
                "Connection {}: URL is too long (max 512).",
                rule.id
            ));
        }
        if rule.enabled && !connection_url_ok(&rule.url) {
            return Err(format!(
                "Connection {}: enabled connections need an http(s) POST URL.",
                rule.id
            ));
        }
        if !rule.url.trim().is_empty() && !connection_url_ok(&rule.url) {
            return Err(format!(
                "Connection {}: URL must start with http:// or https://.",
                rule.id
            ));
        }
        let mode = normalize_payload_mode(&rule.payload_mode);
        if mode == "custom" && rule.payload_template.len() > 4096 {
            return Err(format!(
                "Connection {}: payload template is too long (max 4096).",
                rule.id
            ));
        }
        if mode == "custom"
            && rule.payload_template.trim().is_empty()
            && rule.fields.is_empty()
        {
            return Err(format!(
                "Connection {}: custom payload needs at least one body field (or a template in Raw JSON).",
                rule.id
            ));
        }
        if rule.headers.len() > 16 {
            return Err(format!(
                "Connection {}: at most 16 custom headers.",
                rule.id
            ));
        }
        if rule.fields.len() > 16 {
            return Err(format!(
                "Connection {}: at most 16 custom fields.",
                rule.id
            ));
        }
        if rule.allowed_senders.len() > 64 {
            return Err(format!(
                "Connection {}: at most 64 allowed senders.",
                rule.id
            ));
        }
        for s in &rule.allowed_senders {
            let n = normalize_sender_id(s);
            if n.is_empty() {
                return Err(format!(
                    "Connection {}: allowed sender entry is empty.",
                    rule.id
                ));
            }
            if n.len() > 64 {
                return Err(format!(
                    "Connection {}: allowed sender id is too long (max 64 hex chars).",
                    rule.id
                ));
            }
            if !n.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(format!(
                    "Connection {}: allowed sender must be hex pubkey or prefix ({n}).",
                    rule.id
                ));
            }
        }
        for h in &rule.headers {
            let key = h.key.trim();
            if key.is_empty() {
                return Err(format!("Connection {}: header key is empty.", rule.id));
            }
            if !header_name_ok(key) {
                return Err(format!(
                    "Connection {}: header “{key}” is not allowed (use simple names like X-Api-Key).",
                    rule.id
                ));
            }
            if h.value.len() > 512 {
                return Err(format!(
                    "Connection {}: header value too long (max 512).",
                    rule.id
                ));
            }
        }
        for f in &rule.fields {
            if f.key.trim().is_empty() {
                return Err(format!("Connection {}: field key is empty.", rule.id));
            }
            if f.key.len() > 64 {
                return Err(format!(
                    "Connection {}: field key too long (max 64).",
                    rule.id
                ));
            }
            if f.value.len() > 512 {
                return Err(format!(
                    "Connection {}: field value too long (max 512).",
                    rule.id
                ));
            }
        }
    }
    Ok(())
}

fn header_name_ok(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    if lower.is_empty() || lower.len() > 64 {
        return false;
    }
    match lower.as_str() {
        "host" | "content-length" | "transfer-encoding" | "connection" | "content-type"
        | "x-metis-signature" | "x-metis-event" => return false,
        _ => {}
    }
    name.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Deserialize one connection from JSON without readiness checks (wizard tab switches).
pub fn parse_rule_json_draft(raw: &str) -> Result<WebhookRule, String> {
    serde_json::from_str(raw.trim()).map_err(|e| format!("Invalid connection JSON: {e}"))
}

/// Parse and fully validate one connection (Save).
pub fn parse_rule_json(raw: &str) -> Result<WebhookRule, String> {
    let rule = parse_rule_json_draft(raw)?;
    validate_rules(std::slice::from_ref(&rule))?;
    Ok(rule)
}

pub fn normalize_payload_mode(raw: &str) -> &'static str {
    match raw.trim().to_ascii_lowercase().as_str() {
        "text" => "text",
        "command" => "command",
        "custom" => "custom",
        _ => "full",
    }
}

/// Persist connections to settings, bump the live worker, and return pretty JSON for the UI.
///
/// Saving any enabled connection with a POST URL turns on the page-level master switch
/// (`webhook_enabled`) so operators are not stuck with a ready connection that never fires.
pub fn persist_webhook_rules(db: &Db, rules: &[WebhookRule]) -> Result<String, String> {
    validate_rules(rules)?;
    let stored = serde_json::to_string_pretty(rules).map_err(|e| e.to_string())?;
    db.set_setting("webhook_rules", &stored)
        .map_err(|e| e.to_string())?;
    let _ = db.audit(
        "webhook_rules",
        "",
        &format!("{} connection(s)", rules.len()),
    );
    let ready = rules
        .iter()
        .any(|r| r.enabled && connection_url_ok(&r.url));
    if ready && !setting_bool(db, "webhook_enabled", false) {
        crate::services::set_setting_bool(db, "webhook_enabled", true);
        let _ = db.audit("webhook_enable", "", "auto — connection ready");
    }
    bump_webhook_config();
    Ok(stored)
}

/// Copy legacy page-level `webhook_url` into connections missing a URL, then clear the setting.
/// Returns updated rules JSON when migration wrote anything.
pub fn migrate_legacy_webhook_url(db: &Db) -> Result<Option<String>, String> {
    let legacy = setting(db, "webhook_url");
    if !connection_url_ok(&legacy) {
        return Ok(None);
    }
    let rules_raw = setting(db, "webhook_rules");
    let mut rules = if rules_raw.trim().is_empty() {
        vec![WebhookRule::default_hash_commands()]
    } else {
        serde_json::from_str::<Vec<WebhookRule>>(&rules_raw)
            .map_err(|e| format!("Invalid webhook connections JSON: {e}"))?
    };
    let mut changed = false;
    for rule in &mut rules {
        if rule.url.trim().is_empty() {
            rule.url = legacy.clone();
            changed = true;
        }
    }
    if !changed {
        let _ = db.set_setting("webhook_url", "");
        return Ok(None);
    }
    validate_rules(&rules)?;
    let stored = serde_json::to_string_pretty(&rules).map_err(|e| e.to_string())?;
    db.set_setting("webhook_rules", &stored)
        .map_err(|e| e.to_string())?;
    let _ = db.set_setting("webhook_url", "");
    let _ = db.audit("webhook_url_migrate", "", &format!("{} connection(s)", rules.len()));
    bump_webhook_config();
    Ok(Some(stored))
}

pub fn load_webhook_config(db: &Db, data_dir: &Path) -> WebhookConfig {
    let rate = setting(db, "webhook_rate_per_min")
        .parse::<u32>()
        .unwrap_or(DEFAULT_RATE_PER_MIN);
    let timeout = setting(db, "webhook_timeout_secs")
        .parse::<u32>()
        .unwrap_or(DEFAULT_TIMEOUT_SECS);
    let rules_raw = setting(db, "webhook_rules");
    let rules = parse_rules_json(&rules_raw).unwrap_or_else(|_| {
        vec![WebhookRule::default_hash_commands()]
    });
    WebhookConfig {
        enabled: setting_bool(db, "webhook_enabled", false),
        rate_per_min: clamp_webhook_rate(rate),
        timeout_secs: clamp_webhook_timeout(timeout),
        rules,
        secret: load_webhook_secret(data_dir),
    }
}

fn ready_connection_count(cfg: &WebhookConfig) -> usize {
    cfg.rules
        .iter()
        .filter(|r| r.enabled && connection_url_ok(&r.url))
        .count()
}

pub fn webhook_status_line(db: &Db) -> String {
    let cfg = load_webhook_config(db, &crate::data_dir());
    let ready = ready_connection_count(&cfg);
    if !cfg.enabled {
        if ready > 0 {
            return format!(
                "Webhooks off · {ready} connection(s) ready — turn on Enable webhooks"
            );
        }
        return "Webhooks off".into();
    }
    if ready == 0 {
        return "Webhooks on · add a connection with a POST URL".into();
    }
    let guard = status().lock().unwrap_or_else(|e| e.into_inner());
    if !guard.last_error.is_empty() {
        return format!(
            "Webhooks · last error · delivered {} · dropped {}",
            guard.delivered, guard.dropped
        );
    }
    if let Some(ok) = guard.last_ok {
        let ago = ok.elapsed().as_secs();
        return format!(
            "Webhooks · last OK {ago}s ago (HTTP {}) · delivered {} · dropped {} · {}s timeout",
            guard.last_http, guard.delivered, guard.dropped, cfg.timeout_secs
        );
    }
    let signed = if cfg.secret.is_empty() {
        "unsigned"
    } else {
        "HMAC on"
    };
    format!(
        "Webhooks on · {ready} connection(s) · ≤{}/min · {}s timeout · {signed} · waiting for a match",
        cfg.rate_per_min, cfg.timeout_secs
    )
}

/// Short Overview chip line (refreshed on the dashboard live tick).
pub fn webhook_dashboard_hint(db: &Db) -> String {
    let cfg = load_webhook_config(db, &crate::data_dir());
    let ready = ready_connection_count(&cfg);
    if !cfg.enabled {
        if ready > 0 {
            return format!("Off · {ready} ready");
        }
        return "Off".into();
    }
    if ready == 0 {
        return "On · add URL".into();
    }
    let guard = status().lock().unwrap_or_else(|e| e.into_inner());
    if !guard.last_error.is_empty() {
        let brief: String = guard.last_error.chars().take(28).collect();
        return if brief.len() < guard.last_error.len() {
            format!("{brief}…")
        } else {
            brief
        };
    }
    if let Some(ok) = guard.last_ok {
        return format!("{ready} conn · last OK {}s", ok.elapsed().as_secs());
    }
    format!("{ready} conn · waiting")
}

/// True when the chip should use amber (last error while master is on).
pub fn webhook_dashboard_has_error(db: &Db) -> bool {
    let cfg = load_webhook_config(db, &crate::data_dir());
    if !cfg.enabled {
        return false;
    }
    !status()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .last_error
        .is_empty()
}

pub fn webhook_last_error() -> String {
    status()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .last_error
        .clone()
}

/// Newest-first copy of the in-memory delivery ring.
pub fn webhook_recent_deliveries() -> Vec<DeliveryRecord> {
    let guard = status().lock().unwrap_or_else(|e| e.into_inner());
    guard.recent.iter().cloned().collect()
}

/// Clear the in-memory recent-deliveries ring (does not touch audit).
pub fn clear_webhook_deliveries() {
    let mut g = status().lock().unwrap_or_else(|e| e.into_inner());
    g.recent.clear();
}

fn kinds_match(rule: &WebhookRule, kind: &str) -> bool {
    if rule.kinds.is_empty() {
        return true;
    }
    rule.kinds
        .iter()
        .any(|k| k.eq_ignore_ascii_case(kind) || k == "*")
}

fn channels_match(rule: &WebhookRule, channel: Option<&str>) -> bool {
    if rule.channels.is_empty()
        || rule
            .channels
            .iter()
            .any(|c| c.trim() == "*" || c.eq_ignore_ascii_case("any"))
    {
        return true;
    }
    let Some(name) = channel.map(str::trim).filter(|s| !s.is_empty()) else {
        // Direct messages: channel filters do not apply when kinds include direct.
        return true;
    };
    rule.channels
        .iter()
        .any(|c| c.trim().eq_ignore_ascii_case(name))
}

fn enrollment_match(rule: &WebhookRule, enrollment: Enrollment) -> bool {
    let wanted = if rule.enrollment.is_empty() {
        vec!["enrolled".to_string()]
    } else {
        rule.enrollment.clone()
    };
    if wanted
        .iter()
        .any(|e| e.trim() == "*" || e.eq_ignore_ascii_case("any"))
    {
        return true;
    }
    let label = enrollment.as_str();
    wanted.iter().any(|e| {
        let e = e.trim();
        e.eq_ignore_ascii_case(label)
            || (e.eq_ignore_ascii_case("enrolled")
                && matches!(enrollment, Enrollment::Enrolled | Enrollment::Hq))
    })
}

fn prefix_match(rule: &WebhookRule, text: &str) -> bool {
    let prefix = rule.text_prefix.trim();
    if prefix.is_empty() {
        return true;
    }
    text.trim_start().starts_with(prefix)
}

/// True when `allowed` (hex pubkey or prefix) matches the sender identity.
pub fn sender_id_matches(allowed: &str, from_pubkey: Option<&str>, from_prefix: &str) -> bool {
    let a = normalize_sender_id(allowed);
    if a.is_empty() {
        return false;
    }
    if let Some(pk) = from_pubkey.map(normalize_sender_id).filter(|s| !s.is_empty()) {
        if pk == a || pk.starts_with(&a) {
            return true;
        }
    }
    let pref = normalize_sender_id(from_prefix);
    if !pref.is_empty() && (pref == a || a.starts_with(&pref) || pref.starts_with(&a)) {
        return true;
    }
    false
}

fn senders_match(rule: &WebhookRule, from_pubkey: Option<&str>, from_prefix: &str) -> bool {
    if rule.allowed_senders.is_empty() {
        return true;
    }
    let pk = from_pubkey.map(str::trim).filter(|s| !s.is_empty());
    let pref = from_prefix.trim();
    if pk.is_none() && pref.is_empty() {
        return false;
    }
    rule.allowed_senders
        .iter()
        .any(|allowed| sender_id_matches(allowed, pk, pref))
}

/// Channel frames are usually `Name: body` — match prefixes on the body.
fn event_match_text(event: &MeshEvent) -> &str {
    let body = event.body.trim();
    if !body.is_empty() {
        body
    } else {
        event.text.trim()
    }
}

/// Pure match for tests and the enqueue path.
pub fn matching_rules<'a>(
    rules: &'a [WebhookRule],
    kind: &str,
    channel: Option<&str>,
    enrollment: Enrollment,
    text: &str,
    from_pubkey: Option<&str>,
    from_prefix: &str,
) -> Vec<&'a WebhookRule> {
    rules
        .iter()
        .filter(|r| r.enabled)
        .filter(|r| kinds_match(r, kind))
        .filter(|r| {
            if kind == "channel" {
                channels_match(r, channel)
            } else {
                // Direct: ignore channel name list unless empty/* only.
                true
            }
        })
        .filter(|r| enrollment_match(r, enrollment))
        .filter(|r| senders_match(r, from_pubkey, from_prefix))
        .filter(|r| prefix_match(r, text))
        .collect()
}

fn event_json(job: &DeliveryJob) -> Value {
    let e = &job.event;
    let mut id_bytes = [0u8; 16];
    RngCore::fill_bytes(&mut rand::thread_rng(), &mut id_bytes);
    let delivery_id = hex::encode(id_bytes);
    json!({
        "type": "message.received",
        "api_version": 1,
        "delivery_id": delivery_id,
        "message_id": e.message_id,
        "kind": e.kind,
        "channel": e.channel,
        "channel_idx": e.channel_idx,
        "from": {
            "pubkey": e.from_pubkey,
            "name": e.from_name,
            "prefix": e.from_prefix,
            "enrollment": e.enrollment.as_str(),
        },
        "text": e.text,
        "body": e.body,
        "hop_count": e.hop_count,
        "timestamp": e.timestamp,
        "ts": e.timestamp,
        "rule": {
            "id": job.rule_id,
            "name": job.rule_name,
        },
    })
}

/// Text after the rule prefix (e.g. `#gate open` + prefix `#` → `gate open`).
pub fn command_after_prefix(prefix: &str, text: &str) -> String {
    let t = text.trim_start();
    let p = prefix.trim();
    if !p.is_empty() && t.starts_with(p) {
        t[p.len()..].trim_start().to_string()
    } else {
        t.to_string()
    }
}

fn json_escape_unquoted(s: &str) -> String {
    let quoted = serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into());
    quoted
        .get(1..quoted.len().saturating_sub(1))
        .unwrap_or("")
        .to_string()
}

fn placeholder_pairs(job: &DeliveryJob) -> Vec<(&'static str, String)> {
    let e = &job.event;
    let command = command_after_prefix(&job.text_prefix, event_match_text(e));
    let channel = e.channel.clone().unwrap_or_default();
    let from_name = e.from_name.clone().unwrap_or_default();
    let from_pubkey = e.from_pubkey.clone().unwrap_or_default();
    let hop = e
        .hop_count
        .map(|h| h.to_string())
        .unwrap_or_default();
    vec![
        ("{{command}}", command),
        ("{{text}}", e.text.clone()),
        ("{{body}}", e.body.clone()),
        ("{{kind}}", e.kind.to_string()),
        ("{{channel}}", channel),
        ("{{from.name}}", from_name),
        ("{{from.pubkey}}", from_pubkey),
        ("{{from.prefix}}", e.from_prefix.clone()),
        ("{{from.enrollment}}", e.enrollment.as_str().to_string()),
        ("{{hop_count}}", hop),
        ("{{timestamp}}", e.timestamp.to_string()),
        ("{{ts}}", e.timestamp.to_string()),
        // Alias for connections that still use the old token name.
        ("{{received_at}}", e.timestamp.to_string()),
        ("{{message_id}}", e.message_id.to_string()),
        ("{{rule.id}}", job.rule_id.clone()),
        ("{{rule.name}}", job.rule_name.clone()),
    ]
}

fn apply_raw_placeholders(template: &str, job: &DeliveryJob) -> String {
    let mut out = template.to_string();
    for (token, value) in placeholder_pairs(job) {
        out = out.replace(token, &value);
    }
    out
}

fn apply_payload_template(template: &str, job: &DeliveryJob) -> String {
    let mut out = template.to_string();
    for (token, value) in placeholder_pairs(job) {
        out = out.replace(token, &json_escape_unquoted(&value));
    }
    out
}

fn merge_custom_fields(mut body: Value, job: &DeliveryJob) -> Result<Value, String> {
    if job.fields.is_empty() {
        return Ok(body);
    }
    let obj = body
        .as_object_mut()
        .ok_or_else(|| "payload root must be a JSON object to merge custom fields".to_string())?;
    for field in &job.fields {
        let key = field.key.trim();
        if key.is_empty() {
            continue;
        }
        let rendered = apply_raw_placeholders(&field.value, job);
        let val = serde_json::from_str(&rendered).unwrap_or(Value::String(rendered));
        obj.insert(key.to_string(), val);
    }
    Ok(body)
}

fn render_payload(job: &DeliveryJob) -> Result<String, String> {
    let base = match normalize_payload_mode(&job.payload_mode) {
        "text" => serde_json::to_value(json!({ "text": job.event.text }))
            .map_err(|e| e.to_string())?,
        "command" => {
            let match_text = event_match_text(&job.event);
            let command = command_after_prefix(&job.text_prefix, match_text);
            serde_json::to_value(json!({
                "command": command,
                "text": match_text,
                "prefix": job.text_prefix,
                "kind": job.event.kind,
                "channel": job.event.channel,
                "from": {
                    "name": job.event.from_name,
                    "prefix": job.event.from_prefix,
                    "enrollment": job.event.enrollment.as_str(),
                },
                "rule": { "id": job.rule_id, "name": job.rule_name },
            }))
            .map_err(|e| e.to_string())?
        }
        "custom" => {
            let trimmed = job.payload_template.trim();
            if trimmed.is_empty() {
                json!({})
            } else {
                let rendered = apply_payload_template(&job.payload_template, job);
                serde_json::from_str::<Value>(&rendered)
                    .map_err(|e| format!("custom payload is not valid JSON: {e}"))?
            }
        }
        _ => event_json(job),
    };
    let merged = merge_custom_fields(base, job)?;
    serde_json::to_string(&merged).map_err(|e| e.to_string())
}

/// HMAC-SHA256 (RFC 2104) over the raw body bytes.
pub fn hmac_sha256_hex(key: &[u8], message: &[u8]) -> String {
    const BLOCK: usize = 64;
    let mut key_block = [0u8; BLOCK];
    if key.len() > BLOCK {
        let dig = Sha256::digest(key);
        key_block[..32].copy_from_slice(&dig);
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] ^= key_block[i];
        opad[i] ^= key_block[i];
    }
    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(message);
    let inner_hash = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner_hash);
    hex::encode(outer.finalize())
}

fn rate_allows(limit: u32) -> bool {
    let now = Instant::now();
    let mut window = rate_window().lock().unwrap_or_else(|e| e.into_inner());
    while window
        .front()
        .is_some_and(|t| now.duration_since(*t) >= Duration::from_secs(60))
    {
        window.pop_front();
    }
    if window.len() as u32 >= limit {
        return false;
    }
    window.push_back(now);
    true
}

fn set_error(msg: impl Into<String>) {
    let mut g = status().lock().unwrap_or_else(|e| e.into_inner());
    g.last_error = msg.into();
}

fn set_ok(http: u16) {
    let mut g = status().lock().unwrap_or_else(|e| e.into_inner());
    g.last_ok = Some(Instant::now());
    g.last_http = http;
    g.last_error.clear();
    g.delivered = g.delivered.saturating_add(1);
}

fn bump_dropped() {
    let mut g = status().lock().unwrap_or_else(|e| e.into_inner());
    g.dropped = g.dropped.saturating_add(1);
}

fn push_delivery_record(record: DeliveryRecord) {
    let mut g = status().lock().unwrap_or_else(|e| e.into_inner());
    g.recent.push_front(record);
    while g.recent.len() > RECENT_CAP {
        g.recent.pop_back();
    }
}

fn job_to_delivery_job(rule: &WebhookRule, event: MeshEvent, is_test: bool) -> DeliveryJob {
    DeliveryJob {
        rule_id: rule.id.clone(),
        rule_name: if rule.name.trim().is_empty() {
            rule.id.clone()
        } else {
            rule.name.clone()
        },
        url: rule.url.trim().to_string(),
        text_prefix: rule.text_prefix.clone(),
        payload_mode: rule.payload_mode.clone(),
        payload_template: rule.payload_template.clone(),
        headers: rule.headers.clone(),
        fields: rule.fields.clone(),
        event,
        is_test,
    }
}

/// Synthetic mesh text for a Test POST (`#snore` → `#snore test`).
pub fn test_event_text(rule: &WebhookRule) -> String {
    let prefix = rule.text_prefix.trim();
    if prefix.is_empty() {
        "!test".into()
    } else if prefix.ends_with(char::is_whitespace) {
        format!("{prefix}test")
    } else {
        format!("{prefix} test")
    }
}

fn synthetic_test_event(db: &Db, rule: &WebhookRule) -> MeshEvent {
    let kind = if rule.kinds.iter().any(|k| k.eq_ignore_ascii_case("channel"))
        || rule.kinds.is_empty()
    {
        "channel"
    } else {
        "direct"
    };
    let channel = if kind == "channel" {
        rule.channels
            .iter()
            .map(|c| c.trim())
            .find(|c| !c.is_empty() && *c != "*" && !c.eq_ignore_ascii_case("any"))
            .map(|c| c.to_string())
            .or_else(|| Some("TEST".into()))
    } else {
        None
    };
    let text = test_event_text(rule);
    let advert = db.hq_advert_name().unwrap_or_else(|_| "hq".into());
    let hq = db.hq_node().ok().flatten();
    let from_pubkey = hq.as_ref().map(|n| n.pubkey.clone());
    let from_prefix = hq
        .as_ref()
        .map(|n| n.prefix.clone())
        .filter(|p| !p.trim().is_empty())
        .unwrap_or_else(|| advert.chars().take(12).collect());
    let enrollment = hq
        .as_ref()
        .map(|n| n.enrollment)
        .unwrap_or(Enrollment::Hq);
    MeshEvent {
        message_id: 0,
        kind,
        channel,
        channel_idx: None,
        from_pubkey,
        from_name: Some(advert),
        from_prefix,
        enrollment,
        text: text.clone(),
        body: text,
        hop_count: Some(0),
        timestamp: crate::db::unix_now(),
    }
}

/// Queue one Test POST for a connection (bypasses master enable, connection enabled, and rate).
pub fn enqueue_test_delivery(db: &Db, rule: &WebhookRule) -> Result<(), String> {
    if !connection_url_ok(&rule.url) {
        return Err("Connection needs an http(s) POST URL.".into());
    }
    let event = synthetic_test_event(db, rule);
    let job = job_to_delivery_job(rule, event, true);
    if let Err(err) = render_payload(&job) {
        return Err(format!("Payload would fail: {err}"));
    }
    let tx_guard = delivery_tx().lock().unwrap_or_else(|e| e.into_inner());
    let Some(tx) = tx_guard.as_ref() else {
        return Err("Webhook worker is not running yet — wait a moment after HQ start.".into());
    };
    tx.send(job)
        .map_err(|_| "Webhook queue closed.".to_string())?;
    hq_log!(info, "webhook test queued for {}", rule.id);
    Ok(())
}

/// After a live RX insert (or local channel TX): enqueue matching connection POSTs.
pub fn try_enqueue_mesh_event(db: &Db, event: MeshEvent) {
    if !setting_bool(db, "webhook_enabled", false) {
        hq_log!(
            debug,
            "webhook skip: master off ({} {} {:?})",
            event.kind,
            event_match_text(&event),
            event.channel
        );
        return;
    }
    let rules_raw = setting(db, "webhook_rules");
    let rules = match parse_rules_json(&rules_raw) {
        Ok(r) => r,
        Err(err) => {
            hq_log!(warn, "webhook connections: {err}");
            return;
        }
    };
    let matches = matching_rules(
        &rules,
        event.kind,
        event.channel.as_deref(),
        event.enrollment,
        event_match_text(&event),
        event.from_pubkey.as_deref(),
        &event.from_prefix,
    );
    if matches.is_empty() {
        hq_log!(
            debug,
            "webhook skip: no match ({} {} {:?})",
            event.kind,
            event_match_text(&event),
            event.channel
        );
        return;
    }
    let tx_guard = delivery_tx().lock().unwrap_or_else(|e| e.into_inner());
    let Some(tx) = tx_guard.as_ref() else {
        hq_log!(warn, "webhook skip: worker not started");
        return;
    };
    let mut queued = 0u32;
    for rule in matches {
        if !connection_url_ok(&rule.url) {
            hq_log!(
                debug,
                "webhook skip: connection {} has no URL",
                rule.id
            );
            continue;
        }
        let job = job_to_delivery_job(rule, event.clone(), false);
        if let Err(err) = tx.send(job) {
            hq_log!(debug, "webhook queue: {err}");
            bump_dropped();
        } else {
            queued += 1;
        }
    }
    if queued > 0 {
        hq_log!(
            info,
            "webhook queued {queued} for {} {} {:?}",
            event.kind,
            event_match_text(&event),
            event.channel
        );
    }
}

/// Spawn the webhook worker on the services runtime (once).
pub fn spawn_webhook_worker(db: std::sync::Arc<Db>) {
    let (tx, rx) = unbounded_channel();
    {
        let mut guard = delivery_tx().lock().unwrap_or_else(|e| e.into_inner());
        *guard = Some(tx);
    }
    tokio::spawn(async move {
        worker_loop(db, rx).await;
    });
}

async fn worker_loop(db: std::sync::Arc<Db>, mut rx: UnboundedReceiver<DeliveryJob>) {
    let mut pending: VecDeque<DeliveryJob> = VecDeque::new();
    loop {
        tokio::select! {
            job = rx.recv() => {
                match job {
                    None => break,
                    Some(job) => {
                        if pending.len() < QUEUE_CAP {
                            pending.push_back(job);
                        } else {
                            hq_log!(debug, "webhook queue full; dropping");
                            bump_dropped();
                        }
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(50)), if !pending.is_empty() => {
                if let Some(job) = pending.pop_front() {
                    // Blocking ureq on services thread is intentional (same as map upload).
                    let db = db.clone();
                    tokio::task::spawn_blocking(move || deliver_one(&db, job)).await.ok();
                }
            }
        }
    }
}

fn deliver_one(db: &Db, job: DeliveryJob) {
    let kind_label: &'static str = if job.is_test { "test" } else { "live" };
    let record_base = |http: u16, ok: bool, detail: String| DeliveryRecord {
        at_unix: crate::db::unix_now(),
        rule_id: job.rule_id.clone(),
        rule_name: job.rule_name.clone(),
        url_label: connection_url_label(&job.url),
        http,
        ok,
        detail,
        kind: kind_label,
    };

    if !job.is_test && !setting_bool(db, "webhook_enabled", false) {
        return;
    }
    if !connection_url_ok(&job.url) {
        bump_dropped();
        let detail = format!("connection {} has no URL", job.rule_id);
        set_error(detail.clone());
        push_delivery_record(record_base(0, false, detail));
        return;
    }
    if !job.is_test {
        let rate = setting(db, "webhook_rate_per_min")
            .parse::<u32>()
            .unwrap_or(DEFAULT_RATE_PER_MIN);
        let limit = clamp_webhook_rate(rate);
        if !rate_allows(limit) {
            hq_log!(debug, "webhook rate limited");
            bump_dropped();
            set_error("rate limited");
            push_delivery_record(record_base(0, false, "rate limited".into()));
            return;
        }
    }
    let payload = match render_payload(&job) {
        Ok(s) => s,
        Err(err) => {
            set_error(err.clone());
            bump_dropped();
            hq_log!(warn, "webhook payload: {err}");
            push_delivery_record(record_base(0, false, err));
            return;
        }
    };
    let timeout_secs = clamp_webhook_timeout(
        setting(db, "webhook_timeout_secs")
            .parse::<u32>()
            .unwrap_or(DEFAULT_TIMEOUT_SECS),
    );
    let secret = load_webhook_secret(&crate::data_dir());
    let mut req = ureq::post(job.url.trim())
        .set("User-Agent", HTTP_USER_AGENT)
        .set("Content-Type", "application/json")
        .set("Accept", "application/json")
        .set("X-Metis-Event", "message.received")
        .timeout(Duration::from_secs(u64::from(timeout_secs)));
    if !secret.is_empty() {
        let sig = hmac_sha256_hex(secret.as_bytes(), payload.as_bytes());
        req = req.set("X-Metis-Signature", &format!("sha256={sig}"));
    }
    for header in &job.headers {
        let key = header.key.trim();
        if !header_name_ok(key) {
            continue;
        }
        let value = apply_raw_placeholders(&header.value, &job);
        // Never log header values (may be secret).
        req = req.set(key, &value);
    }
    match req.send_string(&payload) {
        Ok(resp) => {
            let status_code = resp.status();
            if (200..300).contains(&status_code) {
                set_ok(status_code);
                let detail = if job.is_test {
                    "test".into()
                } else {
                    format!("msg={}", job.event.message_id)
                };
                push_delivery_record(record_base(status_code, true, detail.clone()));
                let _ = db.audit(
                    if job.is_test {
                        "webhook_test"
                    } else {
                        "webhook_deliver"
                    },
                    &job.rule_id,
                    &format!("http={status_code} {detail}"),
                );
            } else {
                let brief: String = resp
                    .into_string()
                    .unwrap_or_default()
                    .chars()
                    .take(80)
                    .collect();
                let err = format!("HTTP {status_code}: {brief}");
                set_error(err.clone());
                bump_dropped();
                push_delivery_record(record_base(status_code, false, err.clone()));
                hq_log!(warn, "webhook POST: {err}");
            }
        }
        Err(err) => {
            let err = err.to_string();
            set_error(err.clone());
            bump_dropped();
            push_delivery_record(record_base(0, false, err.clone()));
            hq_log!(warn, "webhook POST: {err}");
        }
    }
}

/// Resolve enrollment for a pubkey (heard if unknown).
pub fn enrollment_for_pubkey(db: &Db, pubkey: Option<&str>) -> Enrollment {
    let Some(pk) = pubkey.map(str::trim).filter(|s| !s.is_empty()) else {
        return Enrollment::Heard;
    };
    db.get_node(pk)
        .ok()
        .flatten()
        .map(|n| n.enrollment)
        .unwrap_or(Enrollment::Heard)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_rule_matches_enrolled_hash() {
        let mut rule = WebhookRule::default_hash_commands();
        rule.enabled = true;
        rule.url = "http://127.0.0.1:9/hook".into();
        let rules = vec![rule];
        let hits = matching_rules(
            &rules,
            "channel",
            Some("METIS-INFO"),
            Enrollment::Enrolled,
            "!gate open",
            None,
            "",
        );
        assert_eq!(hits.len(), 1);
        assert!(matching_rules(
            &rules,
            "channel",
            Some("METIS-INFO"),
            Enrollment::Heard,
            "!gate open",
            None,
            "",
        )
        .is_empty());
        assert!(matching_rules(
            &rules,
            "channel",
            Some("METIS-INFO"),
            Enrollment::Enrolled,
            "hello",
            None,
            "",
        )
        .is_empty());
    }

    #[test]
    fn channel_prefix_matches_body_not_wire() {
        let mut rule = WebhookRule::default_hash_commands();
        rule.enabled = true;
        rule.url = "http://127.0.0.1:9/hook".into();
        rule.text_prefix = "#snore".into();
        rule.enrollment = vec!["any".into()];
        rule.channels = vec!["METIS-NET".into()];
        let event = MeshEvent {
            message_id: 1,
            kind: "channel",
            channel: Some("METIS-NET".into()),
            channel_idx: Some(5),
            from_pubkey: None,
            from_name: Some("Rose01-Hesperia".into()),
            from_prefix: "Rose01".into(),
            enrollment: Enrollment::Heard,
            text: "Rose01-Hesperia: #snore".into(),
            body: "#snore".into(),
            hop_count: Some(0),
            timestamp: 1,
        };
        let rules = [rule];
        let hits = matching_rules(
            &rules,
            event.kind,
            event.channel.as_deref(),
            event.enrollment,
            event_match_text(&event),
            event.from_pubkey.as_deref(),
            &event.from_prefix,
        );
        assert_eq!(hits.len(), 1);
        let wire_only = [WebhookRule {
            text_prefix: "#snore".into(),
            enrollment: vec!["any".into()],
            channels: vec!["METIS-NET".into()],
            enabled: true,
            url: "http://127.0.0.1:9/hook".into(),
            ..WebhookRule::default_hash_commands()
        }];
        assert!(matching_rules(
            &wire_only,
            "channel",
            Some("METIS-NET"),
            Enrollment::Heard,
            "Rose01-Hesperia: #snore",
            None,
            "",
        )
        .is_empty());
    }

    #[test]
    fn channel_filter_and_any_enrollment() {
        let rule = WebhookRule {
            id: "public-only".into(),
            name: "Public".into(),
            enabled: true,
            kinds: vec!["channel".into()],
            channels: vec!["Public".into()],
            enrollment: vec!["any".into()],
            allowed_senders: Vec::new(),
            text_prefix: String::new(),
            payload_mode: "full".into(),
            payload_template: String::new(),
            url: "http://127.0.0.1:9/hook".into(),
            headers: Vec::new(),
            fields: Vec::new(),
        };
        let rules = vec![rule];
        assert_eq!(
            matching_rules(
                &rules,
                "channel",
                Some("Public"),
                Enrollment::Heard,
                "hi",
                None,
                "",
            )
            .len(),
            1
        );
        assert!(matching_rules(
            &rules,
            "channel",
            Some("METIS-INFO"),
            Enrollment::Enrolled,
            "hi",
            None,
            "",
        )
        .is_empty());
        assert!(matching_rules(
            &rules,
            "direct",
            None,
            Enrollment::Enrolled,
            "hi",
            None,
            "",
        )
        .is_empty());
    }

    #[test]
    fn allowed_senders_filter_by_pubkey_prefix() {
        let mut rule = WebhookRule::default_hash_commands();
        rule.enabled = true;
        rule.url = "http://127.0.0.1:9/hook".into();
        rule.enrollment = vec!["any".into()];
        rule.allowed_senders = vec!["aabbcc".into()];
        let rules = [rule];
        assert_eq!(
            matching_rules(
                &rules,
                "channel",
                Some("METIS-INFO"),
                Enrollment::Heard,
                "!x",
                Some("aabbcc001122"),
                "aabbcc",
            )
            .len(),
            1
        );
        assert!(matching_rules(
            &rules,
            "channel",
            Some("METIS-INFO"),
            Enrollment::Heard,
            "!x",
            Some("deadbeef"),
            "deadbe",
        )
        .is_empty());
        assert!(matching_rules(
            &rules,
            "channel",
            Some("METIS-INFO"),
            Enrollment::Heard,
            "!x",
            None,
            "",
        )
        .is_empty());
    }

    #[test]
    fn command_payload_strips_prefix() {
        assert_eq!(command_after_prefix("!", "!gate open"), "gate open");
        assert_eq!(command_after_prefix("!", "  !x"), "x");
        let job = DeliveryJob {
            rule_id: "hash-cmds".into(),
            rule_name: "Hash".into(),
            url: "http://127.0.0.1:9/hook".into(),
            text_prefix: "!".into(),
            payload_mode: "command".into(),
            payload_template: String::new(),
            headers: Vec::new(),
            fields: vec![WebhookField {
                key: "source".into(),
                value: "hq".into(),
            }],
            event: MeshEvent {
                message_id: 1,
                kind: "direct",
                channel: None,
                channel_idx: None,
                from_pubkey: None,
                from_name: Some("Ada".into()),
                from_prefix: "aabbcc".into(),
                enrollment: Enrollment::Enrolled,
                text: "!gate open".into(),
                body: "!gate open".into(),
                hop_count: Some(1),
                timestamp: 1,
            },
            is_test: false,
        };
        let raw = render_payload(&job).unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["command"], "gate open");
        assert_eq!(v["text"], "!gate open");
        assert_eq!(v["source"], "hq");
    }

    #[test]
    fn custom_payload_template_json() {
        let job = DeliveryJob {
            rule_id: "r1".into(),
            rule_name: "R".into(),
            url: "http://127.0.0.1:9/hook".into(),
            text_prefix: "!".into(),
            payload_mode: "custom".into(),
            payload_template: r#"{"cmd":"{{command}}","who":"{{from.name}}"}"#.into(),
            headers: Vec::new(),
            fields: Vec::new(),
            event: MeshEvent {
                message_id: 2,
                kind: "channel",
                channel: Some("Public".into()),
                channel_idx: Some(0),
                from_pubkey: None,
                from_name: Some("Ada\"Bee".into()),
                from_prefix: "aa".into(),
                enrollment: Enrollment::Enrolled,
                text: "!ping".into(),
                body: "!ping".into(),
                hop_count: None,
                timestamp: 9,
            },
            is_test: false,
        };
        let raw = render_payload(&job).unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["cmd"], "ping");
        assert_eq!(v["who"], "Ada\"Bee");
    }

    #[test]
    fn migrate_legacy_url_into_empty_connections() {
        use crate::db::Db;
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "metis-wh-mig-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = Db::open(&dir.join("metis.db")).unwrap();
        db.ensure_defaults().unwrap();
        let mut rule = WebhookRule::default_hash_commands();
        rule.enabled = false; // empty URL ok while disabled
        rule.url.clear();
        db.set_setting("webhook_rules", &serde_json::to_string(&[rule]).unwrap())
            .unwrap();
        db.set_setting("webhook_url", "http://127.0.0.1:1880/hq-event")
            .unwrap();
        let stored = migrate_legacy_webhook_url(&db).unwrap().expect("migrated");
        let rules: Vec<WebhookRule> = serde_json::from_str(&stored).unwrap();
        assert_eq!(rules[0].url, "http://127.0.0.1:1880/hq-event");
        assert!(db.setting("webhook_url").unwrap().unwrap_or_default().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn enabled_connection_requires_url() {
        let mut rule = WebhookRule::default_hash_commands();
        rule.enabled = true;
        rule.url.clear();
        assert!(validate_rules(&[rule.clone()]).is_err());
        rule.url = "http://127.0.0.1:9/x".into();
        assert!(validate_rules(&[rule]).is_ok());
    }

    #[test]
    fn hmac_known_vector() {
        // RFC 4231 test case 1 (truncated key)
        let sig = hmac_sha256_hex(b"key", b"The quick brown fox jumps over the lazy dog");
        assert_eq!(
            sig,
            "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
        );
    }

    #[test]
    fn parse_rules_round_trip() {
        let raw = default_rules_json();
        let rules = parse_rules_json(&raw).unwrap();
        assert!(rules.is_empty());
    }

    #[test]
    fn clamp_timeout_bounds() {
        assert_eq!(clamp_webhook_timeout(1), 3);
        assert_eq!(clamp_webhook_timeout(12), 12);
        assert_eq!(clamp_webhook_timeout(99), 60);
    }

    #[test]
    fn test_event_text_appends_after_prefix() {
        let mut rule = WebhookRule::blank();
        rule.text_prefix = "#snore".into();
        assert_eq!(test_event_text(&rule), "#snore test");
        rule.text_prefix.clear();
        assert_eq!(test_event_text(&rule), "!test");
    }
}
