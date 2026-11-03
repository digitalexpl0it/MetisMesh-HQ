//! Optional HQ brand mark for the top header (whitelabel logo).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::db::{unix_now, Db};

static BRAND_DIR: OnceLock<PathBuf> = OnceLock::new();

const MAX_BYTES: u64 = 2 * 1024 * 1024;
const SETTING_FILE: &str = "brand_logo";
const SETTING_REV: &str = "brand_logo_rev";

/// Bundled EmergHQ mark — shipped inside the binary (release packages have no assets/).
const DEFAULT_LOGO_PNG: &[u8] = include_bytes!("../assets/emerghq_logo.png");
const DEFAULT_LOGO_NAME: &str = "__default.png";

pub fn init(data_dir: &Path) {
    let dir = data_dir.join("brand");
    let _ = fs::create_dir_all(&dir);
    let _ = BRAND_DIR.set(dir);
}

fn brand_dir() -> PathBuf {
    BRAND_DIR
        .get()
        .cloned()
        .unwrap_or_else(|| crate::data_dir().join("brand"))
}

/// URL for the bundled default logo (works in cargo-built desktop releases).
pub fn default_logo_url() -> &'static str {
    "metis://localhost/brand/__default.png"
}

fn sanitize_filename(name: &str) -> Option<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.contains("..") || trimmed.contains('/') || trimmed.contains('\\')
    {
        return None;
    }
    let base = Path::new(trimmed)
        .file_name()?
        .to_str()?
        .to_ascii_lowercase();
    if base.is_empty() {
        return None;
    }
    let ext = Path::new(&base).extension()?.to_str()?;
    if !matches!(
        ext,
        "png" | "jpg" | "jpeg" | "webp" | "gif" | "svg"
    ) {
        return None;
    }
    let stem = Path::new(&base)
        .file_stem()?
        .to_str()?
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(48)
        .collect::<String>();
    if stem.is_empty() {
        return None;
    }
    Some(format!("{stem}.{ext}"))
}

fn content_type(name: &str) -> &'static str {
    match Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

/// Custom uploaded logo URL, if the operator replaced the bundled default.
pub fn custom_logo_url(db: &Db) -> Option<String> {
    let name = db.setting(SETTING_FILE).ok().flatten()?;
    if name.trim().is_empty() {
        return None;
    }
    let name = sanitize_filename(&name)?;
    let path = brand_dir().join(&name);
    if !path.is_file() {
        return None;
    }
    let rev = db
        .setting(SETTING_REV)
        .ok()
        .flatten()
        .unwrap_or_else(|| "0".into());
    Some(format!("metis://localhost/brand/{name}?v={rev}"))
}

/// True when a custom upload is active (Clear restores the bundled default).
pub fn has_custom_logo(db: &Db) -> bool {
    custom_logo_url(db).is_some()
}

/// Absolute path currently configured (for Settings preview fallbacks).
pub fn logo_disk_path(db: &Db) -> Option<PathBuf> {
    let name = db.setting(SETTING_FILE).ok().flatten()?;
    let name = sanitize_filename(&name)?;
    let path = brand_dir().join(name);
    path.is_file().then_some(path)
}

/// Read bytes for the custom protocol handler. Returns `(bytes, content_type)`.
pub fn read_brand_file(name: &str) -> Option<(Vec<u8>, &'static str)> {
    let raw = name.trim().split('?').next().unwrap_or("").trim();
    if raw.eq_ignore_ascii_case(DEFAULT_LOGO_NAME) {
        return Some((DEFAULT_LOGO_PNG.to_vec(), "image/png"));
    }
    let name = sanitize_filename(raw)?;
    let path = brand_dir().join(&name);
    let bytes = fs::read(path).ok()?;
    Some((bytes, content_type(&name)))
}

/// Open a file picker, copy into `data/brand/`, and store the setting.
pub fn pick_and_install_logo(db: &Db) -> Result<String, String> {
    let source = rfd::FileDialog::new()
        .set_title("Choose brand logo")
        .add_filter("Images", &["png", "jpg", "jpeg", "webp", "gif", "svg"])
        .pick_file()
        .ok_or_else(|| "Logo upload cancelled.".to_string())?;

    let meta = fs::metadata(&source).map_err(|err| err.to_string())?;
    if !meta.is_file() {
        return Err("Choose an image file.".into());
    }
    if meta.len() > MAX_BYTES {
        return Err("Logo must be 2 MB or smaller.".into());
    }

    let file_name = source
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| "Invalid file name.".to_string())?;
    let dest_name = sanitize_filename(file_name)
        .ok_or_else(|| "Use png, jpg, jpeg, webp, gif, or svg.".to_string())?;

    let dir = brand_dir();
    fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    // Drop previous logos so the folder does not accumulate.
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                let _ = fs::remove_file(path);
            }
        }
    }
    let dest = dir.join(&dest_name);
    fs::copy(&source, &dest).map_err(|err| err.to_string())?;

    db.set_setting(SETTING_FILE, &dest_name)
        .map_err(|err| err.to_string())?;
    db.set_setting(SETTING_REV, &unix_now().to_string())
        .map_err(|err| err.to_string())?;
    Ok(dest_name)
}

/// Remove a custom upload so the bundled EmergHQ mark shows again.
pub fn clear_logo(db: &Db) -> Result<(), String> {
    if let Some(path) = logo_disk_path(db) {
        let _ = fs::remove_file(path);
    }
    let dir = brand_dir();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                let _ = fs::remove_file(path);
            }
        }
    }
    db.set_setting(SETTING_FILE, "")
        .map_err(|err| err.to_string())?;
    db.set_setting(SETTING_REV, &unix_now().to_string())
        .map_err(|err| err.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_accepts_common_images() {
        assert_eq!(
            sanitize_filename("Church-Logo.PNG").as_deref(),
            Some("church-logo.png")
        );
        assert_eq!(sanitize_filename("../evil.svg"), None);
        assert_eq!(sanitize_filename("notes.txt"), None);
    }
}
