//! In-app GitHub release check + operator-confirmed assisted install.
//!
//! Background loop: startup delay, then every 6 hours. Failures stay silent
//! unless the operator clicks Check now. Install never runs without confirm.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use semver::Version;

use crate::db::Db;
use crate::toast::{self, ToastKind};

const REPO_LATEST: &str =
    "https://api.github.com/repos/digitalexpl0it/MetisMesh-HQ/releases/latest";
const USER_AGENT: &str = concat!(
    "MetisMeshHQ/",
    env!("CARGO_PKG_VERSION"),
    " (app update check; local desktop)"
);
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const STARTUP_DELAY: Duration = Duration::from_secs(45);

pub const SETTING_ENABLED: &str = "update_check_enabled";
pub const SETTING_LAST_CHECKED: &str = "update_last_checked";
pub const SETTING_AVAILABLE: &str = "update_available_version";
pub const SETTING_DISMISSED: &str = "update_dismissed_version";
pub const SETTING_TOASTED: &str = "update_toasted_version";

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

// ---------- Shared UI status ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallFlavor {
    AppImage,
    Deb,
    /// Constructed on Windows (and in unit tests). Matched on all targets for asset pick.
    #[allow(dead_code)]
    WindowsZip,
    TarGz,
}

impl InstallFlavor {
    pub fn label(self) -> &'static str {
        match self {
            Self::AppImage => "AppImage",
            Self::Deb => "Debian package",
            Self::WindowsZip => "Windows zip",
            Self::TarGz => "Linux tarball",
        }
    }

    pub fn primary_action_label(self) -> &'static str {
        match self {
            Self::Deb => "Install update",
            Self::AppImage | Self::WindowsZip | Self::TarGz => "Quit and update",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct UpdateStatus {
    pub check_running: bool,
    pub install_running: bool,
    pub phase: String,
    pub last_error: String,
    pub available_version: Option<String>,
    pub html_url: String,
    pub assets: Vec<ReleaseAsset>,
    pub last_checked_at: i64,
    /// Explicit Check now: surface “up to date” / errors in the UI.
    pub last_check_note: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReleaseAsset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
}

#[derive(Debug, Deserialize)]
struct GhRelease {
    tag_name: String,
    html_url: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<ReleaseAsset>,
}

static STATUS: OnceLock<Mutex<UpdateStatus>> = OnceLock::new();

fn status_lock() -> &'static Mutex<UpdateStatus> {
    STATUS.get_or_init(|| Mutex::new(UpdateStatus::default()))
}

pub fn status() -> UpdateStatus {
    status_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

fn set_status(update: impl FnOnce(&mut UpdateStatus)) {
    let mut guard = status_lock().lock().unwrap_or_else(|e| e.into_inner());
    update(&mut guard);
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn updates_dir() -> PathBuf {
    crate::data_dir().join("updates")
}

fn cache_paths() -> (PathBuf, PathBuf) {
    let dir = updates_dir();
    (dir.join("github-latest.json"), dir.join("github-latest.etag"))
}

// ---------- Version / asset helpers (unit-tested) ----------

pub fn parse_release_version(tag: &str) -> Option<Version> {
    let t = tag.trim().trim_start_matches('v').trim_start_matches('V');
    Version::parse(t).ok()
}

pub fn is_newer_than_current(remote_tag: &str) -> bool {
    let Some(remote) = parse_release_version(remote_tag) else {
        return false;
    };
    let Ok(current) = Version::parse(CURRENT_VERSION) else {
        return false;
    };
    remote > current
}

pub fn asset_matches_flavor(name: &str, flavor: InstallFlavor) -> bool {
    let n = name.to_ascii_lowercase();
    match flavor {
        InstallFlavor::Deb => n.starts_with("metis-mesh-hq_") && n.ends_with("_amd64.deb"),
        InstallFlavor::AppImage => {
            n.starts_with("metis-mesh-hq-x86_64") && n.ends_with(".appimage")
        }
        InstallFlavor::WindowsZip => {
            n.starts_with("metis-mesh-hq-windows-x86_64") && n.ends_with(".zip")
        }
        InstallFlavor::TarGz => {
            n.starts_with("metis-mesh-hq-linux-x86_64") && n.ends_with(".tar.gz")
        }
    }
}

pub fn pick_asset<'a>(assets: &'a [ReleaseAsset], flavor: InstallFlavor) -> Option<&'a ReleaseAsset> {
    assets.iter().find(|a| asset_matches_flavor(&a.name, flavor))
}

