//! Runtime dependency probe + install helpers for first-run setup.
//!
//! Critical checks (WebKitGTK / WebView2) run before the window opens.
//! Soft checks (BlueZ, dialout, flasher CLIs) run inside the welcome UI.

use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Spawn helper processes without flashing a console window on Windows.
fn command(program: &str) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CriticalStatus {
    pub ok: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoftDepKind {
    Bluez,
    Dialout,
    Esptool,
    Nrfutil,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoftDepItem {
    pub kind: SoftDepKind,
    pub label: String,
    pub ok: bool,
    pub detail: String,
    /// When true, welcome can offer an Install button.
    pub can_install: bool,
    pub hint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoftDepsReport {
    pub items: Vec<SoftDepItem>,
}

impl SoftDepKind {
    #[cfg_attr(not(test), allow(dead_code))] // exercised in unit tests; kept for UI string round-trips
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bluez => "bluez",
            Self::Dialout => "dialout",
            Self::Esptool => "esptool",
            Self::Nrfutil => "nrfutil",
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "bluez" => Some(Self::Bluez),
            "dialout" => Some(Self::Dialout),
            "esptool" => Some(Self::Esptool),
            "nrfutil" | "adafruit-nrfutil" => Some(Self::Nrfutil),
            _ => None,
        }
    }
}

/// Probe critical desktop runtime (must pass before UI).
pub fn critical_status() -> CriticalStatus {
    #[cfg(target_os = "linux")]
    {
        linux_critical_status()
    }
    #[cfg(target_os = "windows")]
    {
        windows_critical_status()
    }
    #[cfg(target_os = "macos")]
    {
        CriticalStatus {
            ok: true,
            detail: "macOS uses system WebKit.".into(),
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        CriticalStatus {
            ok: true,
            detail: "Unknown platform; skipping critical runtime probe.".into(),
        }
    }
}

/// Ensure critical runtime is present; attempt install once if missing.
pub fn ensure_critical() -> Result<(), String> {
    let status = critical_status();
    if status.ok {
        return Ok(());
    }
    eprintln!("Metis Mesh HQ: {}", status.detail);
    eprintln!("Metis Mesh HQ: attempting to install required runtime…");
    install_critical()?;
    let again = critical_status();
    if again.ok {
        eprintln!("Metis Mesh HQ: runtime ready.");
        return Ok(());
    }
    Err(format!(
        "Required desktop runtime is still missing after install.\n{}\n\n{}",
        again.detail,
        critical_manual_hint()
    ))
}

fn critical_manual_hint() -> String {
    #[cfg(target_os = "linux")]
    {
        "Install WebKitGTK 4.1 (+ GTK) manually:\n  Ubuntu/Debian: sudo apt-get install -y libwebkit2gtk-4.1-0 libgtk-3-0t64 \\\n    libayatana-appindicator3-1 libsoup-3.0-0 libjavascriptcoregtk-4.1-0 libdbus-1-3\n  Arch/CachyOS: sudo pacman -S webkit2gtk-4.1\n  Fedora: sudo dnf install webkit2gtk4.1\n(AppImage builds use host WebKit — do not rely on a bundled copy.)"
            .into()
    }
    #[cfg(target_os = "windows")]
    {
        "Install the Microsoft Edge WebView2 Runtime from:\n  https://developer.microsoft.com/microsoft-edge/webview2/"
            .into()
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        "See the README runtime section for your platform.".into()
    }
}

fn install_critical() -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        linux_install_critical()
    }
    #[cfg(target_os = "windows")]
    {
        windows_install_webview2()
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        Err("Automatic runtime install is not available on this platform.".into())
    }
}

fn soft_status_cache() -> &'static Mutex<Option<(Instant, SoftDepsReport)>> {
    static CACHE: Mutex<Option<(Instant, SoftDepsReport)>> = Mutex::new(None);
    &CACHE
}

/// Soft deps for the welcome System setup step.
///
/// Cached briefly: welcome re-renders on the app tick, and uncached probes spawn
/// `where` / PowerShell on Windows (console flash + UI stall).
pub fn soft_status() -> SoftDepsReport {
    if let Ok(guard) = soft_status_cache().lock() {
        if let Some((at, report)) = guard.as_ref() {
            if at.elapsed() < Duration::from_secs(4) {
                return report.clone();
            }
        }
    }
    let report = soft_status_uncached();
    if let Ok(mut guard) = soft_status_cache().lock() {
        *guard = Some((Instant::now(), report.clone()));
    }
    report
}

/// Bust the soft-deps cache after an Install attempt.
pub fn invalidate_soft_status() {
    if let Ok(mut guard) = soft_status_cache().lock() {
        *guard = None;
    }
}

