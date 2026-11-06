//! Rotating on-disk copy of the app log, readable outside HQ (online or off).
//!
//! A fresh `metis-hq.log` starts every launch; older sessions shift to
//! `metis-hq.1.log` … `metis-hq.4.log` and the oldest is deleted. A session
//! that passes `MAX_FILE_BYTES` rotates mid-run, so disk use stays under
//! `MAX_FILES * MAX_FILE_BYTES`. Lines arrive already redacted by `app_log`.
//! Writes happen on one background thread so the UI never waits on the disk.

use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Mutex, OnceLock};
use std::thread;

use crate::app_log::LogLevel;

/// Settings key; default on.
pub const SETTING_KEY: &str = "log_to_file";
pub const MAX_FILES: usize = 5;
pub const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
const FILE_STEM: &str = "metis-hq";

#[derive(Default)]
struct State {
    dir: Option<PathBuf>,
    tx: Option<Sender<String>>,
    last_error: Option<String>,
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(State::default()))
}

fn lock() -> std::sync::MutexGuard<'static, State> {
    state().lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Summary for the Settings card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogFileStatus {
    pub enabled: bool,
    pub dir: Option<PathBuf>,
    pub files: usize,
    pub total_bytes: u64,
    pub last_error: Option<String>,
}

pub fn current_path(dir: &Path) -> PathBuf {
    dir.join(format!("{FILE_STEM}.log"))
}

fn numbered_path(dir: &Path, index: usize) -> PathBuf {
    dir.join(format!("{FILE_STEM}.{index}.log"))
}

/// `metis-hq.N.log` → `Some(N)`; anything else → `None`.
fn rotation_index(file_name: &str) -> Option<usize> {
    file_name
        .strip_prefix(FILE_STEM)?
        .strip_prefix('.')?
        .strip_suffix(".log")?
        .parse()
        .ok()
}

/// Called once at startup. Lines already in the in-app buffer (pushed before
/// the database opened) are copied into the new file.
pub fn init(dir: PathBuf, enabled: bool) {
    {
        let mut guard = lock();
        guard.dir = Some(dir);
    }
    if enabled {
        start(true);
    }
}

/// Settings toggle. Turning on mid-session appends to the current file.
pub fn set_enabled(enabled: bool) {
    let running = lock().tx.is_some();
    if enabled && !running {
        start(false);
        hq_log!(info, "Log file turned on.");
    } else if !enabled && running {
        hq_log!(info, "Log file turned off.");
        // Dropping the sender lets the writer drain, flush, and exit.
        lock().tx = None;
    }
}

fn start(new_session: bool) {
    let Some(dir) = lock().dir.clone() else {
        return;
    };
    match open_writer(&dir, new_session) {
        Ok((file, written)) => {
            let (tx, rx) = mpsc::channel::<String>();
            if new_session {
                let header = format!(
                    "==== Metis Mesh HQ {} · {} {} · started {} ====\n",
                    env!("CARGO_PKG_VERSION"),
                    std::env::consts::OS,
                    std::env::consts::ARCH,
                    chrono::Local::now().format("%Y-%m-%d %H:%M:%S %:z"),
                );
                let _ = tx.send(header);
                for entry in crate::app_log::snapshot() {
                    let _ = tx.send(format_line(entry.level, local_stamp_secs(entry.at), &entry.message));
                }
            }
            let thread_dir = dir.clone();
            let spawned = thread::Builder::new()
                .name("hq-log-file".into())
                .spawn(move || run_writer(thread_dir, rx, file, written));
            let mut guard = lock();
            match spawned {
                Ok(_) => {
                    guard.tx = Some(tx);
                    guard.last_error = None;
                }
                Err(err) => {
                    guard.last_error = Some(format!("Could not start the log writer: {err}"));
                }
            }
        }
        Err(err) => {
            let msg = format!("Could not open the log file in {}: {err}", dir.display());
            eprintln!("[warn] {msg}");
            lock().last_error = Some(msg);
        }
    }
}

fn open_writer(dir: &Path, new_session: bool) -> std::io::Result<(BufWriter<File>, u64)> {
    fs::create_dir_all(dir)?;
    if new_session {
        rotate(dir);
    }
    let path = current_path(dir);
    let file = OpenOptions::new().create(true).append(true).open(&path)?;
    let written = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    Ok((BufWriter::new(file), written))
}

