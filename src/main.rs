#[macro_use]
mod app_log;
mod backup;
mod ble_pair;
mod brand;
mod contact_export;
mod contact_qr;
mod db;
mod deps;
mod firmware;
mod map_tiles;
mod mesh_regions;
mod mesh_text;
mod path;
mod pmtiles_path;
mod radio;
mod radio_presets;
mod secrets;
mod services;
mod signup;
mod timefmt;
mod tcp_radio;
mod toast;
mod ui;
mod usb_serial;

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Arc;

use db::Db;
use radio::spawn_radio;
use secrets::Secrets;
use ui::{install, App, AppState};

fn main() {
    let data = data_dir();
    if let Err(err) = std::fs::create_dir_all(&data) {
        eprintln!(
            "unable to create the headquarters data folder {}: {err}",
            data.display()
        );
        std::process::exit(1);
    }
    let db = Db::open(&data.join("metis.db")).unwrap_or_else(|err| {
        eprintln!("unable to open the headquarters database: {err}");
        std::process::exit(1);
    });
    if let Err(err) = db.ensure_defaults() {
        eprintln!("unable to prepare the headquarters database: {err}");
        std::process::exit(1);
    }
    map_tiles::sync_active_path(&db);
    if pmtiles_path::current_pmtiles_path().is_none() {
        if let Ok(Some(path)) = db.setting("pmtiles_path") {
            pmtiles_path::set_pmtiles_path(Some(PathBuf::from(path)));
        }
    }
    let db = Arc::new(db);
    let secrets = Arc::new(Secrets::new(&data));
    brand::init(&data);
    contact_qr::spawn_qr_worker(db.clone(), data.clone());
    let radio = spawn_radio(db.clone(), secrets.clone());
    services::spawn_poller(db.clone(), secrets.clone(), radio.clone());
    firmware::spawn_sync_loop(db.clone());
    install(AppState { db, secrets, radio });
    hq_log!(info, "Metis Mesh HQ started · data {}", data.display());
    if let Err(err) = deps::ensure_critical() {
        eprintln!("{err}");
        std::process::exit(1);
    }
    launch_window();
}

#[cfg(feature = "desktop")]
fn launch_window() {
    dioxus::LaunchBuilder::desktop()
        .with_cfg(desktop_config())
        .launch(App);
}

#[cfg(all(feature = "web", not(feature = "desktop")))]
fn launch_window() {
    dioxus::launch(App);
}

#[cfg(feature = "desktop")]
fn desktop_config() -> dioxus::desktop::Config {
    use dioxus::desktop::{Config, LogicalSize, WindowBuilder};

    // document::Stylesheet injects <link>s only after the first RSX paint, so the
    // WebView briefly shows unstyled markup. Inline the app CSS in the HTML head
    // so layout/theme exist before Dioxus mounts content into #main.
    let boot_head = {
        let mut head = String::from(
            r#"<style>
html, body, #main {
  background-color: #101418;
  color: #f4f4f5;
  color-scheme: dark;
  margin: 0;
  min-height: 100%;
}
</style>
<style>
"#,
        );
        head.push_str(include_str!("../assets/tailwind.css"));
        head.push('\n');
        head.push_str(include_str!("../assets/form.css"));
        head.push_str("\n</style>");
        head
    };
    Config::new()
        .with_menu(None)
        .with_background_color((16, 20, 24, 255))
        .with_custom_head(boot_head)
        .with_disable_context_menu(true)
        .with_custom_protocol("metis", |_id, request| {
            serve_metis_protocol(request)
        })
        .with_window(
            WindowBuilder::new()
                .with_title("Metis Mesh HQ")
                .with_inner_size(LogicalSize::new(1200.0, 800.0))
                .with_min_inner_size(LogicalSize::new(960.0, 640.0)),
        )
        .with_on_window(|window, _dom| {
            // tao draws its own Wayland header ("Dioxus App") under the desktop title bar,
            // and the default menu adds a Window / Edit / Help row. Drop that extra header
            // so the desktop keeps a single title bar.
            #[cfg(target_os = "linux")]
            quiet_linux_titlebar(&window);
        })
}