fn soft_status_uncached() -> SoftDepsReport {
    let mut items = Vec::new();

    #[cfg(target_os = "linux")]
    {
        let bluez_ok = linux_bluez_ok();
        let bluez_install = !bluez_ok && linux_pkg_mgr().is_some();
        items.push(SoftDepItem {
            kind: SoftDepKind::Bluez,
            label: "Bluetooth (BlueZ)".into(),
            ok: bluez_ok,
            detail: if bluez_ok {
                "bluetoothd available.".into()
            } else {
                "BlueZ / bluetoothd not found.".into()
            },
            can_install: bluez_install,
            hint: bluez_install_hint(bluez_install),
        });

        let serial = linux_serial_access();
        let group = serial.group.unwrap_or("dialout");
        items.push(SoftDepItem {
            kind: SoftDepKind::Dialout,
            label: format!("USB serial ({group} group)"),
            ok: serial.in_session,
            detail: if serial.in_session {
                format!("Current user is in {group}.")
            } else if serial.configured {
                format!(
                    "Added to {group}, but this session is stale — log out and back in before USB serial works."
                )
            } else {
                format!(
                    "Not in {group} — USB companions may fail with permission denied."
                )
            },
            // Offer Install only until membership is written; re-login is separate.
            can_install: !serial.configured,
            hint: if serial.configured && !serial.in_session {
                "Log out and back in (or reboot).".into()
            } else {
                "After install, log out and back in.".into()
            },
        });
    }

    #[cfg(not(target_os = "linux"))]
    {
        items.push(SoftDepItem {
            kind: SoftDepKind::Bluez,
            label: "Bluetooth".into(),
            ok: true,
            detail: "Managed by the OS.".into(),
            can_install: false,
            hint: String::new(),
        });
        items.push(SoftDepItem {
            kind: SoftDepKind::Dialout,
            label: "USB serial access".into(),
            ok: true,
            detail: "Managed by the OS / drivers.".into(),
            can_install: false,
            hint: String::new(),
        });
    }

    let esptool_ok = tool_on_path(&["esptool.py", "esptool"]);
    let esptool_install = !esptool_ok && can_install_flasher_cli(SoftDepKind::Esptool);
    items.push(SoftDepItem {
        kind: SoftDepKind::Esptool,
        label: "esptool (ESP32 flasher)".into(),
        ok: esptool_ok,
        detail: if esptool_ok {
            "Found on PATH.".into()
        } else {
            "Optional — only needed for ESP32 USB flashing.".into()
        },
        can_install: esptool_install,
        hint: flasher_install_hint(SoftDepKind::Esptool, esptool_install),
    });

    let nrf_ok = tool_on_path(&["adafruit-nrfutil", "nrfutil"]);
    let nrf_install = !nrf_ok && can_install_flasher_cli(SoftDepKind::Nrfutil);
    items.push(SoftDepItem {
        kind: SoftDepKind::Nrfutil,
        label: "nRF DFU tools".into(),
        ok: nrf_ok,
        detail: if nrf_ok {
            "Found on PATH.".into()
        } else {
            "Optional — only needed for nRF52 serial DFU.".into()
        },
        can_install: nrf_install,
        hint: flasher_install_hint(SoftDepKind::Nrfutil, nrf_install),
    });

    SoftDepsReport { items }
}

pub fn install_soft(kind: SoftDepKind) -> Result<String, String> {
    match kind {
        SoftDepKind::Bluez => {
            #[cfg(target_os = "linux")]
            {
                match linux_pkg_mgr() {
                    Some(LinuxPkgMgr::Apt) => run_pkexec_apt(&["bluez"])?,
                    Some(LinuxPkgMgr::Pacman) => run_pkexec_pacman(&["bluez"])?,
                    Some(LinuxPkgMgr::Dnf) => run_pkexec_dnf(&["bluez"])?,
                    None => {
                        return Err(
                            "Automatic BlueZ install needs apt, pacman, or dnf. Install manually: sudo apt-get install -y bluez   OR   sudo pacman -S bluez   OR   sudo dnf install bluez"
                                .into(),
                        );
                    }
                }
                Ok("BlueZ installed. You may need to enable Bluetooth in system settings.".into())
            }
            #[cfg(not(target_os = "linux"))]
            {
                Err("BlueZ install is Linux-only.".into())
            }
        }
        SoftDepKind::Dialout => {
            #[cfg(target_os = "linux")]
            {
                linux_add_dialout()
            }
            #[cfg(not(target_os = "linux"))]
            {
                Err("dialout group is Linux-only.".into())
            }
        }
        SoftDepKind::Esptool => install_esptool(),
        SoftDepKind::Nrfutil => install_nrfutil(),
    }
}

