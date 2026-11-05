//! MeshCore contact QR codes: background encode → `data/qr/*.svg` → SQLite pointers.
//! Map / UI only *read* cached files; they never encode on the refresh path.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use qrcode::render::svg;
use qrcode::{EcLevel, QrCode};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

use crate::db::Db;

static QR_TX: OnceLock<UnboundedSender<String>> = OnceLock::new();
static QR_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Build `meshcore://contact/add?name=…&public_key=…&type=N`.
pub fn contact_add_uri(name: &str, pubkey_hex: &str, contact_type: u8) -> String {
    let key = pubkey_hex.trim().to_ascii_lowercase();
    let ty = match contact_type {
        1..=4 => contact_type,
        _ => 1,
    };
    format!(
        "meshcore://contact/add?name={}&public_key={}&type={}",
        urlencode_component(name.trim()),
        urlencode_component(&key),
        ty
    )
}

/// Parse a `meshcore://contact/add?…` URI into `(name, pubkey, type)`.
/// Returns `None` for binary adverts or malformed links.
pub fn parse_contact_add_uri(uri: &str) -> Option<(String, String, u8)> {
    let uri = uri.trim();
    let rest = uri
        .strip_prefix("meshcore://contact/add?")
        .or_else(|| uri.strip_prefix("MESHCORE://contact/add?"))?;
    let mut name = String::new();
    let mut pubkey = String::new();
    let mut ty: u8 = 1;
    for pair in rest.split('&') {
        let mut parts = pair.splitn(2, '=');
        let key = parts.next().unwrap_or("");
        let value = urldecode_component(parts.next().unwrap_or(""));
        match key {
            "name" => name = value,
            "public_key" => pubkey = value.trim().to_ascii_lowercase(),
            "type" => {
                ty = value.parse::<u8>().unwrap_or(1).clamp(1, 4);
            }
            _ => {}
        }
    }
    if pubkey.len() != 64 || !pubkey.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    if name.trim().is_empty() {
        name = pubkey.chars().take(12).collect();
    }
    Some((name, pubkey, ty))
}

fn urldecode_component(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let h = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                if let Some(h) = h {
                    if let Ok(v) = u8::from_str_radix(h, 16) {
                        out.push(v);
                        i += 3;
                        continue;
                    }
                }
                out.push(bytes[i]);
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Public URL for a stored QR file name (`{pubkey}.svg`).
pub fn qr_public_url(qr_file: &str) -> Option<String> {
    let name = sanitize_qr_filename(qr_file)?;
    Some(format!("metis://localhost/qr/{name}"))
}

/// Absolute path under the app data dir for a QR file name.
pub fn qr_disk_path(qr_file: &str) -> Option<PathBuf> {
    let name = sanitize_qr_filename(qr_file)?;
    Some(qr_dir().join(name))
}

/// Read a cached SVG from disk for the custom protocol handler.
pub fn read_qr_file(name: &str) -> Option<Vec<u8>> {
    let path = qr_disk_path(name)?;
    std::fs::read(path).ok()
}

/// SVG data URL for Identity (one-off read from disk cache).
pub fn qr_data_url_from_file(qr_file: &str) -> Option<String> {
    let bytes = read_qr_file(qr_file)?;
    let svg = String::from_utf8(bytes).ok()?;
    Some(format!(
        "data:image/svg+xml;charset=utf-8,{}",
        urlencode_component(&svg)
    ))
}

fn qr_dir() -> &'static Path {
    QR_DIR.get().map(PathBuf::as_path).unwrap_or_else(|| Path::new("data/qr"))
}

fn sanitize_qr_filename(name: &str) -> Option<String> {
    let base = name.trim().trim_start_matches('/').to_ascii_lowercase();
    let stem = base.strip_suffix(".svg").unwrap_or(&base);
    if stem.is_empty() || !stem.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    // Full 64-char pubkey or a short prefix — keep it bounded.
    if stem.len() > 64 {
        return None;
    }
    Some(format!("{stem}.svg"))
}

fn render_svg(uri: &str) -> Option<String> {
    let code = QrCode::with_error_correction_level(uri.as_bytes(), EcLevel::M).ok()?;
    Some(
        code.render::<svg::Color>()
            .min_dimensions(160, 160)
            .dark_color(svg::Color("#0f172a"))
            .light_color(svg::Color("#ffffff"))
            .build(),
    )
}

fn share_type(enrollment: &str, contact_type: u8) -> u8 {
    if enrollment == "hq" {
        1
    } else if (1..=4).contains(&contact_type) {
        contact_type
    } else {
        1
    }
}

