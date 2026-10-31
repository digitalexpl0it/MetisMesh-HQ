mod ble_pair;
mod db;
mod path;
mod pmtiles_path;
mod radio;
mod radio_presets;
mod secrets;
mod signup;
mod ui;

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Arc;

use db::Db;
use radio::spawn_radio;
use secrets::Secrets;
use ui::{install, App, AppState};

fn main() {
    let data = data_dir();
    let db = Db::open(&data.join("metis.db")).unwrap_or_else(|err| {
        eprintln!("unable to open the headquarters database: {err}");
        std::process::exit(1);
    });
    if let Err(err) = db.ensure_defaults() {
        eprintln!("unable to prepare the headquarters database: {err}");
        std::process::exit(1);
    }
    if let Ok(Some(path)) = db.setting("pmtiles_path") {
        pmtiles_path::set_pmtiles_path(Some(PathBuf::from(path)));
    }
    let db = Arc::new(db);
    let secrets = Arc::new(Secrets::new(&data));
    let radio = spawn_radio(db.clone(), secrets.clone());
    install(AppState { db, secrets, radio });
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
    if path != "/pmtiles/basemap" && !path.ends_with("/pmtiles/basemap") {
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Cow::Borrowed(b"not found" as &[u8]))
            .unwrap_or_else(|_| Response::new(Cow::Borrowed(b"not found" as &[u8])));
    }
    let Some(file_path) = pmtiles_path::current_pmtiles_path() else {
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Cow::Borrowed(b"pmtiles not configured" as &[u8]))
            .unwrap_or_else(|_| {
                Response::new(Cow::Borrowed(b"pmtiles not configured" as &[u8]))
            });
    };
    let Ok(bytes) = std::fs::read(&file_path) else {
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Cow::Borrowed(b"pmtiles missing" as &[u8]))
            .unwrap_or_else(|_| Response::new(Cow::Borrowed(b"pmtiles missing" as &[u8])));
    };
    let len = bytes.len() as u64;
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
        let slice = bytes[start as usize..=end as usize].to_vec();
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
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, len.to_string())
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

fn data_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("METIS_DATA_DIR") {
        return PathBuf::from(path);
    }
    PathBuf::from("data")
}
