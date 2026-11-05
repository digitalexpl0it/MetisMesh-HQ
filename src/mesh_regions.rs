//! MeshCore region names, scope keys, and discover-response parsing.

use sha2::{Digest, Sha256};

use crate::db::Db;

const REGIONS_KEY: &str = "mesh_regions";
const ACTIVE_KEY: &str = "mesh_region_active";

/// Max UTF-8 byte length for a MeshCore region name (docs: 29).
pub const MAX_REGION_NAME_BYTES: usize = 29;

/// Normalize for storage/compare: trim, strip a single leading `#`, lowercase.
pub fn normalize_region_name(raw: &str) -> String {
    let s = raw.trim();
    let s = s.strip_prefix('#').unwrap_or(s).trim();
    s.to_ascii_lowercase()
}

/// Valid MeshCore region code: lowercase a-z0-9-, 1..=29 bytes (after normalize).
pub fn validate_region_name(raw: &str) -> Result<String, String> {
    let name = normalize_region_name(raw);
    if name.is_empty() {
        return Err("Region name required.".into());
    }
    if name.len() > MAX_REGION_NAME_BYTES {
        return Err(format!(
            "Region name too long (max {MAX_REGION_NAME_BYTES} bytes)."
        ));
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err("Region names use lowercase a-z, 0-9, and hyphen only.".into());
    }
    Ok(name)
}

/// First 16 bytes of SHA-256(`#` + name) — MeshCore transport scope key.
pub fn region_scope_key(name: &str) -> [u8; 16] {
    let name = normalize_region_name(name);
    let mut hasher = Sha256::new();
    hasher.update(b"#");
    hasher.update(name.as_bytes());
    let digest = hasher.finalize();
    let mut key = [0u8; 16];
    key.copy_from_slice(&digest[..16]);
    key
}

/// Normalize a hash-channel name for join: trim, strip one leading `#`.
pub fn normalize_hash_channel_name(raw: &str) -> Result<String, String> {
    let name = raw.trim().strip_prefix('#').unwrap_or(raw.trim()).trim();
    if name.is_empty() {
        return Err("Hash channel name is required.".into());
    }
    if name.len() > 16 {
        return Err("Hash channel name is limited to 16 characters.".into());
    }
    Ok(name.to_string())
}

/// MeshCore hash-channel secret: first 16 bytes of SHA-256(UTF-8 name).
pub fn hash_channel_secret(name: &str) -> [u8; 16] {
    let mut hasher = Sha256::new();
    hasher.update(name.as_bytes());
    let digest = hasher.finalize();
    let mut key = [0u8; 16];
    key.copy_from_slice(&digest[..16]);
    key
}

/// Build `CMD_SET_DEFAULT_FLOOD_SCOPE (63)` frame: clear, or name(31 NUL) + key(16).
pub fn default_flood_scope_frame(scope: Option<&str>) -> Result<Vec<u8>, String> {
    const CMD: u8 = 63;
    match scope {
        None => Ok(vec![CMD]),
        Some(raw) => {
            let name = validate_region_name(raw)?;
            let key = region_scope_key(&name);
            let mut frame = Vec::with_capacity(1 + 31 + 16);
            frame.push(CMD);
            let mut name_bytes = [0u8; 31];
            let n = name.as_bytes().len().min(31);
            name_bytes[..n].copy_from_slice(&name.as_bytes()[..n]);
            frame.extend_from_slice(&name_bytes);
            frame.extend_from_slice(&key);
            Ok(frame)
        }
    }
}

/// Clear session flood-scope override: `CMD_SET_FLOOD_SCOPE_KEY (54)` with sub=0 and no key.
pub fn clear_session_flood_scope_frame() -> Vec<u8> {
    vec![54, 0]
}

/// Parse `PUSH_CODE_BINARY_RESPONSE` payload after the companion tag matching:
/// `u32 LE clock` + NUL-terminated (or rest-of-buffer) comma-separated names.
pub fn parse_regions_binary_response(data: &[u8]) -> Vec<String> {
    if data.len() < 4 {
        return Vec::new();
    }
    let body = &data[4..];
    let end = body.iter().position(|&b| b == 0).unwrap_or(body.len());
    let text = String::from_utf8_lossy(&body[..end]);
    let mut out = Vec::new();
    for part in text.split(',') {
        let part = part.trim();
        if part.is_empty() || part == "*" {
            continue;
        }
        if let Ok(name) = validate_region_name(part) {
            if !out.iter().any(|n| n == &name) {
                out.push(name);
            }
        }
    }
    out
}