/// Shift `metis-hq.log` → `.1` → … → `.{MAX_FILES-1}` and drop anything older.
fn rotate(dir: &Path) {
    // Remove files past the cap (also cleans up if MAX_FILES is ever lowered).
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            if let Some(index) = name.to_str().and_then(rotation_index) {
                if index >= MAX_FILES - 1 {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
    }
    for index in (1..MAX_FILES).rev() {
        let from = if index == 1 {
            current_path(dir)
        } else {
            numbered_path(dir, index - 1)
        };
        if from.exists() {
            let to = numbered_path(dir, index);
            // Windows rename does not overwrite.
            let _ = fs::remove_file(&to);
            if let Err(err) = fs::rename(&from, &to) {
                eprintln!("[warn] log rotate {} → {}: {err}", from.display(), to.display());
            }
        }
    }
}

fn run_writer(dir: PathBuf, rx: Receiver<String>, mut file: BufWriter<File>, mut written: u64) {
    // Block for one line, drain whatever else is queued, then flush once.
    while let Ok(first) = rx.recv() {
        let mut batch = vec![first];
        batch.extend(rx.try_iter());
        for line in batch {
            let len = line.len() as u64;
            if written > 0 && written + len > MAX_FILE_BYTES {
                let _ = file.flush();
                drop(file);
                match open_writer(&dir, true) {
                    Ok((next, size)) => {
                        file = next;
                        written = size;
                    }
                    Err(err) => {
                        writer_failed(format!("Log rotation failed: {err}"));
                        return;
                    }
                }
            }
            if let Err(err) = file.write_all(line.as_bytes()) {
                writer_failed(format!("Log file write failed: {err}"));
                return;
            }
            written += len;
        }
        if let Err(err) = file.flush() {
            writer_failed(format!("Log file write failed: {err}"));
            return;
        }
    }
    let _ = file.flush();
}

fn writer_failed(msg: String) {
    eprintln!("[warn] {msg}");
    let mut guard = lock();
    guard.tx = None;
    guard.last_error = Some(msg);
}

fn local_stamp_secs(at: i64) -> String {
    chrono::DateTime::from_timestamp(at, 0)
        .map(|dt| {
            dt.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S%.3f %:z")
                .to_string()
        })
        .unwrap_or_else(|| "----------".into())
}

fn format_line(level: LogLevel, stamp: String, message: &str) -> String {
    // One record per line: multi-line messages stay greppable.
    let flat = message.replace("\r\n", " ⏎ ").replace(['\n', '\r'], " ⏎ ");
    format!("{stamp} [{:<7}] {flat}\n", level.as_str())
}

/// Called by `app_log::push` for every line (already redacted).
pub fn write_line(level: LogLevel, message: &str) {
    let guard = lock();
    let Some(tx) = guard.tx.as_ref() else {
        return;
    };
    let stamp = chrono::Local::now()
        .format("%Y-%m-%d %H:%M:%S%.3f %:z")
        .to_string();
    let _ = tx.send(format_line(level, stamp, message));
}

/// Panic hook path: write straight to disk so the line survives the exit.
pub fn write_line_now(level: LogLevel, message: &str) {
    let Some(dir) = lock().dir.clone() else {
        return;
    };
    let stamp = chrono::Local::now()
        .format("%Y-%m-%d %H:%M:%S%.3f %:z")
        .to_string();
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(current_path(&dir))
    {
        let _ = file.write_all(format_line(level, stamp, message).as_bytes());
        let _ = file.flush();
    }
}

fn log_files(dir: &Path) -> Vec<(PathBuf, u64)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| {
            entry.file_name().to_str().is_some_and(|name| {
                name == format!("{FILE_STEM}.log") || rotation_index(name).is_some()
            })
        })
        .filter_map(|entry| {
            let size = entry.metadata().ok()?.len();
            Some((entry.path(), size))
        })
        .collect()
}