pub fn should_show_banner(available: Option<&str>, dismissed: Option<&str>) -> bool {
    let Some(avail) = available.map(str::trim).filter(|s| !s.is_empty()) else {
        return false;
    };
    if !is_newer_than_current(avail) {
        return false;
    }
    if let Some(d) = dismissed.map(str::trim).filter(|s| !s.is_empty()) {
        if parse_release_version(d) == parse_release_version(avail) {
            return false;
        }
    }
    true
}

pub fn detect_install_flavor() -> InstallFlavor {
    detect_install_flavor_with(
        cfg!(target_os = "windows"),
        std::env::var_os("APPIMAGE").is_some(),
        std::env::current_exe().ok(),
        dpkg_metis_installed(),
    )
}

/// Testable flavor detection (no live process env required).
pub fn detect_install_flavor_with(
    is_windows: bool,
    has_appimage_env: bool,
    current_exe: Option<PathBuf>,
    dpkg_installed: bool,
) -> InstallFlavor {
    if is_windows {
        return InstallFlavor::WindowsZip;
    }
    if has_appimage_env {
        return InstallFlavor::AppImage;
    }
    if let Some(exe) = current_exe {
        let s = exe.to_string_lossy();
        if s.contains("/usr/lib/metis-mesh-hq") || s.starts_with("/usr/bin/metis-mesh-hq") {
            return InstallFlavor::Deb;
        }
    }
    if dpkg_installed {
        return InstallFlavor::Deb;
    }
    InstallFlavor::TarGz
}