/// Merge new names into an existing list (dedupe, sorted case already normalized).
pub fn merge_region_lists(existing: &[String], discovered: &[String]) -> Vec<String> {
    let mut out = existing.to_vec();
    for name in discovered {
        let Ok(n) = validate_region_name(name) else {
            continue;
        };
        if !out.iter().any(|e| e == &n) {
            out.push(n);
        }
    }
    out.sort();
    out
}

pub fn load_known_regions(db: &Db) -> Vec<String> {
    let raw = db
        .setting(REGIONS_KEY)
        .ok()
        .flatten()
        .unwrap_or_else(|| "[]".into());
    let parsed: Vec<String> = serde_json::from_str(&raw).unwrap_or_default();
    let mut out = Vec::new();
    for name in parsed {
        if let Ok(n) = validate_region_name(&name) {
            if !out.iter().any(|e| e == &n) {
                out.push(n);
            }
        }
    }
    out.sort();
    out
}

pub fn save_known_regions(db: &Db, regions: &[String]) -> Result<(), String> {
    let cleaned = merge_region_lists(&[], regions);
    let raw = serde_json::to_string(&cleaned).map_err(|e| e.to_string())?;
    db.set_setting(REGIONS_KEY, &raw).map_err(|e| e.to_string())
}

pub fn merge_known_regions(db: &Db, discovered: &[String]) -> Result<Vec<String>, String> {
    let merged = merge_region_lists(&load_known_regions(db), discovered);
    save_known_regions(db, &merged)?;
    Ok(merged)
}

pub fn add_known_region(db: &Db, raw: &str) -> Result<String, String> {
    let name = validate_region_name(raw)?;
    let _ = merge_known_regions(db, &[name.clone()])?;
    Ok(name)
}

pub fn load_active_region(db: &Db) -> Option<String> {
    db.setting(ACTIVE_KEY)
        .ok()
        .flatten()
        .and_then(|s| validate_region_name(&s).ok())
}

pub fn save_active_region(db: &Db, scope: Option<&str>) -> Result<(), String> {
    match scope {
        None => db.set_setting(ACTIVE_KEY, "").map_err(|e| e.to_string()),
        Some(raw) => {
            let name = validate_region_name(raw)?;
            db.set_setting(ACTIVE_KEY, &name).map_err(|e| e.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_channel_secret_is_stable() {
        let a = hash_channel_secret("highdesert");
        let b = hash_channel_secret("highdesert");
        assert_eq!(a, b);
        assert_ne!(a, hash_channel_secret("other"));
        assert_eq!(
            normalize_hash_channel_name("#Ops").unwrap(),
            "Ops"
        );
    }

    #[test]
    fn validate_and_normalize() {
        assert_eq!(validate_region_name("  SoCal-West ").unwrap(), "socal-west");
        assert_eq!(validate_region_name("#be-vbr").unwrap(), "be-vbr");
        assert!(validate_region_name("").is_err());
        assert!(validate_region_name("Bad_Name").is_err());
        assert!(validate_region_name(&"a".repeat(30)).is_err());
    }

    #[test]
    fn scope_key_stable() {
        let a = region_scope_key("sample-city");
        let b = region_scope_key("#sample-city");
        let c = region_scope_key("SAMPLE-CITY");
        assert_eq!(a, b);
        assert_eq!(a, c);
        assert_ne!(a, region_scope_key("sample-west"));
    }

    #[test]
    fn parse_binary_csv() {
        let mut data = Vec::new();
        data.extend_from_slice(&1_700_000_000u32.to_le_bytes());
        data.extend_from_slice(b"sample-city,sample-west,*,bad_NAME,be-vbr\0extra");
        let names = parse_regions_binary_response(&data);
        assert_eq!(names, vec!["sample-city", "sample-west", "be-vbr"]);
    }

    #[test]
    fn default_scope_frame_sizes() {
        assert_eq!(default_flood_scope_frame(None).unwrap(), vec![63]);
        let frame = default_flood_scope_frame(Some("be-vbr")).unwrap();
        assert_eq!(frame.len(), 1 + 31 + 16);
        assert_eq!(frame[0], 63);
        assert_eq!(&frame[1..7], b"be-vbr");
        assert_eq!(&frame[32..], &region_scope_key("be-vbr"));
    }

    #[test]
    fn merge_dedupes() {
        let merged = merge_region_lists(
            &["be".into(), "be-vbr".into()],
            &["be-vbr".into(), "nl".into()],
        );
        assert_eq!(merged, vec!["be", "be-vbr", "nl"]);
    }
}
