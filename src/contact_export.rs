//! Bulk contact JSON export / import (HQ DB only — never pushes to the radio).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::contact_qr::contact_add_uri;
use crate::db::{unix_now, Db};
use crate::signup::Enrollment;

const FORMAT: &str = "metis-hq-contacts";
const VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContactsFile {
    pub format: String,
    pub version: u32,
    pub exported_at: i64,
    pub contacts: Vec<ContactExport>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContactExport {
    pub public_key: String,
    pub name: String,
    #[serde(rename = "type")]
    pub contact_type: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lat: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lon: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enrollment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_heard: Option<i64>,
}

#[derive(Debug, Clone, Default)]
pub struct ImportSummary {
    pub new: usize,
    pub updated: usize,
    pub skipped: usize,
}

pub fn export_contacts(db: &Db) -> Result<ContactsFile, String> {
    let nodes = db.list_nodes().map_err(|e| e.to_string())?;
    let mut contacts = Vec::with_capacity(nodes.len());
    for node in nodes {
        let (lat, lon) = match (node.lat, node.lon) {
            (Some(la), Some(lo)) if la.abs() > 0.000_001 || lo.abs() > 0.000_001 => {
                (Some(la), Some(lo))
            }
            _ => (None, None),
        };
        contacts.push(ContactExport {
            public_key: node.pubkey,
            name: node.name,
            contact_type: node.contact_type,
            lat,
            lon,
            enrollment: Some(node.enrollment.as_str().into()),
            last_heard: node.last_heard,
        });
    }
    Ok(ContactsFile {
        format: FORMAT.into(),
        version: VERSION,
        exported_at: unix_now(),
        contacts,
    })
}

pub fn write_contacts_json(db: &Db, path: &Path) -> Result<usize, String> {
    let file = export_contacts(db)?;
    let count = file.contacts.len();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
    fs::write(path, json).map_err(|e| e.to_string())?;
    Ok(count)
}

/// One `meshcore://contact/add…` URI per line for phone-app pastes.
pub fn write_contacts_uri_list(db: &Db, path: &Path) -> Result<usize, String> {
    let file = export_contacts(db)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut lines = Vec::with_capacity(file.contacts.len());
    for c in &file.contacts {
        lines.push(contact_add_uri(&c.name, &c.public_key, c.contact_type));
    }
    fs::write(path, lines.join("\n") + "\n").map_err(|e| e.to_string())?;
    Ok(lines.len())
}

#[derive(Debug, Clone)]
pub struct ContactExportEntry {
    pub path: PathBuf,
    pub created_at: i64,
    pub file_name: String,
}

impl ContactExportEntry {
    pub fn path_string(&self) -> String {
        self.path.display().to_string()
    }
}

fn export_created_at(path: &Path) -> i64 {
    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
        if let Some(stamp) = stem.strip_prefix("contacts-") {
            if let Ok(secs) = stamp.parse::<i64>() {
                if secs > 0 {
                    return secs;
                }
            }
        }
    }
    // Prefer exported_at inside JSON when present.
    if let Ok(file) = read_contacts_file(path) {
        if file.exported_at > 0 {
            return file.exported_at;
        }
    }
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Write a timestamped contacts JSON (+ URI list) under `data/exports/`.
pub fn create_timestamped_export(db: &Db, data_dir: &Path) -> Result<(PathBuf, usize), String> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let dir = data_dir.join("exports");
    let json_path = dir.join(format!("contacts-{stamp}.json"));
    let uri_path = dir.join(format!("contacts-{stamp}-uris.txt"));
    let n = write_contacts_json(db, &json_path)?;
    let _ = write_contacts_uri_list(db, &uri_path);
    Ok((json_path, n))
}