#[cfg(target_os = "linux")]
fn bluez_hint_for(mgr: Option<LinuxPkgMgr>, can_install: bool) -> String {
    if can_install {
        match mgr {
            Some(LinuxPkgMgr::Apt) => {
                "Needed for BLE companions. Install via apt (bluez).".into()
            }
            Some(LinuxPkgMgr::Pacman) => {
                "Needed for BLE companions. Install via pacman (bluez).".into()
            }
            Some(LinuxPkgMgr::Dnf) => {
                "Needed for BLE companions. Install via dnf (bluez).".into()
            }
            None => "Needed for BLE companions.".into(),
        }
    } else {
        match mgr {
            Some(LinuxPkgMgr::Apt) => {
                "Needed for BLE companions. Install manually: sudo apt-get install -y bluez".into()
            }
            Some(LinuxPkgMgr::Pacman) => {
                "Needed for BLE companions. Install manually: sudo pacman -S bluez".into()
            }
            Some(LinuxPkgMgr::Dnf) => {
                "Needed for BLE companions. Install manually: sudo dnf install bluez".into()
            }
            None => {
                "Needed for BLE companions. Install bluez via your package manager (apt / pacman / dnf)."
                    .into()
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn bluez_install_hint(can_install: bool) -> String {
    bluez_hint_for(linux_pkg_mgr(), can_install)
}

fn flasher_install_hint(kind: SoftDepKind, can_install: bool) -> String {
    match kind {
        SoftDepKind::Esptool => {
            if can_install {
                #[cfg(target_os = "windows")]
                {
                    return "Install via PowerShell (Python + pip if needed).".into();
                }
                #[cfg(target_os = "linux")]
                {
                    return match linux_pkg_mgr() {
                        Some(LinuxPkgMgr::Apt) => "Install via apt (or pipx).".into(),
                        Some(LinuxPkgMgr::Pacman) => "Install via pacman (esptool).".into(),
                        Some(LinuxPkgMgr::Dnf) => "Install via dnf (esptool).".into(),
                        None => "Install via pipx / pip.".into(),
                    };
                }
                #[cfg(not(any(target_os = "windows", target_os = "linux")))]
                {
                    return "Install via pipx / pip.".into();
                }
            }
            #[cfg(target_os = "windows")]
            {
                "Install manually: winget install Python.Python.3.12 ; python -m pip install --user esptool"
                    .into()
            }
            #[cfg(target_os = "linux")]
            {
                match linux_pkg_mgr() {
                    Some(LinuxPkgMgr::Apt) => {
                        "Install manually: sudo apt-get install -y esptool   (or pipx install esptool)"
                            .into()
                    }
                    Some(LinuxPkgMgr::Pacman) => {
                        "Install manually: sudo pacman -S esptool".into()
                    }
                    Some(LinuxPkgMgr::Dnf) => "Install manually: sudo dnf install esptool".into(),
                    None => "Install manually: pipx install esptool".into(),
                }
            }
            #[cfg(not(any(target_os = "windows", target_os = "linux")))]
            {
                "Install manually: pipx install esptool".into()
            }
        }
        SoftDepKind::Nrfutil => {
            if can_install {
                #[cfg(target_os = "windows")]
                {
                    return "Install adafruit-nrfutil via PowerShell (Python + pip if needed).".into();
                }
                #[cfg(target_os = "linux")]
                {
                    return match linux_pkg_mgr() {
                        Some(LinuxPkgMgr::Apt) => {
                            "Install adafruit-nrfutil via apt pipx (or pipx alone).".into()
                        }
                        Some(LinuxPkgMgr::Pacman) => {
                            "Install python-pipx via pacman, then adafruit-nrfutil via pipx.".into()
                        }
                        Some(LinuxPkgMgr::Dnf) => {
                            "Install pipx via dnf, then adafruit-nrfutil via pipx.".into()
                        }
                        None => "Install adafruit-nrfutil via pipx.".into(),
                    };
                }
                #[cfg(not(any(target_os = "windows", target_os = "linux")))]
                {
                    return "Install adafruit-nrfutil via pipx.".into();
                }
            }
            #[cfg(target_os = "windows")]
            {
                "Install manually: python -m pip install --user adafruit-nrfutil".into()
            }
            #[cfg(target_os = "linux")]
            {
                match linux_pkg_mgr() {
                    Some(LinuxPkgMgr::Pacman) => {
                        "Install manually: sudo pacman -S python-pipx && pipx install adafruit-nrfutil"
                            .into()
                    }
                    Some(LinuxPkgMgr::Dnf) => {
                        "Install manually: sudo dnf install pipx && pipx install adafruit-nrfutil"
                            .into()
                    }
                    _ => "Install manually: pipx install adafruit-nrfutil".into(),
                }
            }
            #[cfg(not(any(target_os = "windows", target_os = "linux")))]
            {
                "Install manually: pipx install adafruit-nrfutil".into()
            }
        }
        SoftDepKind::Bluez | SoftDepKind::Dialout => String::new(),
    }
}

fn can_install_flasher_cli(kind: SoftDepKind) -> bool {
    match kind {
        SoftDepKind::Esptool | SoftDepKind::Nrfutil => {
            #[cfg(target_os = "windows")]
            {
                // PowerShell can fetch Python via winget, then pip-install the tool.
                return windows_powershell_available();
            }
            #[cfg(target_os = "linux")]
            {
                // apt / pacman / dnf can fetch the package or pipx; otherwise need pipx/pip already.
                if linux_pkg_mgr().is_some() {
                    return true;
                }
                return python_cli_installer_available();
            }
            #[cfg(not(any(target_os = "windows", target_os = "linux")))]
            {
                python_cli_installer_available()
            }
        }
        SoftDepKind::Bluez | SoftDepKind::Dialout => false,
    }
}

#[cfg(not(target_os = "windows"))]
fn python_cli_installer_available() -> bool {
    tool_on_path(&["pipx"])
        || std::path::Path::new("/usr/bin/pipx").is_file()
        || python_pip_module_ok()
}

#[cfg(not(target_os = "windows"))]
fn python_commands() -> &'static [&'static str] {
    // macOS/Linux prefer python3; keep `python` as a fallback.
    &["python3", "python"]
}

#[cfg(not(target_os = "windows"))]
fn python_pip_module_ok() -> bool {
    python_commands().iter().any(|py| {
        command(py)
            .args(["-m", "pip", "--version"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
}

#[cfg(not(target_os = "windows"))]
fn run_pip_user_install(pip_name: &str) -> Result<(bool, String), String> {
    let mut last_err = String::new();
    for py in python_commands() {
        let mut args = vec![
            "-m".into(),
            "pip".into(),
            "install".into(),
            "--user".into(),
            pip_name.to_string(),
        ];
        match command(py).args(&args).output() {
            Ok(output) if output.status.success() => return Ok((true, String::new())),
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                if stderr.contains("externally-managed-environment") {
                    args.push("--break-system-packages".into());
                    match command(py).args(&args).output() {
                        Ok(retry) if retry.status.success() => return Ok((true, String::new())),
                        Ok(retry) => {
                            last_err = String::from_utf8_lossy(&retry.stderr).to_string();
                        }
                        Err(e) => last_err = format!("Could not run {py} -m pip retry: {e}"),
                    }
                } else {
                    last_err = stderr;
                }
            }
            Err(e) => last_err = format!("Could not run {py} -m pip: {e}"),
        }
    }
    Ok((false, last_err))
}

fn install_esptool() -> Result<String, String> {
    #[cfg(target_os = "windows")]
    {
        return windows_install_python_package("esptool", &["esptool.py", "esptool"]);
    }
    #[cfg(target_os = "linux")]
    {
        match linux_pkg_mgr() {
            Some(LinuxPkgMgr::Apt) => {
                run_pkexec_apt(&["esptool"])?;
                if tool_on_path(&["esptool.py", "esptool"]) {
                    return Ok("esptool installed via apt.".into());
                }
                return Ok(
                    "apt finished installing esptool. Click Refresh if status is still pending."
                        .into(),
                );
            }
            Some(LinuxPkgMgr::Pacman) => {
                run_pkexec_pacman(&["esptool"])?;
                if tool_on_path(&["esptool.py", "esptool"]) {
                    return Ok("esptool installed via pacman.".into());
                }
                return Ok(
                    "pacman finished installing esptool. Click Refresh if status is still pending."
                        .into(),
                );
            }
            Some(LinuxPkgMgr::Dnf) => {
                run_pkexec_dnf(&["esptool"])?;
                if tool_on_path(&["esptool.py", "esptool"]) {
                    return Ok("esptool installed via dnf.".into());
                }
                return Ok(
                    "dnf finished installing esptool. Click Refresh if status is still pending."
                        .into(),
                );
            }
            None => {}
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        install_python_cli("esptool", &["esptool.py", "esptool"])
    }
}

fn install_nrfutil() -> Result<String, String> {
    #[cfg(target_os = "windows")]
    {
        windows_install_python_package("adafruit-nrfutil", &["adafruit-nrfutil", "nrfutil"])
    }
    #[cfg(not(target_os = "windows"))]
    {
        install_python_cli("adafruit-nrfutil", &["adafruit-nrfutil", "nrfutil"])
    }
}

#[cfg(not(target_os = "windows"))]
fn install_python_cli(pip_name: &str, binaries: &[&str]) -> Result<String, String> {
    // Prefer pipx on Ubuntu 26.04+ (avoids PEP 668 externally-managed env).
    if ensure_pipx().is_ok() {
        let output = command(pipx_bin())
            .args(["install", pip_name])
            .output()
            .map_err(|e| format!("Could not run pipx: {e}"))?;
        if output.status.success() || tool_on_path(binaries) {
            return Ok(format!(
                "{pip_name} installed via pipx. Tools live in ~/.local/bin — click Refresh."
            ));
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        // "already installed" still counts as success for us.
        let combined = format!("{stdout}\n{stderr}").to_ascii_lowercase();
        if combined.contains("already installed") || tool_on_path(binaries) {
            return Ok(format!("{pip_name} is already available via pipx.").into());
        }
        return Err(format!(
            "pipx install {pip_name} failed.\n{}\n{}",
            stdout.trim(),
            stderr.trim()
        ));
    }

    let (ok, err) = run_pip_user_install(pip_name)?;
    if ok || tool_on_path(binaries) {
        return Ok(format!(
            "{pip_name} installed with pip --user. Click Refresh (ensure user Scripts / ~/.local/bin is on PATH)."
        ));
    }
    if err.contains("externally-managed-environment") {
        return Err(format!(
            "pip is blocked (externally managed). Install pipx, then: pipx install {pip_name}\n{}",
            err.trim()
        ));
    }
    Err(format!("pip install {pip_name} failed.\n{}", err.trim()))
}

#[cfg(not(target_os = "windows"))]
fn pipx_bin() -> &'static str {
    if std::path::Path::new("/usr/bin/pipx").is_file() {
        "/usr/bin/pipx"
    } else {
        "pipx"
    }
}

#[cfg(not(target_os = "windows"))]
fn ensure_pipx() -> Result<(), String> {
    if tool_on_path(&["pipx"]) || std::path::Path::new("/usr/bin/pipx").is_file() {
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    {
        match linux_pkg_mgr() {
            Some(LinuxPkgMgr::Apt) => {
                run_pkexec_apt(&["pipx"])?;
            }
            Some(LinuxPkgMgr::Pacman) => {
                // Arch/CachyOS package name.
                run_pkexec_pacman(&["python-pipx"])?;
            }
            Some(LinuxPkgMgr::Dnf) => {
                run_pkexec_dnf(&["pipx"])?;
            }
            None => return Err("pipx is not available.".into()),
        }
        if tool_on_path(&["pipx"]) || std::path::Path::new("/usr/bin/pipx").is_file() {
            return Ok(());
        }
        return Err(
            "pipx package install finished but pipx is still missing from PATH.".into(),
        );
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err("pipx is not available.".into())
    }
}

fn tool_on_path(names: &[&str]) -> bool {
    for name in names {
        if let Ok(output) = command("which").arg(name).output() {
            if output.status.success() {
                let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path.is_empty() {
                    return true;
                }
            }
        }
        if let Ok(output) = command("where").arg(name).output() {
            if output.status.success() {
                let path = String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if !path.is_empty() {
                    return true;
                }
            }
        }
        // pipx / pip --user often land here even when the desktop session PATH omitted them.
        for dir in user_tool_dirs() {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return true;
            }
            #[cfg(windows)]
            {
                let exe = dir.join(format!("{name}.exe"));
                if exe.is_file() {
                    return true;
                }
            }
        }
    }
    false
}

fn user_tool_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    if let Some(home) = home {
        let home = std::path::PathBuf::from(home);
        dirs.push(home.join(".local").join("bin"));
        #[cfg(windows)]
        {
            if let Ok(appdata) = std::env::var("APPDATA") {
                let python_root = std::path::PathBuf::from(appdata).join("Python");
                if let Ok(entries) = std::fs::read_dir(&python_root) {
                    for entry in entries.flatten() {
                        let scripts = entry.path().join("Scripts");
                        if scripts.is_dir() {
                            dirs.push(scripts);
                        }
                    }
                }
            }
        }
    }
    dirs
}

#[cfg(target_os = "linux")]
fn linux_critical_packages() -> &'static [&'static str] {
    // Ubuntu 26.04+ package names (GTK uses the time_t64 ABI package).
    &[
        "libwebkit2gtk-4.1-0",
        "libgtk-3-0t64",
        "libayatana-appindicator3-1",
        "libsoup-3.0-0",
        "libjavascriptcoregtk-4.1-0",
        "libdbus-1-3",
    ]
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LinuxPkgMgr {
    Apt,
    Pacman,
    Dnf,
}

#[cfg(target_os = "linux")]
fn linux_is_apt() -> bool {
    matches!(linux_pkg_mgr(), Some(LinuxPkgMgr::Apt))
}

#[cfg(target_os = "linux")]
fn linux_pkg_mgr() -> Option<LinuxPkgMgr> {
    // Prefer the native manager for the running distro. apt first so
    // Debian/Ubuntu derivatives that also ship dnf-compat tools stay on apt.
    if std::path::Path::new("/usr/bin/apt-get").is_file()
        || std::path::Path::new("/bin/apt-get").is_file()
    {
        return Some(LinuxPkgMgr::Apt);
    }
    if std::path::Path::new("/usr/bin/pacman").is_file() {
        return Some(LinuxPkgMgr::Pacman);
    }
    if std::path::Path::new("/usr/bin/dnf").is_file()
        || std::path::Path::new("/usr/bin/dnf5").is_file()
    {
        return Some(LinuxPkgMgr::Dnf);
    }
    None
}

#[cfg(target_os = "linux")]
fn linux_pkexec_bin() -> &'static str {
    if std::path::Path::new("/usr/bin/pkexec").is_file() {
        "/usr/bin/pkexec"
    } else {
        "pkexec"
    }
}

#[cfg(target_os = "linux")]
fn run_pkexec_pacman(packages: &[&str]) -> Result<(), String> {
    if packages.is_empty() {
        return Ok(());
    }
    let mut args = vec![
        "pacman".to_string(),
        "-S".into(),
        "--needed".into(),
        "--noconfirm".into(),
    ];
    args.extend(packages.iter().map(|s| (*s).to_string()));
    let output = command(linux_pkexec_bin())
        .args(&args)
        .output()
        .map_err(|e| {
            format!(
                "Could not run pkexec pacman ({e}). Install manually: sudo pacman -S {}",
                packages.join(" ")
            )
        })?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    Err(format!(
        "pacman failed.\n{}\n{}",
        stdout.trim(),
        stderr.trim()
    ))
}

#[cfg(target_os = "linux")]
fn run_pkexec_dnf(packages: &[&str]) -> Result<(), String> {
    if packages.is_empty() {
        return Ok(());
    }
    let dnf = if std::path::Path::new("/usr/bin/dnf5").is_file() {
        "dnf5"
    } else {
        "dnf"
    };
    let mut args = vec![dnf.to_string(), "install".into(), "-y".into()];
    args.extend(packages.iter().map(|s| (*s).to_string()));
    let output = command(linux_pkexec_bin())
        .args(&args)
        .output()
        .map_err(|e| {
            format!(
                "Could not run pkexec {dnf} ({e}). Install manually: sudo {dnf} install {}",
                packages.join(" ")
            )
        })?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    Err(format!(
        "{dnf} failed.\n{}\n{}",
        stdout.trim(),
        stderr.trim()
    ))
}

#[cfg(target_os = "linux")]
fn linux_package_installed(name: &str) -> bool {
    command("dpkg-query")
        .args(["-W", "-f=${Status}", name])
        .output()
        .ok()
        .and_then(|o| {
            if !o.status.success() {
                return None;
            }
            let s = String::from_utf8_lossy(&o.stdout);
            Some(s.contains("install ok installed"))
        })
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn linux_critical_status() -> CriticalStatus {
    if !linux_is_apt() {
        // Non-Debian: assume OK if we got this far; wry will fail loudly otherwise.
        return CriticalStatus {
            ok: true,
            detail: "Non-apt Linux; skipping package probe.".into(),
        };
    }
    let missing: Vec<&str> = linux_critical_packages()
        .iter()
        .copied()
        .filter(|pkg| !linux_package_installed(pkg))
        .collect();
    if missing.is_empty() {
        CriticalStatus {
            ok: true,
            detail: "WebKitGTK / GTK packages present.".into(),
        }
    } else {
        CriticalStatus {
            ok: false,
            detail: format!("Missing packages: {}", missing.join(", ")),
        }
    }
}

#[cfg(target_os = "linux")]
fn linux_install_critical() -> Result<(), String> {
    if !linux_is_apt() {
        return Err("Automatic install requires apt (Debian/Ubuntu).".into());
    }
    run_pkexec_apt(linux_critical_packages())
}

#[cfg(target_os = "linux")]
fn run_pkexec_apt(packages: &[&str]) -> Result<(), String> {
    if packages.is_empty() {
        return Ok(());
    }
    let mut args = vec![
        "apt-get".to_string(),
        "install".into(),
        "-y".into(),
    ];
    args.extend(packages.iter().map(|s| (*s).to_string()));
    let output = command(linux_pkexec_bin())
        .args(&args)
        .output()
        .map_err(|e| {
            format!(
                "Could not run pkexec apt-get ({e}). Install manually: sudo apt-get install -y {}",
                packages.join(" ")
            )
        })?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    Err(format!(
        "apt-get failed.\n{}\n{}",
        stdout.trim(),
        stderr.trim()
    ))
}

#[cfg(target_os = "linux")]
fn linux_bluez_ok() -> bool {
    linux_package_installed("bluez")
        || std::path::Path::new("/usr/lib/bluetooth/bluetoothd").is_file()
        || std::path::Path::new("/usr/sbin/bluetoothd").is_file()
        || tool_on_path(&["bluetoothctl"])
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct LinuxSerialAccess {
    /// Debian/Ubuntu: dialout. Arch/CachyOS/Fedora: usually uucp.
    group: Option<&'static str>,
    /// True when `/etc/group` (getent) lists the user — survives usermod without re-login.
    configured: bool,
    /// True when the current process already has the group (needed for open() on /dev/tty*).
    in_session: bool,
}

#[cfg(target_os = "linux")]
fn linux_group_exists(group: &str) -> bool {
    command("getent")
        .args(["group", group])
        .output()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false)
}

/// Prefer dialout when present; Arch-family serial devices use uucp.
#[cfg(target_os = "linux")]
fn linux_serial_group() -> Option<&'static str> {
    if linux_group_exists("dialout") {
        Some("dialout")
    } else if linux_group_exists("uucp") {
        Some("uucp")
    } else {
        None
    }
}

#[cfg(target_os = "linux")]
fn linux_current_username() -> Result<String, String> {
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .map_err(|_| "Could not determine current username.".to_string())?;
    if user.trim().is_empty() || user.contains('/') || user.contains(' ') {
        return Err("Refusing to modify groups for an invalid username.".into());
    }
    Ok(user)
}

#[cfg(target_os = "linux")]
fn linux_user_in_group_session(group: &str) -> bool {
    command("id")
        .args(["-nG"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .split_whitespace()
                .any(|g| g == group)
        })
        .unwrap_or(false)
}

/// Parse `getent group` output (`name:x:gid:user1,user2`).
#[cfg(target_os = "linux")]
fn linux_getent_group_has_user(line: &str, user: &str) -> bool {
    let Some(members) = line.trim().rsplit_once(':').map(|(_, m)| m) else {
        return false;
    };
    members.split(',').any(|m| m.trim() == user)
}

#[cfg(target_os = "linux")]
fn linux_user_in_group_configured(user: &str, group: &str) -> bool {
    let Ok(output) = command("getent").args(["group", group]).output() else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    linux_getent_group_has_user(&String::from_utf8_lossy(&output.stdout), user)
}

#[cfg(target_os = "linux")]
fn linux_serial_access() -> LinuxSerialAccess {
    let group = linux_serial_group();
    let user = linux_current_username().ok();
    let Some(group) = group else {
        return LinuxSerialAccess {
            group: None,
            configured: false,
            in_session: false,
        };
    };
    let in_session = linux_user_in_group_session(group);
    let configured = user
        .as_deref()
        .map(|u| linux_user_in_group_configured(u, group))
        .unwrap_or(false)
        || in_session;
    LinuxSerialAccess {
        group: Some(group),
        configured,
        in_session,
    }
}

#[cfg(target_os = "linux")]
fn linux_add_dialout() -> Result<String, String> {
    let user = linux_current_username()?;
    let group = linux_serial_group().ok_or_else(|| {
        "No serial access group found (expected dialout or uucp). Create one or fix udev rules for /dev/ttyUSB* / /dev/ttyACM*."
            .to_string()
    })?;
    if linux_user_in_group_configured(&user, group) {
        return Ok(format!(
            "{user} is already in {group}. Log out and back in (or reboot) so this session picks up the group."
        ));
    }
    let output = command(linux_pkexec_bin())
        .args(["usermod", "-aG", group, &user])
        .output()
        .map_err(|e| format!("Could not run pkexec usermod: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "usermod failed: {}\nManual fix: sudo usermod -aG {group} {user}",
            stderr.trim()
        ));
    }
    Ok(format!(
        "Added {user} to {group}. Log out and back in (or reboot) before using USB serial."
    ))
}

#[cfg(target_os = "windows")]
fn windows_powershell_available() -> bool {
    command("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", "$PSVersionTable.PSVersion.Major"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(target_os = "windows")]
fn windows_run_powershell(script: &str) -> Result<String, String> {
    let output = command("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ])
        .output()
        .map_err(|e| format!("Could not start PowerShell: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if output.status.success() {
        return Ok(if stdout.is_empty() {
            stderr
        } else {
            stdout
        });
    }
    Err(format!(
        "PowerShell failed.\n{}\n{}",
        stdout,
        stderr
    ))
}

#[cfg(target_os = "windows")]
fn windows_critical_status() -> CriticalStatus {
    if windows_webview2_present() {
        CriticalStatus {
            ok: true,
            detail: "WebView2 runtime present.".into(),
        }
    } else {
        CriticalStatus {
            ok: false,
            detail: "Microsoft Edge WebView2 Runtime not found.".into(),
        }
    }
}

#[cfg(target_os = "windows")]
fn windows_webview2_present() -> bool {
    // Evergreen runtime leaves a registry value under these keys.
    const KEYS: &[&str] = &[
        r"SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}",
        r"SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}",
    ];
    for key in KEYS {
        if let Ok(output) = command("reg")
            .args(["query", &format!("HKLM\\{key}"), "/v", "pv"])
            .output()
        {
            if output.status.success() {
                let text = String::from_utf8_lossy(&output.stdout);
                if text.to_ascii_lowercase().contains("pv") {
                    return true;
                }
            }
        }
    }
    // Fallback: msedgewebview2.exe on PATH / Program Files.
    let candidates = [
        r"C:\Program Files (x86)\Microsoft\EdgeWebView\Application",
        r"C:\Program Files\Microsoft\EdgeWebView\Application",
    ];
    for dir in candidates {
        if std::path::Path::new(dir).is_dir() {
            return true;
        }
    }
    false
}

#[cfg(target_os = "windows")]
fn windows_install_webview2() -> Result<(), String> {
    if !windows_powershell_available() {
        return Err(
            "PowerShell is required to download WebView2. Install WebView2 manually from Microsoft."
                .into(),
        );
    }
    eprintln!("Metis Mesh HQ: downloading WebView2 via PowerShell…");
    // Evergreen Standalone Bootstrapper (Microsoft).
    let script = r#"
$ErrorActionPreference = 'Stop'
$url = 'https://go.microsoft.com/fwlink/p/?LinkId=2124703'
$dest = Join-Path $env:TEMP 'MicrosoftEdgeWebview2Setup.exe'
Write-Output 'Downloading WebView2 bootstrapper…'
Invoke-WebRequest -Uri $url -OutFile $dest -UseBasicParsing
Write-Output 'Running WebView2 installer…'
$p = Start-Process -FilePath $dest -ArgumentList '/silent','/install' -Wait -PassThru
Remove-Item -LiteralPath $dest -Force -ErrorAction SilentlyContinue
if ($null -ne $p.ExitCode -and $p.ExitCode -ne 0) {
  throw "WebView2 installer exited with code $($p.ExitCode)"
}
Write-Output 'WebView2 installer finished.'
"#;
    windows_run_powershell(script)?;
    if windows_webview2_present() {
        return Ok(());
    }
    Err(
        "WebView2 installer finished but the runtime was not detected. Install manually from Microsoft."
            .into(),
    )
}

/// Install a PyPI flasher CLI on Windows via PowerShell.
/// Ensures Python (winget) when missing, then `pip install --user`.
#[cfg(target_os = "windows")]
fn windows_install_python_package(pip_name: &str, binaries: &[&str]) -> Result<String, String> {
    // Only known package names from SoftDepKind reach here.
    if !pip_name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("Refusing to install an unexpected package name.".into());
    }
    if !windows_powershell_available() {
        return Err(format!(
            "PowerShell is required to install {pip_name}. Install Python and run: python -m pip install --user {pip_name}"
        ));
    }
    let script = format!(
        r#"
$ErrorActionPreference = 'Stop'
function Find-Python {{
  foreach ($c in @('py', 'python', 'python3')) {{
    $cmd = Get-Command $c -ErrorAction SilentlyContinue
    if ($cmd) {{ return $cmd.Source }}
  }}
  return $null
}}
function Refresh-Path {{
  $machine = [System.Environment]::GetEnvironmentVariable('Path','Machine')
  $user = [System.Environment]::GetEnvironmentVariable('Path','User')
  $env:Path = @($machine, $user) -join ';'
}}
$py = Find-Python
if (-not $py) {{
  $winget = Get-Command winget -ErrorAction SilentlyContinue
  if (-not $winget) {{
    throw 'Python was not found and winget is unavailable. Install Python 3 from python.org, then retry.'
  }}
  Write-Output 'Installing Python via winget…'
  & winget install -e --id Python.Python.3.12 --silent --accept-package-agreements --accept-source-agreements
  if ($LASTEXITCODE -ne 0 -and $LASTEXITCODE -ne -1978335189) {{
    # -1978335189 = already installed
    throw "winget failed to install Python (exit $LASTEXITCODE)"
  }}
  Refresh-Path
  $py = Find-Python
  if (-not $py) {{
    throw 'Python still not on PATH after winget. Close and reopen Metis Mesh HQ, then retry Install.'
  }}
}}
Write-Output ("Using Python: " + $py)
Write-Output 'Installing {pip_name} with pip --user…'
& $py -m pip install --user {pip_name}
if ($LASTEXITCODE -ne 0) {{
  throw "pip install {pip_name} failed (exit $LASTEXITCODE)"
}}
Write-Output '{pip_name} installed.'
"#
    );
    let out = windows_run_powershell(&script)?;
    if tool_on_path(binaries) {
        return Ok(format!(
            "{pip_name} installed via PowerShell. Click Refresh if status is still pending."
        ));
    }
    Ok(format!(
        "{pip_name} installed via PowerShell. If Refresh still shows missing, reopen HQ so PATH picks up Python Scripts.\n{out}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soft_status_returns_items() {
        let report = soft_status();
        assert!(report.items.len() >= 2);
    }

    #[test]
    fn soft_kind_roundtrip() {
        assert_eq!(SoftDepKind::parse("bluez"), Some(SoftDepKind::Bluez));
        assert_eq!(SoftDepKind::Dialout.as_str(), "dialout");
    }

    #[test]
    fn critical_status_does_not_panic() {
        let _ = critical_status();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn getent_group_member_parse() {
        assert!(linux_getent_group_has_user("uucp:x:14:alice,bob", "alice"));
        assert!(linux_getent_group_has_user("dialout:x:20:bob", "bob"));
        assert!(!linux_getent_group_has_user("uucp:x:14:alice,bob", "carol"));
        assert!(!linux_getent_group_has_user("uucp:x:14:", "alice"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn bluez_hints_are_distro_correct() {
        let apt_install = bluez_hint_for(Some(LinuxPkgMgr::Apt), true);
        assert!(apt_install.contains("apt"), "{apt_install}");
        assert!(!apt_install.contains("pacman"), "{apt_install}");

        let pacman_manual = bluez_hint_for(Some(LinuxPkgMgr::Pacman), false);
        assert!(pacman_manual.contains("pacman"), "{pacman_manual}");
        assert!(!pacman_manual.contains("apt-get"), "{pacman_manual}");

        let dnf_install = bluez_hint_for(Some(LinuxPkgMgr::Dnf), true);
        assert!(dnf_install.contains("dnf"), "{dnf_install}");
        assert!(!dnf_install.contains("apt"), "{dnf_install}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_critical_packages_use_gtk_t64() {
        assert!(
            linux_critical_packages().contains(&"libgtk-3-0t64"),
            "Ubuntu 26.04 ships GTK as libgtk-3-0t64"
        );
        assert!(
            !linux_critical_packages().contains(&"libgtk-3-0"),
            "do not probe the pre-t64 package name"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn critical_status_ok_when_runtime_present() {
        // Developer / CI machines with WebKitGTK installed should pass.
        let status = critical_status();
        assert!(
            status.ok,
            "expected critical runtime present, got: {}",
            status.detail
        );
    }
}