fn dpkg_metis_installed() -> bool {
    #[cfg(target_os = "linux")]
    {
        Command::new("dpkg")
            .args(["-s", "metis-mesh-hq"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

pub fn checks_enabled(db: &Db) -> bool {
    crate::services::setting_bool(db, SETTING_ENABLED, true)
}

pub fn banner_visible(db: &Db) -> bool {
    let st = status();
    let available = st
        .available_version
        .clone()
        .or_else(|| db.setting(SETTING_AVAILABLE).ok().flatten());
    let dismissed = db.setting(SETTING_DISMISSED).ok().flatten();
    should_show_banner(available.as_deref(), dismissed.as_deref())
}

// ---------- Spawn loop ----------

pub fn spawn_update_loop(db: Arc<Db>) {
    // Seed UI from last persisted available version.
    if let Ok(Some(ver)) = db.setting(SETTING_AVAILABLE) {
        if is_newer_than_current(&ver) {
            set_status(|s| {
                s.available_version = Some(ver);
            });
        }
    }
    if let Ok(Some(raw)) = db.setting(SETTING_LAST_CHECKED) {
        if let Ok(at) = raw.parse::<i64>() {
            set_status(|s| s.last_checked_at = at);
        }
    }

    thread::Builder::new()
        .name("app-update-loop".into())
        .spawn(move || {
            thread::sleep(STARTUP_DELAY);
            loop {
                if checks_enabled(&db) {
                    let last = db
                        .setting(SETTING_LAST_CHECKED)
                        .ok()
                        .flatten()
                        .and_then(|s| s.parse::<i64>().ok())
                        .unwrap_or(0);
                    let due = last == 0
                        || now_unix().saturating_sub(last) >= CHECK_INTERVAL.as_secs() as i64;
                    if due {
                        let _ = run_check(&db, false);
                    }
                }
                thread::sleep(Duration::from_secs(60));
            }
        })
        .ok();
}

/// Operator-facing Check now (may toast errors / up-to-date).
pub fn check_now(db: Arc<Db>) {
    thread::Builder::new()
        .name("app-update-check".into())
        .spawn(move || {
            match run_check(&db, true) {
                Ok(CheckOutcome::UpToDate) => {
                    toast::push(ToastKind::Success, "You're on the latest Metis Mesh HQ.");
                }
                Ok(CheckOutcome::UpdateAvailable(ver)) => {
                    toast::push_for(
                        ToastKind::Info,
                        format!("Metis Mesh HQ {ver} is available."),
                        Duration::from_secs(12),
                    );
                }
                Ok(CheckOutcome::Unchanged) => {}
                Err(err) => {
                    toast::push(ToastKind::Warn, format!("Update check failed: {err}"));
                }
            }
        })
        .ok();
}

#[derive(Debug)]
enum CheckOutcome {
    UpToDate,
    UpdateAvailable(String),
    Unchanged,
}

fn run_check(db: &Db, explicit: bool) -> Result<CheckOutcome, String> {
    if status().check_running {
        return Ok(CheckOutcome::Unchanged);
    }
    set_status(|s| {
        s.check_running = true;
        s.last_error.clear();
        if explicit {
            s.last_check_note = "Checking GitHub…".into();
        }
    });
    let result = fetch_and_apply(db, explicit);
    set_status(|s| {
        s.check_running = false;
        s.last_checked_at = now_unix();
    });
    let _ = db.set_setting(SETTING_LAST_CHECKED, &now_unix().to_string());
    result
}

fn fetch_and_apply(db: &Db, explicit: bool) -> Result<CheckOutcome, String> {
    let (body_path, etag_path) = cache_paths();
    let _ = fs::create_dir_all(updates_dir());
    let cached_etag = fs::read_to_string(&etag_path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let cached_body = fs::read_to_string(&body_path).ok();

    let mut req = ureq::get(REPO_LATEST)
        .set("User-Agent", USER_AGENT)
        .set("Accept", "application/vnd.github+json")
        .set("X-GitHub-Api-Version", "2022-11-28")
        .timeout(Duration::from_secs(45));
    if let Some(ref etag) = cached_etag {
        req = req.set("If-None-Match", etag);
    }

    let json = match req.call() {
        Ok(resp) => {
            let status_code = resp.status();
            let new_etag = resp
                .header("etag")
                .or_else(|| resp.header("ETag"))
                .map(|s| s.to_string());
            if status_code == 304 {
                cached_body.ok_or_else(|| "GitHub 304 but no local cache.".to_string())?
            } else {
                let body = read_body(resp)?;
                if let Some(etag) = new_etag.as_deref().map(str::trim).filter(|e| !e.is_empty()) {
                    let _ = fs::write(&etag_path, format!("{etag}\n"));
                    let _ = fs::write(&body_path, &body);
                }
                body
            }
        }
        Err(ureq::Error::Status(code, resp)) => {
            let body = read_body(resp).unwrap_or_default();
            let snippet: String = body.chars().take(160).collect();
            return Err(if snippet.is_empty() {
                format!("GitHub HTTP {code}")
            } else {
                format!("GitHub HTTP {code}: {snippet}")
            });
        }
        Err(err) => {
            if !explicit {
                hq_log!(info, "app update check skipped (offline/network): {err}");
            }
            return Err(err.to_string());
        }
    };

    let release: GhRelease =
        serde_json::from_str(&json).map_err(|e| format!("Invalid release JSON: {e}"))?;
    if release.draft || release.prerelease {
        let _ = db.set_setting(SETTING_AVAILABLE, "");
        set_status(|s| {
            s.available_version = None;
            s.assets.clear();
            s.html_url.clear();
            if explicit {
                s.last_check_note = "Latest GitHub release is a draft/prerelease.".into();
            }
        });
        return Ok(CheckOutcome::UpToDate);
    }

    let tag = release.tag_name.clone();
    let ver_label = parse_release_version(&tag)
        .map(|v| v.to_string())
        .unwrap_or_else(|| tag.trim_start_matches('v').to_string());

    if !is_newer_than_current(&tag) {
        let _ = db.set_setting(SETTING_AVAILABLE, "");
        set_status(|s| {
            s.available_version = None;
            s.assets.clear();
            s.html_url = release.html_url.clone();
            if explicit {
                s.last_check_note = format!("Up to date ({CURRENT_VERSION}).");
            }
        });
        return Ok(CheckOutcome::UpToDate);
    }

    let _ = db.set_setting(SETTING_AVAILABLE, &ver_label);
    set_status(|s| {
        s.available_version = Some(ver_label.clone());
        s.html_url = release.html_url.clone();
        s.assets = release.assets.clone();
        s.last_check_note = format!("Update {ver_label} available.");
    });

    // One toast per newly seen version (background or first discover).
    let toasted = db.setting(SETTING_TOASTED).ok().flatten().unwrap_or_default();
    if parse_release_version(&toasted) != parse_release_version(&ver_label) {
        let _ = db.set_setting(SETTING_TOASTED, &ver_label);
        if !explicit {
            toast::push_for(
                ToastKind::Info,
                format!("Metis Mesh HQ {ver_label} is available."),
                Duration::from_secs(12),
            );
        }
    }

    Ok(CheckOutcome::UpdateAvailable(ver_label))
}

fn read_body(resp: ureq::Response) -> Result<String, String> {
    let mut reader = resp.into_reader().take(4 * 1024 * 1024);
    let mut buf = String::new();
    reader
        .read_to_string(&mut buf)
        .map_err(|e| e.to_string())?;
    Ok(buf)
}

// ---------- Dismiss / open ----------

pub fn dismiss(db: &Db) {
    let ver = status()
        .available_version
        .clone()
        .or_else(|| db.setting(SETTING_AVAILABLE).ok().flatten())
        .unwrap_or_default();
    if !ver.is_empty() {
        let _ = db.set_setting(SETTING_DISMISSED, &ver);
    }
}

pub fn open_release_page() {
    let url = {
        let st = status();
        if st.html_url.trim().is_empty() {
            "https://github.com/digitalexpl0it/MetisMesh-HQ/releases/latest".to_string()
        } else {
            st.html_url.clone()
        }
    };
    if let Err(err) = crate::services::local_api::open_url_in_browser(&url) {
        toast::push(ToastKind::Warn, err);
    }
}

// ---------- Install (operator confirmed) ----------

pub fn start_install(db: Arc<Db>) {
    if status().install_running || status().check_running {
        toast::push(ToastKind::Warn, "An update check or install is already running.");
        return;
    }
    if crate::firmware::flash_status().running {
        toast::push(
            ToastKind::Warn,
            "Finish the firmware flash before updating HQ.",
        );
        return;
    }

    thread::Builder::new()
        .name("app-update-install".into())
        .spawn(move || {
            set_status(|s| {
                s.install_running = true;
                s.last_error.clear();
                s.phase = "Preparing…".into();
            });
            match install_inner(&db) {
                Ok(()) => {
                    set_status(|s| {
                        s.install_running = false;
                        s.phase.clear();
                    });
                }
                Err(err) => {
                    hq_log!(error, "app update install failed: {err}");
                    set_status(|s| {
                        s.install_running = false;
                        s.phase.clear();
                        s.last_error = err.clone();
                    });
                    toast::push(ToastKind::Error, err);
                }
            }
        })
        .ok();
}

fn install_inner(db: &Db) -> Result<(), String> {
    // Ensure we have asset metadata (restart may have version but empty assets).
    if status().assets.is_empty() {
        set_status(|s| s.phase = "Refreshing release metadata…".into());
        run_check(db, false).map_err(|e| e)?;
        if status().assets.is_empty() {
            return Err("Could not load release assets from GitHub.".into());
        }
    }
    let st = status();
    let version = st
        .available_version
        .clone()
        .ok_or_else(|| "No update available.".to_string())?;
    let flavor = detect_install_flavor();
    let asset = pick_asset(&st.assets, flavor)
        .cloned()
        .ok_or_else(|| {
            format!(
                "No {} asset on the GitHub release. Open the release page to download manually.",
                flavor.label()
            )
        })?;

    let dest = updates_dir().join(&asset.name);
    set_status(|s| {
        s.phase = format!("Downloading {}…", asset.name);
    });
    toast::push(
        ToastKind::Info,
        format!("Downloading Metis Mesh HQ {version}…"),
    );
    download_file(&asset.browser_download_url, &dest, Some(asset.size))?;

    match flavor {
        InstallFlavor::Deb => {
            set_status(|s| s.phase = "Installing .deb (admin prompt)…".into());
            install_deb(&dest)?;
            toast::push(
                ToastKind::Success,
                format!("Installed {version}. Restart Metis Mesh HQ to finish."),
            );
            let _ = db.set_setting(SETTING_AVAILABLE, "");
            set_status(|s| s.available_version = None);
            Ok(())
        }
        InstallFlavor::AppImage => {
            set_status(|s| s.phase = "Staging AppImage…".into());
            quit_and_replace_appimage(&dest)?;
            Ok(())
        }
        InstallFlavor::WindowsZip => {
            set_status(|s| s.phase = "Staging Windows build…".into());
            quit_and_replace_windows_zip(&dest, &version)?;
            Ok(())
        }
        InstallFlavor::TarGz => {
            set_status(|s| s.phase = "Staging Linux tarball…".into());
            quit_and_replace_targz(&dest, &version)?;
            Ok(())
        }
    }
}

fn download_file(url: &str, dest: &Path, expected_size: Option<u64>) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = dest.with_extension("partial");
    let _ = fs::remove_file(&tmp);
    let resp = ureq::get(url)
        .set("User-Agent", USER_AGENT)
        .timeout(Duration::from_secs(600))
        .call()
        .map_err(|e| format!("Download failed: {e}"))?;
    let mut reader = resp.into_reader();
    let mut file = File::create(&tmp).map_err(|e| e.to_string())?;
    let mut buf = [0u8; 64 * 1024];
    let mut written: u64 = 0;
    loop {
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        written += n as u64;
    }
    drop(file);
    if written == 0 {
        let _ = fs::remove_file(&tmp);
        return Err("Download was empty.".into());
    }
    if let Some(expected) = expected_size {
        if written != expected {
            let _ = fs::remove_file(&tmp);
            return Err(format!(
                "Download size mismatch: got {written}, expected {expected}"
            ));
        }
    }
    fs::rename(&tmp, dest).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn install_deb(deb_path: &Path) -> Result<(), String> {
    let pkexec = if Path::new("/usr/bin/pkexec").is_file() {
        "/usr/bin/pkexec"
    } else {
        "pkexec"
    };
    let deb = deb_path
        .canonicalize()
        .unwrap_or_else(|_| deb_path.to_path_buf());
    let deb_s = deb.to_string_lossy().to_string();
    let output = Command::new(pkexec)
        .args(["apt-get", "install", "-y", &deb_s])
        .output()
        .map_err(|e| {
            format!(
                "Could not run pkexec apt-get ({e}). Install manually: sudo apt-get install -y {deb_s}"
            )
        })?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    Err(format!(
        "apt-get install failed.\n{}\n{}",
        stdout.trim(),
        stderr.trim()
    ))
}

#[cfg(not(target_os = "linux"))]
fn install_deb(_deb_path: &Path) -> Result<(), String> {
    Err(".deb install is only supported on Linux.".into())
}

fn quit_and_replace_appimage(new_image: &Path) -> Result<(), String> {
    let appimage = std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .ok_or_else(|| "APPIMAGE path not set — not running from an AppImage.".to_string())?;
    let pid = std::process::id();
    let script = updates_dir().join("apply-appimage.sh");
    let new_s = new_image
        .canonicalize()
        .unwrap_or_else(|_| new_image.to_path_buf());
    let body = format!(
        "#!/bin/sh\nset -e\nTARGET={}\nNEW={}\nPID={pid}\nwhile kill -0 \"$PID\" 2>/dev/null; do sleep 0.4; done\nsleep 0.3\ncp -f \"$NEW\" \"$TARGET\"\nchmod +x \"$TARGET\"\nexec \"$TARGET\"\n",
        shell_single_quote(&appimage.to_string_lossy()),
        shell_single_quote(&new_s.to_string_lossy()),
    );
    fs::write(&script, body).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&script).map_err(|e| e.to_string())?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms).map_err(|e| e.to_string())?;
    }
    Command::new(&script)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Could not start AppImage updater: {e}"))?;
    toast::push(ToastKind::Info, "Quitting to finish the AppImage update…");
    thread::sleep(Duration::from_millis(400));
    std::process::exit(0);
}

fn quit_and_replace_targz(archive: &Path, version: &str) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let install_dir = exe
        .parent()
        .ok_or_else(|| "Could not resolve install directory.".to_string())?
        .to_path_buf();
    let stage = updates_dir().join(format!("stage-{version}"));
    let _ = fs::remove_dir_all(&stage);
    fs::create_dir_all(&stage).map_err(|e| e.to_string())?;

    let status = Command::new("tar")
        .args([
            "-xzf",
            &archive.to_string_lossy(),
            "-C",
            &stage.to_string_lossy(),
        ])
        .status()
        .map_err(|e| format!("tar extract failed: {e}"))?;
    if !status.success() {
        return Err("tar extract failed.".into());
    }

    // Tarball contains metis-mesh-hq + assets/ at top level (or nested).
    let staged_bin = find_staged_linux_binary(&stage)?;
    let staged_assets = find_staged_assets(&stage);
    let pid = std::process::id();
    let script = updates_dir().join("apply-targz.sh");
    let install_q = shell_single_quote(&install_dir.to_string_lossy());
    let assets_line = if let Some(ref assets) = staged_assets {
        format!(
            "rm -rf {}/assets\ncp -a {} {}/assets\n",
            install_q,
            shell_single_quote(&assets.to_string_lossy()),
            install_q,
        )
    } else {
        String::new()
    };
    let exe_q = shell_single_quote(&exe.to_string_lossy());
    let body = format!(
        "#!/bin/sh\nset -e\nPID={pid}\nwhile kill -0 \"$PID\" 2>/dev/null; do sleep 0.4; done\nsleep 0.3\ncp -f {} {}\nchmod +x {}\n{assets_line}exec {}\n",
        shell_single_quote(&staged_bin.to_string_lossy()),
        exe_q,
        exe_q,
        exe_q,
    );
    fs::write(&script, body).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&script).map_err(|e| e.to_string())?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms).map_err(|e| e.to_string())?;
    }
    Command::new(&script)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Could not start tarball updater: {e}"))?;
    toast::push(ToastKind::Info, "Quitting to finish the update…");
    thread::sleep(Duration::from_millis(400));
    std::process::exit(0);
}