pub fn list_contact_exports(data_dir: &Path) -> Vec<ContactExportEntry> {
    let dir = data_dir.join("exports");
    let mut entries = fs::read_dir(&dir)
        .ok()
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.extension()
                        .and_then(|x| x.to_str())
                        .is_some_and(|x| x.eq_ignore_ascii_case("json"))
                        && p.file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| n.starts_with("contacts"))
                })
                .map(|path| {
                    let file_name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("contacts.json")
                        .to_string();
                    let created_at = export_created_at(&path);
                    ContactExportEntry {
                        path,
                        created_at,
                        file_name,
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

pub fn delete_contact_export(path: &Path) -> Result<(), String> {
    if !path.is_file() {
        return Err("Export file not found.".into());
    }
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| "Invalid export path.".to_string())?;
    if !name.starts_with("contacts") || !name.ends_with(".json") {
        return Err("Not a contacts export file.".into());
    }
    let parent_ok = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        == Some("exports");
    if !parent_ok {
        return Err("Can only delete exports in data/exports.".into());
    }
    fs::remove_file(path).map_err(|e| e.to_string())?;
    // Best-effort sibling URI list (`contacts-123.json` → `contacts-123-uris.txt`).
    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
        if let Some(parent) = path.parent() {
            let _ = fs::remove_file(parent.join(format!("{stem}-uris.txt")));
        }
    }
    Ok(())
}

pub fn export_contacts_with_dialog(source: &Path) -> Result<PathBuf, String> {
    if !source.is_file() {
        return Err("Export file not found.".into());
    }
    let suggested = source
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("contacts.json");
    let dest = rfd::FileDialog::new()
        .set_title("Save contacts export")
        .set_file_name(suggested)
        .add_filter("Contacts JSON", &["json"])
        .save_file()
        .ok_or_else(|| "Save cancelled.".to_string())?;
    fs::copy(source, &dest).map_err(|e| e.to_string())?;
    Ok(dest)
}

/// Pick an external contacts JSON and copy it into `data/exports/` for import.
pub fn import_contacts_with_dialog(data_dir: &Path) -> Result<PathBuf, String> {
    let source = rfd::FileDialog::new()
        .set_title("Open contacts export")
        .add_filter("Contacts JSON", &["json"])
        .pick_file()
        .ok_or_else(|| "Open cancelled.".to_string())?;
    if !source.is_file() {
        return Err("Export file not found.".into());
    }
    // Validate format before copying into the HQ exports folder.
    let _ = read_contacts_file(&source)?;
    let dir = data_dir.join("exports");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let base = source
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| n.starts_with("contacts") && n.ends_with(".json"))
        .map(|n| n.to_string())
        .unwrap_or_else(|| format!("contacts-import-{stamp}.json"));
    let mut dest = dir.join(&base);
    if dest.exists() {
        let stem = base.trim_end_matches(".json");
        dest = dir.join(format!("{stem}-{stamp}.json"));
    }
    fs::copy(&source, &dest).map_err(|e| e.to_string())?;
    Ok(dest)
}

pub fn parse_contacts_json(raw: &str) -> Result<ContactsFile, String> {
    let file: ContactsFile = serde_json::from_str(raw).map_err(|e| e.to_string())?;
    if file.format != FORMAT {
        return Err(format!("unsupported contacts format {:?}", file.format));
    }
    if file.version != VERSION {
        return Err(format!("unsupported contacts version {}", file.version));
    }
    Ok(file)
}

pub fn read_contacts_file(path: &Path) -> Result<ContactsFile, String> {
    let raw = fs::read_to_string(path).map_err(|e| e.to_string())?;
    parse_contacts_json(&raw)
}

pub fn import_contacts(db: &Db, file: &ContactsFile) -> Result<ImportSummary, String> {
    let mut summary = ImportSummary::default();
    let mut touched = Vec::new();
    for c in &file.contacts {
        let enrollment = c
            .enrollment
            .as_deref()
            .map(Enrollment::parse)
            .unwrap_or(Enrollment::Heard);
        let status = db
            .import_roster_contact(
                &c.public_key,
                &c.name,
                c.contact_type,
                c.lat,
                c.lon,
                enrollment,
                c.last_heard,
            )
            .map_err(|e| e.to_string())?;
        match status {
            "new" => {
                summary.new += 1;
                touched.push(c.public_key.trim().to_ascii_lowercase());
            }
            "updated" => {
                summary.updated += 1;
                touched.push(c.public_key.trim().to_ascii_lowercase());
            }
            _ => summary.skipped += 1,
        }
    }
    for key in touched {
        crate::contact_qr::enqueue_contact_qr(&key);
    }
    let _ = db.audit(
        "contacts_import",
        "",
        &format!(
            "new={} updated={} skipped={}",
            summary.new, summary.updated, summary.skipped
        ),
    );
    Ok(summary)
}

pub fn import_contacts_path(db: &Db, path: &Path) -> Result<ImportSummary, String> {
    let file = read_contacts_file(path)?;
    import_contacts(db, &file)
}

pub fn preview_import(file: &ContactsFile) -> String {
    format!(
        "{} contacts in file (exported {}).",
        file.contacts.len(),
        file.exported_at
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn export_import_merges_without_downgrade() {
        let db = Db::from_conn(Connection::open_in_memory().unwrap()).unwrap();
        db.ensure_defaults().unwrap();
        let key = "aa".repeat(32);
        db.import_roster_contact(
            &key,
            "Ada",
            1,
            Some(34.0),
            Some(-117.0),
            Enrollment::Enrolled,
            Some(100),
        )
        .unwrap();

        let exported = export_contacts(&db).unwrap();
        assert_eq!(exported.format, FORMAT);
        assert!(exported.contacts.iter().any(|c| c.public_key == key));

        // Re-import as heard — must stay enrolled.
        let mut round = exported.clone();
        for c in &mut round.contacts {
            if c.public_key == key {
                c.enrollment = Some("heard".into());
                c.name = "Ada Lovelace".into();
            }
        }
        let summary = import_contacts(&db, &round).unwrap();
        assert_eq!(summary.updated, 1);
        let node = db.get_node(&key).unwrap().unwrap();
        assert_eq!(node.enrollment, Enrollment::Enrolled);
        assert_eq!(node.name, "Ada Lovelace");
    }

    #[test]
    fn rejects_bad_format() {
        let err = parse_contacts_json(r#"{"format":"nope","version":1,"exported_at":0,"contacts":[]}"#)
            .unwrap_err();
        assert!(err.contains("unsupported"));
    }
}
