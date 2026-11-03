//! Runtime dependency probe + install helpers for first-run setup.
//!
//! Critical checks (WebKitGTK / WebView2) run before the window opens.
//! Soft checks (BlueZ, dialout, flasher CLIs) run inside the welcome UI.

use std::process::Command;

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
        "Install packages manually (Ubuntu 26.04+):\n  sudo apt-get install -y libwebkit2gtk-4.1-0 libgtk-3-0t64 \\\n    libayatana-appindicator3-1 libsoup-3.0-0 libjavascriptcoregtk-4.1-0 libdbus-1-3"
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

/// Soft deps for the welcome System setup step.
pub fn soft_status() -> SoftDepsReport {
    let mut items = Vec::new();

    #[cfg(target_os = "linux")]
    {
        let bluez_ok = linux_bluez_ok();
        items.push(SoftDepItem {
            kind: SoftDepKind::Bluez,
            label: "Bluetooth (BlueZ)".into(),
            ok: bluez_ok,
            detail: if bluez_ok {
                "bluetoothd available.".into()
            } else {
                "BlueZ / bluetoothd not found.".into()
            },
            can_install: !bluez_ok && linux_is_apt(),
            hint: "Needed for BLE companions.".into(),
        });

        let dialout_ok = linux_in_dialout();
        items.push(SoftDepItem {
            kind: SoftDepKind::Dialout,
            label: "USB serial (dialout group)".into(),
            ok: dialout_ok,
            detail: if dialout_ok {
                "Current user is in dialout.".into()
            } else {
                "Not in dialout — USB companions may fail with permission denied.".into()
            },
            can_install: !dialout_ok,
            hint: "After install, log out and back in.".into(),
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
        hint: if esptool_install {
            "Install via apt (Ubuntu) or pipx.".into()
        } else {
            "Install manually: sudo apt-get install -y esptool   (or pipx install esptool)".into()
        },
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
        hint: if nrf_install {
            "Install adafruit-nrfutil via pipx.".into()
        } else {
            "Install manually: pipx install adafruit-nrfutil".into()
        },
    });

    SoftDepsReport { items }
}

pub fn install_soft(kind: SoftDepKind) -> Result<String, String> {
    match kind {
        SoftDepKind::Bluez => {
            #[cfg(target_os = "linux")]
            {
                if !linux_is_apt() {
                    return Err("Automatic BlueZ install needs apt (Debian/Ubuntu).".into());
                }
                run_pkexec_apt(&["bluez"])?;
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
        SoftDepKind::Nrfutil => install_python_cli("adafruit-nrfutil", &["adafruit-nrfutil", "nrfutil"]),
    }
}

fn can_install_flasher_cli(kind: SoftDepKind) -> bool {
    match kind {
        SoftDepKind::Esptool => {
            #[cfg(target_os = "linux")]
            {
                if linux_is_apt() {
                    return true;
                }
            }
            python_cli_installer_available()
        }
        SoftDepKind::Nrfutil => python_cli_installer_available() || {
            #[cfg(target_os = "linux")]
            {
                // Can apt-install pipx first, then pipx-install the tool.
                linux_is_apt()
            }
            #[cfg(not(target_os = "linux"))]
            {
                false
            }
        },
        SoftDepKind::Bluez | SoftDepKind::Dialout => false,
    }
}

fn python_cli_installer_available() -> bool {
    tool_on_path(&["pipx"])
        || std::path::Path::new("/usr/bin/pipx").is_file()
        || python_pip_module_ok()
}

fn python_pip_module_ok() -> bool {
    let py = if cfg!(windows) { "python" } else { "python3" };
    Command::new(py)
        .args(["-m", "pip", "--version"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn install_esptool() -> Result<String, String> {
    #[cfg(target_os = "linux")]
    {
        if linux_is_apt() {
            run_pkexec_apt(&["esptool"])?;
            if tool_on_path(&["esptool.py", "esptool"]) {
                return Ok("esptool installed via apt.".into());
            }
            return Ok(
                "apt finished installing esptool. Click Refresh if status is still pending.".into(),
            );
        }
    }
    install_python_cli("esptool", &["esptool.py", "esptool"])
}

fn install_python_cli(pip_name: &str, binaries: &[&str]) -> Result<String, String> {
    // Prefer pipx on Ubuntu 26.04+ (avoids PEP 668 externally-managed env).
    if ensure_pipx().is_ok() {
        let output = Command::new(pipx_bin())
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

    let py = if cfg!(windows) { "python" } else { "python3" };
    let mut args = vec![
        "-m".into(),
        "pip".into(),
        "install".into(),
        "--user".into(),
        pip_name.to_string(),
    ];
    let output = Command::new(py)
        .args(&args)
        .output()
        .map_err(|e| format!("Could not run {py} -m pip: {e}"))?;
    if output.status.success() || tool_on_path(binaries) {
        return Ok(format!(
            "{pip_name} installed with pip --user. Click Refresh (ensure user Scripts / ~/.local/bin is on PATH)."
        ));
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("externally-managed-environment") {
        // Last resort on older setups; Ubuntu path should have used pipx above.
        args.push("--break-system-packages".into());
        let retry = Command::new(py)
            .args(&args)
            .output()
            .map_err(|e| format!("Could not run pip retry: {e}"))?;
        if retry.status.success() || tool_on_path(binaries) {
            return Ok(format!("{pip_name} installed with pip --user.").into());
        }
        return Err(format!(
            "pip is blocked (externally managed). Install pipx, then: pipx install {pip_name}\n{}",
            stderr.trim()
        ));
    }
    Err(format!(
        "pip install {pip_name} failed.\n{}",
        stderr.trim()
    ))
}

fn pipx_bin() -> &'static str {
    if std::path::Path::new("/usr/bin/pipx").is_file() {
        "/usr/bin/pipx"
    } else {
        "pipx"
    }
}

fn ensure_pipx() -> Result<(), String> {
    if tool_on_path(&["pipx"]) || std::path::Path::new("/usr/bin/pipx").is_file() {
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    {
        if linux_is_apt() {
            run_pkexec_apt(&["pipx"])?;
            if tool_on_path(&["pipx"]) || std::path::Path::new("/usr/bin/pipx").is_file() {
                return Ok(());
            }
            return Err("pipx install via apt finished but pipx is still missing.".into());
        }
    }
    Err("pipx is not available.".into())
}

fn tool_on_path(names: &[&str]) -> bool {
    for name in names {
        if let Ok(output) = Command::new("which").arg(name).output() {
            if output.status.success() {
                let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path.is_empty() {
                    return true;
                }
            }
        }
        if let Ok(output) = Command::new("where").arg(name).output() {
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
fn linux_is_apt() -> bool {
    std::path::Path::new("/usr/bin/apt-get").is_file()
        || std::path::Path::new("/bin/apt-get").is_file()
}

#[cfg(target_os = "linux")]
fn linux_package_installed(name: &str) -> bool {
    Command::new("dpkg-query")
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
    let pkexec = if std::path::Path::new("/usr/bin/pkexec").is_file() {
        "/usr/bin/pkexec"
    } else {
        "pkexec"
    };
    let mut args = vec![
        "apt-get".to_string(),
        "install".into(),
        "-y".into(),
    ];
    args.extend(packages.iter().map(|s| (*s).to_string()));
    let output = Command::new(pkexec)
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
fn linux_in_dialout() -> bool {
    // Primary: id -nG
    if let Ok(output) = Command::new("id").args(["-nG"]).output() {
        if output.status.success() {
            let groups = String::from_utf8_lossy(&output.stdout);
            if groups.split_whitespace().any(|g| g == "dialout") {
                return true;
            }
        }
    }
    // Fallback: some images use `uucp` instead of dialout for serial.
    if let Ok(output) = Command::new("id").args(["-nG"]).output() {
        if output.status.success() {
            let groups = String::from_utf8_lossy(&output.stdout);
            if groups.split_whitespace().any(|g| g == "uucp") {
                return true;
            }
        }
    }
    false
}

#[cfg(target_os = "linux")]
fn linux_add_dialout() -> Result<String, String> {
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .map_err(|_| "Could not determine current username.".to_string())?;
    if user.trim().is_empty() || user.contains('/') || user.contains(' ') {
        return Err("Refusing to modify groups for an invalid username.".into());
    }
    let pkexec = if std::path::Path::new("/usr/bin/pkexec").is_file() {
        "/usr/bin/pkexec"
    } else {
        "pkexec"
    };
    let output = Command::new(pkexec)
        .args(["usermod", "-aG", "dialout", &user])
        .output()
        .map_err(|e| format!("Could not run pkexec usermod: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("usermod failed: {}", stderr.trim()));
    }
    Ok(format!(
        "Added {user} to dialout. Log out and back in (or reboot) before using USB serial."
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
        if let Ok(output) = Command::new("reg")
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
    // Microsoft Evergreen Standalone Bootstrapper
    const URL: &str =
        "https://go.microsoft.com/fwlink/p/?LinkId=2124703";
    let dest = std::env::temp_dir().join("MicrosoftEdgeWebview2Setup.exe");
    eprintln!("Metis Mesh HQ: downloading WebView2 bootstrapper…");
    let resp = ureq::get(URL)
        .timeout(std::time::Duration::from_secs(180))
        .call()
        .map_err(|e| format!("WebView2 download failed: {e}"))?;
    let mut reader = resp.into_reader();
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut reader, &mut bytes)
        .map_err(|e| format!("WebView2 download read failed: {e}"))?;
    if bytes.len() < 1024 {
        return Err("WebView2 bootstrapper download looked empty.".into());
    }
    std::fs::write(&dest, &bytes).map_err(|e| format!("Could not write bootstrapper: {e}"))?;
    eprintln!("Metis Mesh HQ: running WebView2 installer…");
    let status = Command::new(&dest)
        .args(["/silent", "/install"])
        .status()
        .map_err(|e| format!("Could not run WebView2 installer: {e}"))?;
    let _ = std::fs::remove_file(&dest);
    if status.success() || windows_webview2_present() {
        return Ok(());
    }
    Err(format!(
        "WebView2 installer exited with {status}. Install manually from Microsoft."
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