fn quit_and_replace_windows_zip(archive: &Path, version: &str) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let install_dir = exe
        .parent()
        .ok_or_else(|| "Could not resolve install directory.".to_string())?
        .to_path_buf();
    let stage = updates_dir().join(format!("stage-{version}"));
    let _ = fs::remove_dir_all(&stage);
    fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
    extract_zip(archive, &stage)?;

    let staged_exe = find_staged_windows_exe(&stage)?;
    let staged_assets = find_staged_assets(&stage);
    let pid = std::process::id();
    let script = updates_dir().join("apply-windows.cmd");
    let exe_s = exe.to_string_lossy().replace('/', "\\");
    let staged_exe_s = staged_exe.to_string_lossy().replace('/', "\\");
    let install_s = install_dir.to_string_lossy().replace('/', "\\");
    let assets_block = if let Some(assets) = staged_assets {
        let assets_s = assets.to_string_lossy().replace('/', "\\");
        format!(
            "if exist \"{install_s}\\assets\" rmdir /s /q \"{install_s}\\assets\"\r\nxcopy /E /I /Y \"{assets_s}\" \"{install_s}\\assets\\\" >NUL\r\n"
        )
    } else {
        String::new()
    };
    let body = format!(
        "@echo off\r\nsetlocal\r\n:wait\r\ntasklist /FI \"PID eq {pid}\" 2>NUL | find \"{pid}\" >NUL\r\nif not errorlevel 1 (\r\n  timeout /t 1 /nobreak >NUL\r\n  goto wait\r\n)\r\ncopy /Y \"{staged_exe_s}\" \"{exe_s}\" >NUL\r\n{assets_block}start \"\" \"{exe_s}\"\r\n"
    );
    fs::write(&script, body).map_err(|e| e.to_string())?;
    Command::new("cmd")
        .args(["/C", &script.to_string_lossy()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Could not start Windows updater: {e}"))?;
    toast::push(ToastKind::Info, "Quitting to finish the update…");
    thread::sleep(Duration::from_millis(400));
    std::process::exit(0);
}

fn extract_zip(archive: &Path, dest: &Path) -> Result<(), String> {
    let file = File::open(archive).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
        let name = entry
            .enclosed_name()
            .ok_or_else(|| "Unsafe zip entry path.".to_string())?
            .to_path_buf();
        let out = dest.join(&name);
        if entry.is_dir() {
            fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut outfile = File::create(&out).map_err(|e| e.to_string())?;
        std::io::copy(&mut entry, &mut outfile).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn find_staged_linux_binary(stage: &Path) -> Result<PathBuf, String> {
    let direct = stage.join("metis-mesh-hq");
    if direct.is_file() {
        return Ok(direct);
    }
    for entry in walk_files(stage) {
        if entry.file_name().and_then(|s| s.to_str()) == Some("metis-mesh-hq") {
            return Ok(entry);
        }
    }
    Err("Staged tarball has no metis-mesh-hq binary.".into())
}

fn find_staged_windows_exe(stage: &Path) -> Result<PathBuf, String> {
    for entry in walk_files(stage) {
        let name = entry
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if name.ends_with(".exe") && name.contains("metis-mesh-hq") {
            return Ok(entry);
        }
    }
    Err("Staged zip has no Metis Mesh HQ .exe.".into())
}

fn find_staged_assets(stage: &Path) -> Option<PathBuf> {
    let direct = stage.join("assets");
    if direct.is_dir() {
        return Some(direct);
    }
    for entry in walk_dirs(stage) {
        if entry.file_name().and_then(|s| s.to_str()) == Some("assets") {
            return Some(entry);
        }
    }
    None
}

fn walk_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(root) else {
        return out;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk_files(&path));
        } else if path.is_file() {
            out.push(path);
        }
    }
    out
}