#[cfg(feature = "desktop")]
fn serve_metis_protocol(
    request: dioxus::desktop::wry::http::Request<Vec<u8>>,
) -> dioxus::desktop::wry::http::Response<Cow<'static, [u8]>> {
    use dioxus::desktop::wry::http::{header, Response, StatusCode};

    let path = request.uri().path();
    let host = request.uri().host().unwrap_or("");

    // Contact QR SVGs from data/qr/{pubkey}.svg (written by the background worker).
    let qr_name = path
        .strip_prefix("/qr/")
        .or_else(|| path.rsplit_once("/qr/").map(|(_, rest)| rest))
        .map(|s| s.trim_start_matches('/'))
        .filter(|s| !s.is_empty());
    if let Some(name) = qr_name {
        if let Some(body) = contact_qr::read_qr_file(name) {
            return Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "image/svg+xml; charset=utf-8")
                .header(header::CONTENT_LENGTH, body.len().to_string())
                .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
                .header(header::CACHE_CONTROL, "public, max-age=86400")
                .body(Cow::Owned(body))
                .unwrap_or_else(|_| Response::new(Cow::Borrowed(b"" as &[u8])));
        }
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Cow::Borrowed(b"qr not found" as &[u8]))
            .unwrap_or_else(|_| Response::new(Cow::Borrowed(b"qr not found" as &[u8])));
    }

    // Whitelabel brand mark from data/brand/.
    let brand_name = path
        .strip_prefix("/brand/")
        .or_else(|| path.rsplit_once("/brand/").map(|(_, rest)| rest))
        .map(|s| s.trim_start_matches('/').split('?').next().unwrap_or(""))
        .filter(|s| !s.is_empty());
    if let Some(name) = brand_name {
        if let Some((body, ctype)) = brand::read_brand_file(name) {
            return Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, ctype)
                .header(header::CONTENT_LENGTH, body.len().to_string())
                .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
                .header(header::CACHE_CONTROL, "public, max-age=60")
                .body(Cow::Owned(body))
                .unwrap_or_else(|_| Response::new(Cow::Borrowed(b"" as &[u8])));
        }
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Cow::Borrowed(b"brand not found" as &[u8]))
            .unwrap_or_else(|_| Response::new(Cow::Borrowed(b"brand not found" as &[u8])));
    }

    // Bundled outline GeoJSON (tiny style JSON references these URLs).
    let geojson_name = path
        .strip_prefix("/geojson/")
        .or_else(|| {
            path.rsplit_once("/geojson/")
                .map(|(_, rest)| rest.trim_start_matches('/'))
        })
        .filter(|s| !s.is_empty());
    if let Some(name) = geojson_name {
        let body: &'static [u8] = match name {
            "world-land" | "world-land.geojson" => {
                include_bytes!("../assets/world-land.geojson").as_slice()
            }
            "admin0-borders" | "admin0-borders.geojson" => {
                include_bytes!("../assets/admin0-borders.geojson").as_slice()
            }
            "admin1-lines" | "admin1-lines.geojson" => {
                include_bytes!("../assets/admin1-lines.geojson").as_slice()
            }
            _ => {
                return Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .body(Cow::Borrowed(b"geojson not found" as &[u8]))
                    .unwrap_or_else(|_| {
                        Response::new(Cow::Borrowed(b"geojson not found" as &[u8]))
                    });
            }
        };
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/geo+json")
            .header(header::CONTENT_LENGTH, body.len().to_string())
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .header(header::CACHE_CONTROL, "public, max-age=86400")
            .body(Cow::Borrowed(body))
            .unwrap_or_else(|_| Response::new(Cow::Borrowed(body)));
    }

    // Paths: /pmtiles/basemap (active) or /pmtiles/{region_id}
    let region_from_path = path
        .strip_prefix("/pmtiles/")
        .or_else(|| {
            path.rsplit_once("/pmtiles/")
                .map(|(_, rest)| rest.trim_start_matches('/'))
        })
        .filter(|s| !s.is_empty());
    let legacy = host == "pmtiles"
        && (path == "/basemap" || path.ends_with("/basemap") || path == "/");
    if region_from_path.is_none() && !legacy {
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Cow::Borrowed(b"not found" as &[u8]))
            .unwrap_or_else(|_| Response::new(Cow::Borrowed(b"not found" as &[u8])));
    }
    let file_path = region_from_path
        .and_then(|id| {
            // Strip cache-buster query fragments if the WebView folds them into path.
            let id = id.split(['?', '#']).next().unwrap_or(id);
            if id == "basemap" {
                pmtiles_path::current_pmtiles_path()
            } else {
                map_tiles::region_pmtiles_path(id)
                    .or_else(pmtiles_path::current_pmtiles_path)
            }
        })
        .or_else(pmtiles_path::current_pmtiles_path);
    let Some(file_path) = file_path else {
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Cow::Borrowed(b"pmtiles not configured" as &[u8]))
            .unwrap_or_else(|_| {
                Response::new(Cow::Borrowed(b"pmtiles not configured" as &[u8]))
            });
    };
    // Seek+read ranges — never load the full archive per tile request (a 300 mi
    // pack is tens of MB; full reads stalled Streets after download).
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = std::fs::File::open(&file_path) else {
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Cow::Borrowed(b"pmtiles missing" as &[u8]))
            .unwrap_or_else(|_| Response::new(Cow::Borrowed(b"pmtiles missing" as &[u8])));
    };
    let Ok(meta) = file.metadata() else {
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Cow::Borrowed(b"pmtiles missing" as &[u8]))
            .unwrap_or_else(|_| Response::new(Cow::Borrowed(b"pmtiles missing" as &[u8])));
    };
    let len = meta.len();
    let range = request
        .headers()
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(parse_byte_range);
    if let Some((start, end)) = range {
        let end = end.min(len.saturating_sub(1));
        if start > end || start >= len {
            return Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(header::CONTENT_RANGE, format!("bytes */{len}"))
                .body(Cow::Borrowed(b"" as &[u8]))
                .unwrap_or_else(|_| Response::new(Cow::Borrowed(b"" as &[u8])));
        }
        let want = (end - start + 1) as usize;
        let mut slice = vec![0u8; want];
        if file.seek(SeekFrom::Start(start)).is_err() || file.read_exact(&mut slice).is_err() {
            return Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Cow::Borrowed(b"pmtiles read failed" as &[u8]))
                .unwrap_or_else(|_| Response::new(Cow::Borrowed(b"pmtiles read failed" as &[u8])));
        }
        return Response::builder()
            .status(StatusCode::PARTIAL_CONTENT)
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .header(header::ACCEPT_RANGES, "bytes")
            .header(
                header::CONTENT_RANGE,
                format!("bytes {start}-{end}/{len}"),
            )
            .header(header::CONTENT_LENGTH, slice.len().to_string())
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .body(Cow::Owned(slice))
            .unwrap_or_else(|_| Response::new(Cow::Borrowed(b"" as &[u8])));
    }
    // Unranged GET (rare for PMTiles) — still stream from disk without a second copy path.
    let mut bytes = Vec::with_capacity(len.min(64 * 1024 * 1024) as usize);
    if file.read_to_end(&mut bytes).is_err() {
        return Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(Cow::Borrowed(b"pmtiles read failed" as &[u8]))
            .unwrap_or_else(|_| Response::new(Cow::Borrowed(b"pmtiles read failed" as &[u8])));
    }
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, bytes.len().to_string())
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(Cow::Owned(bytes))
        .unwrap_or_else(|_| Response::new(Cow::Borrowed(b"" as &[u8])))
}