fn urlencode_component(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() * 3);
    for b in raw.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(HEX[(b >> 4) as usize] as char);
            out.push(HEX[(b & 0xf) as usize] as char);
        }
    }
    out
}

const HEX: &[u8; 16] = b"0123456789ABCDEF";

/// Start the background QR worker. Safe to call once at process start.
pub fn spawn_qr_worker(db: Arc<Db>, data_dir: PathBuf) {
    let dir = data_dir.join("qr");
    let _ = std::fs::create_dir_all(&dir);
    let _ = QR_DIR.set(dir.clone());

    let (tx, rx) = unbounded_channel::<String>();
    let _ = QR_TX.set(tx);

    std::thread::Builder::new()
        .name("metis-qr-worker".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(err) => {
                    eprintln!("qr worker runtime: {err}");
                    return;
                }
            };
            rt.block_on(worker_loop(db, dir, rx));
        })
        .ok();
}

/// Queue a contact for (re)generation after upsert. No-op if worker is not up.
pub fn enqueue_contact_qr(pubkey: &str) {
    let key = pubkey.trim().to_ascii_lowercase();
    if key.is_empty() || !key.chars().all(|c| c.is_ascii_hexdigit()) {
        return;
    }
    if let Some(tx) = QR_TX.get() {
        let _ = tx.send(key);
    }
}

async fn worker_loop(
    db: Arc<Db>,
    dir: PathBuf,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<String>,
) {
    // Backfill anything missing or stale once at startup.
    match db.list_qr_backfill_pubkeys() {
        Ok(keys) => {
            for key in keys {
                if let Err(err) = generate_one(&db, &dir, &key) {
                    eprintln!("qr backfill {key}: {err}");
                }
                tokio::task::yield_now().await;
            }
        }
        Err(err) => eprintln!("qr backfill list: {err}"),
    }

    while let Some(first) = rx.recv().await {
        let mut batch = HashSet::new();
        batch.insert(first);
        while let Ok(more) = rx.try_recv() {
            batch.insert(more);
        }
        for key in batch {
            if let Err(err) = generate_one(&db, &dir, &key) {
                eprintln!("qr generate {key}: {err}");
            }
            tokio::task::yield_now().await;
        }
    }
}

fn generate_one(db: &Db, dir: &Path, pubkey: &str) -> Result<(), String> {
    let row = db
        .qr_contact_row(pubkey)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "contact gone".to_string())?;

    let ty = share_type(&row.enrollment, row.contact_type);
    let uri = contact_add_uri(&row.name, &row.pubkey, ty);
    let file = format!("{}.svg", row.pubkey.to_ascii_lowercase());
    let path = dir.join(&file);

    // Already current on disk + DB.
    if row.qr_uri.as_deref() == Some(uri.as_str())
        && row.qr_file.as_deref() == Some(file.as_str())
        && path.is_file()
    {
        return Ok(());
    }

    let svg = render_svg(&uri).ok_or_else(|| "qr encode failed".to_string())?;
    std::fs::write(&path, svg.as_bytes()).map_err(|e| e.to_string())?;
    db.set_node_qr(&row.pubkey, &uri, &file)
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Test helper: encode without the worker.
#[cfg(test)]
pub fn qr_svg_for_test(uri: &str) -> Option<String> {
    render_svg(uri)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_shape() {
        let uri = contact_add_uri("Rim Of World", "AaBb", 2);
        assert!(uri.starts_with("meshcore://contact/add?"));
        assert!(uri.contains("public_key=aabb"));
        assert!(uri.contains("type=2"));
        assert!(uri.contains("name=Rim%20Of%20World"));
    }

    #[test]
    fn parse_round_trip() {
        let key = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let uri = contact_add_uri("Rim Of World", key, 2);
        let (name, pubkey, ty) = parse_contact_add_uri(&uri).expect("parse");
        assert_eq!(name, "Rim Of World");
        assert_eq!(pubkey, key);
        assert_eq!(ty, 2);
        assert!(parse_contact_add_uri("meshcore://aabbcc").is_none());
    }

    #[test]
    fn sanitize_rejects_path_tricks() {
        assert!(sanitize_qr_filename("../etc/passwd").is_none());
        assert!(sanitize_qr_filename("abc.svg").is_some());
        assert_eq!(
            sanitize_qr_filename("ABCDEF.svg").as_deref(),
            Some("abcdef.svg")
        );
    }

    #[test]
    fn qr_renders() {
        let uri = contact_add_uri(
            "HQ",
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            1,
        );
        let svg = qr_svg_for_test(&uri).expect("svg");
        assert!(svg.contains("<svg"));
    }
}