fn walk_dirs(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(root) else {
        return out;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.push(path.clone());
            out.extend(walk_dirs(&path));
        }
    }
    out
}

fn shell_single_quote(s: &str) -> String {
    // Embed in single-quoted shell string.
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tags() {
        assert_eq!(
            parse_release_version("v0.1.22").unwrap().to_string(),
            "0.1.22"
        );
        assert_eq!(
            parse_release_version("0.1.22").unwrap().to_string(),
            "0.1.22"
        );
        assert!(parse_release_version("nope").is_none());
    }

    #[test]
    fn newer_compare() {
        let current = Version::parse(CURRENT_VERSION).unwrap();
        let bump = Version::new(current.major, current.minor, current.patch + 1);
        assert!(is_newer_than_current(&format!("v{bump}")));
        assert!(!is_newer_than_current(CURRENT_VERSION));
        assert!(!is_newer_than_current(&format!("v{CURRENT_VERSION}")));
    }

    #[test]
    fn asset_names() {
        assert!(asset_matches_flavor(
            "metis-mesh-hq_0.1.22_amd64.deb",
            InstallFlavor::Deb
        ));
        assert!(asset_matches_flavor(
            "metis-mesh-hq-x86_64-v0.1.22.AppImage",
            InstallFlavor::AppImage
        ));
        assert!(asset_matches_flavor(
            "metis-mesh-hq-windows-x86_64-v0.1.22.zip",
            InstallFlavor::WindowsZip
        ));
        assert!(asset_matches_flavor(
            "metis-mesh-hq-linux-x86_64-v0.1.22.tar.gz",
            InstallFlavor::TarGz
        ));
        assert!(!asset_matches_flavor(
            "metis-mesh-hq-linux-x86_64-v0.1.22.tar.gz",
            InstallFlavor::Deb
        ));
    }

    #[test]
    fn pick_asset_prefers_flavor() {
        let assets = vec![
            ReleaseAsset {
                name: "metis-mesh-hq_0.1.22_amd64.deb".into(),
                browser_download_url: "https://example/a".into(),
                size: 1,
            },
            ReleaseAsset {
                name: "metis-mesh-hq-x86_64-v0.1.22.AppImage".into(),
                browser_download_url: "https://example/b".into(),
                size: 2,
            },
        ];
        assert_eq!(
            pick_asset(&assets, InstallFlavor::AppImage).unwrap().name,
            "metis-mesh-hq-x86_64-v0.1.22.AppImage"
        );
        assert_eq!(
            pick_asset(&assets, InstallFlavor::Deb).unwrap().name,
            "metis-mesh-hq_0.1.22_amd64.deb"
        );
        assert!(pick_asset(&assets, InstallFlavor::WindowsZip).is_none());
    }

    #[test]
    fn dismiss_hides_same_version() {
        assert!(should_show_banner(Some("9.9.9"), None));
        assert!(!should_show_banner(Some("9.9.9"), Some("9.9.9")));
        assert!(!should_show_banner(Some("9.9.9"), Some("v9.9.9")));
        assert!(!should_show_banner(Some(CURRENT_VERSION), None));
        assert!(!should_show_banner(None, None));
    }

    #[test]
    fn flavor_detection_matrix() {
        assert_eq!(
            detect_install_flavor_with(true, false, None, false),
            InstallFlavor::WindowsZip
        );
        assert_eq!(
            detect_install_flavor_with(false, true, None, false),
            InstallFlavor::AppImage
        );
        assert_eq!(
            detect_install_flavor_with(
                false,
                false,
                Some(PathBuf::from("/usr/lib/metis-mesh-hq/metis-mesh-hq")),
                false
            ),
            InstallFlavor::Deb
        );
        assert_eq!(
            detect_install_flavor_with(false, false, None, true),
            InstallFlavor::Deb
        );
        assert_eq!(
            detect_install_flavor_with(
                false,
                false,
                Some(PathBuf::from("/home/user/metis-mesh-hq")),
                false
            ),
            InstallFlavor::TarGz
        );
    }
}
