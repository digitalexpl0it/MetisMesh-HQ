//! USB serial companion discovery and stable path helpers.

use std::path::{Path, PathBuf};

/// One USB (or USB-CDC) serial port that may host a MeshCore companion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsbPort {
    /// Preferred open path (`by-id` when available).
    pub path: String,
    /// Human label for Settings (product / by-id leaf / basename).
    pub label: String,
    /// Raw system port name (e.g. `/dev/ttyACM0`).
    pub system_path: String,
}

/// MeshCore companion serial default.
pub const DEFAULT_BAUD: u32 = 115_200;

/// List USB serial ports.
///
/// - Linux: prefer stable `/dev/serial/by-id/…` when present
/// - macOS: prefer `/dev/cu.*` (callout) over `/dev/tty.*` call-in twins
/// - Windows: `COMn` paths from the USB CDC driver
pub fn list_usb_ports() -> Result<Vec<UsbPort>, String> {
    let ports = serialport::available_ports().map_err(|err| err.to_string())?;
    let mut out = Vec::new();
    for port in ports {
        if !is_usb_serial(&port) {
            continue;
        }
        // macOS exposes both tty.* and cu.* for the same USB CDC device; open cu.*.
        if cfg!(target_os = "macos") && is_macos_tty_twin(&port.port_name) {
            continue;
        }
        let system_path = port.port_name.clone();
        let path = prefer_stable_path(&system_path).unwrap_or_else(|| system_path.clone());
        let label = port_label(&port, &path);
        out.push(UsbPort {
            path,
            label,
            system_path,
        });
    }
    out.sort_by(|a, b| a.label.to_ascii_lowercase().cmp(&b.label.to_ascii_lowercase()));
    out.dedup_by(|a, b| a.path == b.path);
    Ok(out)
}

fn is_usb_serial(port: &serialport::SerialPortInfo) -> bool {
    use serialport::SerialPortType;
    match &port.port_type {
        SerialPortType::UsbPort(_) => true,
        SerialPortType::PciPort | SerialPortType::BluetoothPort | SerialPortType::Unknown => {
            // Some CDC devices only show as ttyACM/ttyUSB/COM without USB metadata.
            let name = port.port_name.to_ascii_lowercase();
            name.contains("ttyacm")
                || name.contains("ttyusb")
                || name.contains("cu.usb")
                || name.contains("usbmodem")
                || name.contains("usbserial")
                || looks_like_windows_com(&name)
        }
    }
}

fn looks_like_windows_com(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    let trimmed = lower.trim_start_matches(r"\\.\");
    let Some(rest) = trimmed.strip_prefix("com") else {
        return false;
    };
    !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit())
}

fn is_macos_tty_twin(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains("/tty.usb") || lower.contains("/tty.usbmodem") || lower.contains("/tty.usbserial")
}

/// Prefer a stable open path when the OS provides one (Linux by-id today).
pub fn prefer_by_id_path(system_path: &str) -> Option<String> {
    prefer_stable_path(system_path)
}

fn prefer_stable_path(system_path: &str) -> Option<String> {
    if cfg!(target_os = "linux") {
        prefer_by_id_path_in(Path::new("/dev/serial/by-id"), system_path)
    } else {
        None
    }
}

fn prefer_by_id_path_in(by_id_dir: &Path, system_path: &str) -> Option<String> {
    let target = Path::new(system_path);
    let entries = std::fs::read_dir(by_id_dir).ok()?;
    let mut matches = Vec::new();
    for entry in entries.flatten() {
        let link = entry.path();
        let Ok(resolved) = std::fs::canonicalize(&link) else {
            continue;
        };
        if paths_equal(&resolved, target) {
            matches.push(link);
        }
    }
    if matches.is_empty() {
        return None;
    }
    matches.sort();
    Some(matches[0].to_string_lossy().into_owned())
}

fn paths_equal(a: &Path, b: &Path) -> bool {
    if let (Ok(ca), Ok(cb)) = (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        return ca == cb;
    }
    a == b
}

fn port_label(port: &serialport::SerialPortInfo, preferred_path: &str) -> String {
    use serialport::SerialPortType;
    if let SerialPortType::UsbPort(info) = &port.port_type {
        let product = info.product.as_deref().unwrap_or("").trim();
        let manufacturer = info.manufacturer.as_deref().unwrap_or("").trim();
        if !product.is_empty() && !manufacturer.is_empty() {
            return format!("{manufacturer} {product}");
        }
        if !product.is_empty() {
            return product.to_string();
        }
        if !manufacturer.is_empty() {
            return manufacturer.to_string();
        }
    }
    PathBuf::from(preferred_path)
        .file_name()
        .and_then(|n| n.to_str())
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| preferred_path.to_string())
}

/// Normalize an operator-entered path: expand `by-id` when the basename matches.
pub fn normalize_usb_address(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if let Some(by_id) = prefer_by_id_path(trimmed) {
        return by_id;
    }
    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_empty() {
        assert!(normalize_usb_address("  ").is_empty());
    }

    #[test]
    fn prefer_by_id_absent_is_none() {
        // Nonsensical device path — no by-id match expected on CI.
        assert!(prefer_by_id_path("/dev/does-not-exist-metis-test").is_none());
    }

    #[test]
    fn windows_com_names_are_recognized() {
        assert!(looks_like_windows_com("COM3"));
        assert!(looks_like_windows_com(r"\\.\COM12"));
        assert!(!looks_like_windows_com("COM"));
        assert!(!looks_like_windows_com("ttyACM0"));
    }

    #[test]
    fn macos_tty_twins_are_skipped() {
        assert!(is_macos_tty_twin("/dev/tty.usbmodem14401"));
        assert!(!is_macos_tty_twin("/dev/cu.usbmodem14401"));
    }

    #[test]
    fn prefer_by_id_when_both_paths_given() {
        let root = std::env::temp_dir().join(format!(
            "metis-usb-byid-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let devices = root.join("devices");
        let by_id = root.join("by-id");
        std::fs::create_dir_all(&devices).unwrap();
        std::fs::create_dir_all(&by_id).unwrap();
        let tty = devices.join("ttyACM0");
        std::fs::write(&tty, b"").unwrap();
        let link = by_id.join("usb-MeshCore-Companion-if00");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&tty, &link).unwrap();
        #[cfg(not(unix))]
        {
            let _ = (tty, link);
            let _ = std::fs::remove_dir_all(&root);
            return;
        }

        let preferred = prefer_by_id_path_in(&by_id, tty.to_str().unwrap()).expect("by-id");
        assert_eq!(preferred, link.to_string_lossy());
        // A second symlink sorts later; first sorted by-id wins.
        let link_b = by_id.join("usb-MeshCore-Companion-if01");
        std::os::unix::fs::symlink(&tty, &link_b).unwrap();
        let preferred2 = prefer_by_id_path_in(&by_id, tty.to_str().unwrap()).expect("by-id");
        assert_eq!(preferred2, link.to_string_lossy());
        let _ = std::fs::remove_dir_all(root);
    }
}