#[cfg(feature = "desktop")]
fn parse_byte_range(header: &str) -> Option<(u64, u64)> {
    let header = header.strip_prefix("bytes=")?;
    let (start, end) = header.split_once('-')?;
    let start: u64 = start.parse().ok()?;
    let end: u64 = if end.is_empty() {
        u64::MAX
    } else {
        end.parse().ok()?
    };
    Some((start, end))
}

#[cfg(all(feature = "desktop", target_os = "linux"))]
fn quiet_linux_titlebar(window: &dioxus::desktop::tao::window::Window) {
    use dioxus::desktop::tao::platform::unix::WindowExtUnix;
    use gtk::prelude::*;
    let gtk_window = window.gtk_window();
    gtk_window.set_title("Metis Mesh HQ");
    // tao's Wayland header is created with the default title and is not updated
    // when the document title changes, so it sits under the desktop title bar.
    if let Some(titlebar) = gtk_window.titlebar() {
        if let Some(bin) = titlebar.downcast_ref::<gtk::EventBox>() {
            if let Some(child) = bin.child() {
                if let Some(header) = child.downcast_ref::<gtk::HeaderBar>() {
                    header.set_title(Some("Metis Mesh HQ"));
                    header.set_show_close_button(false);
                }
            }
        }
        titlebar.hide();
        titlebar.set_visible(false);
    }
    gtk_window.set_decorated(true);
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_application_prefer_dark_theme(true);
    }
}

/// HQ local store: SQLite, secrets fallback, firmware, map packs, backups, QR.
///
/// Override with `METIS_DATA_DIR`. Default is per-user profile storage:
/// - Linux: `~/.MetisMeshHQ`
/// - macOS: `~/Library/Application Support/MetisMeshHQ`
/// - Windows: `%APPDATA%\MetisMeshHQ`
pub(crate) fn data_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("METIS_DATA_DIR") {
        return PathBuf::from(path);
    }
    default_profile_data_dir().unwrap_or_else(|| PathBuf::from("data"))
}

fn default_profile_data_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let base = std::env::var_os("APPDATA")?;
        return Some(PathBuf::from(base).join("MetisMeshHQ"));
    }
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME")?;
        return Some(
            PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("MetisMeshHQ"),
        );
    }
    #[cfg(target_os = "linux")]
    {
        let home = std::env::var_os("HOME")?;
        return Some(PathBuf::from(home).join(".MetisMeshHQ"));
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        None
    }
}