pub fn status() -> LogFileStatus {
    let (enabled, dir, last_error) = {
        let guard = lock();
        (guard.tx.is_some(), guard.dir.clone(), guard.last_error.clone())
    };
    let files = dir.as_deref().map(log_files).unwrap_or_default();
    LogFileStatus {
        enabled,
        dir,
        files: files.len(),
        total_bytes: files.iter().map(|(_, size)| size).sum(),
        last_error,
    }
}

/// Delete rotated files (`.1` … `.N`). The current file stays; it is open
/// while logging is on and Windows cannot delete an open file.
pub fn delete_old() -> Result<usize, String> {
    let Some(dir) = lock().dir.clone() else {
        return Ok(0);
    };
    let mut removed = 0;
    for (path, _) in log_files(&dir) {
        let rotated = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(rotation_index)
            .is_some();
        if rotated {
            fs::remove_file(&path)
                .map_err(|err| format!("Could not delete {}: {err}", path.display()))?;
            removed += 1;
        }
    }
    Ok(removed)
}

/// Open the logs folder in the system file manager (off the UI thread).
pub fn open_folder() -> Result<(), String> {
    let dir = lock()
        .dir
        .clone()
        .ok_or_else(|| "Log folder is not set up yet.".to_string())?;
    fs::create_dir_all(&dir).map_err(|err| format!("Could not create {}: {err}", dir.display()))?;
    thread::Builder::new()
        .name("open-log-dir".into())
        .spawn(move || {
            if let Err(err) = crate::firmware::open_folder_detached(&dir) {
                hq_log!(warn, "open log folder: {err}");
            }
        })
        .map(|_| ())
        .map_err(|err| err.to_string())
}

pub fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b < KB {
        format!("{bytes} B")
    } else if b < KB * KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{:.1} MB", b / (KB * KB))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "metis-log-test-{tag}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn rotation_index_parses_only_numbered_logs() {
        assert_eq!(rotation_index("metis-hq.1.log"), Some(1));
        assert_eq!(rotation_index("metis-hq.12.log"), Some(12));
        assert_eq!(rotation_index("metis-hq.log"), None);
        assert_eq!(rotation_index("metis-hq.x.log"), None);
        assert_eq!(rotation_index("other.1.log"), None);
    }

    #[test]
    fn rotate_keeps_at_most_max_files() {
        let dir = temp_dir("rotate");
        for session in 0..(MAX_FILES + 3) {
            rotate(&dir);
            fs::write(current_path(&dir), format!("session {session}\n")).unwrap();
        }
        let files = log_files(&dir);
        assert_eq!(files.len(), MAX_FILES, "{files:?}");
        let newest = fs::read_to_string(current_path(&dir)).unwrap();
        assert_eq!(newest, format!("session {}\n", MAX_FILES + 2));
        let oldest = fs::read_to_string(numbered_path(&dir, MAX_FILES - 1)).unwrap();
        assert_eq!(oldest, "session 3\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotate_drops_files_beyond_cap() {
        let dir = temp_dir("cap");
        fs::write(numbered_path(&dir, MAX_FILES + 4), "stale").unwrap();
        rotate(&dir);
        assert!(!numbered_path(&dir, MAX_FILES + 4).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn format_line_flattens_newlines() {
        let line = format_line(LogLevel::Warn, "T".into(), "a\nb\r\nc");
        assert_eq!(line, "T [warn   ] a ⏎ b ⏎ c\n");
    }

    #[test]
    fn writer_rotates_when_file_is_full() {
        let dir = temp_dir("writer");
        let (file, _) = open_writer(&dir, true).unwrap();
        let (tx, rx) = mpsc::channel();
        let big = "x".repeat(1024) + "\n";
        let lines = (MAX_FILE_BYTES / big.len() as u64) + 10;
        for _ in 0..lines {
            tx.send(big.clone()).unwrap();
        }
        drop(tx);
        run_writer(dir.clone(), rx, file, 0);
        let current = fs::metadata(current_path(&dir)).unwrap().len();
        let rotated = fs::metadata(numbered_path(&dir, 1)).unwrap().len();
        assert!(rotated <= MAX_FILE_BYTES, "{rotated}");
        assert!(current > 0 && current < MAX_FILE_BYTES, "{current}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn format_bytes_units() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2048), "2.0 KB");
        assert_eq!(format_bytes(3 * 1024 * 1024), "3.0 MB");
    }
}
