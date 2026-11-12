// Hide the console window on Windows release builds (avoids a lingering cmd host).
#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

#[macro_use]
mod app_log;
mod backup;
mod ble_pair;
mod brand;
mod channel_speakers;
mod contact_export;
mod contact_qr;
mod db;
mod deps;
mod firmware;
mod log_file;
mod map_tiles;
mod mesh_regions;
mod mesh_text;
mod messages_appearance;
mod node_kit;
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
    app_log::install_library_bridge();
    install_panic_logger();
    #[cfg(target_os = "windows")]
    set_windows_app_user_model_id();
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
    log_file::init(
        data.join("logs"),
        services::setting_bool(&db, log_file::SETTING_KEY, true),
    );
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

/// Record panics in the app log and write them to disk synchronously: the
/// background log writer may not get to flush before the process exits.
fn install_panic_logger() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let message = app_log::redact(&format!("panic: {info}"));
        log_file::write_line_now(app_log::LogLevel::Error, &message);
        previous(info);
    }));
}

#[cfg(feature = "desktop")]
fn launch_window() {
    // Wayland docks resolve icons from .desktop / hicolor at map time — install
    // before the window is created so the first launch is not a blank placeholder.
    #[cfg(target_os = "linux")]
    ensure_linux_desktop_integration();
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
        // user-scalable=no: keep trackpad pinch off WebView page-zoom so the
        // map can own ctrl+wheel (Linux WebKitGTK / Windows WebView2).
        let mut head = String::from(
            r#"<meta name="viewport" content="width=device-width, initial-scale=1, maximum-scale=1, user-scalable=no" />
<style>
html, body, #main {
  background-color: #101418;
  color: #f4f4f5;
  color-scheme: dark;
  margin: 0;
  min-height: 100%;
  overscroll-behavior: none;
}
</style>
<style>
"#,
        );
        head.push_str(include_str!("../assets/tailwind.css"));
        head.push('\n');
        head.push_str(include_str!("../assets/form.css"));
        head.push_str("\n</style>");
        // Selective context menu: allow native OS spell suggestions on compose
        // fields marked data-allow-context-menu; block WebView chrome elsewhere.
        // (Dioxus's with_disable_context_menu(true) is a blanket preventDefault.)
        // lang=en helps WebView dictionaries pick a language when the OS locale
        // is unset or mismatched.
        head.push_str(
            r#"
<script>
(function () {
  try { document.documentElement.setAttribute('lang', 'en'); } catch (_) {}
  document.addEventListener('contextmenu', function (e) {
    if (e.target && e.target.closest && e.target.closest('[data-allow-context-menu]')) {
      return;
    }
    e.preventDefault();
  }, false);
})();
</script>
"#,
        );
        head
    };
    // Mesh-head mark from the EmergHQ / Metis brand logo (assets/app_icon.png).
    // WindowBuilder also gets the icon so Windows taskbar / title bar pick it up
    // (exe VERSIONINFO icon alone is not always enough for the live HWND).
    let app_icon = dioxus::desktop::icon_from_memory::<dioxus::desktop::tao::window::Icon>(
        include_bytes!("../assets/app_icon.png"),
    )
    .ok();

    let mut window = WindowBuilder::new()
        .with_title("Metis Mesh HQ")
        .with_inner_size(LogicalSize::new(1200.0, 800.0))
        .with_min_inner_size(LogicalSize::new(960.0, 640.0));
    if let Some(ref icon) = app_icon {
        window = window.with_window_icon(Some(icon.clone()));
    }

    let mut cfg = Config::new()
        .with_menu(None)
        .with_background_color((16, 20, 24, 255))
        .with_custom_head(boot_head)
        // Context menus blocked selectively via boot_head script (compose only).
        .with_disable_context_menu(false)
        // Dioxus/wry default on Windows is %LOCALAPPDATA%\<exe-stem>. Release
        // builds ship as metis-mesh-hq-0.1.x.exe, which created a new WebView2
        // folder per version. Keep one stable cache across upgrades.
        .with_data_directory(webview_data_dir())
        .with_custom_protocol("metis", |_id, request| {
            serve_metis_protocol(request)
        })
        .with_window(window);
    if let Some(icon) = app_icon {
        cfg = cfg.with_icon(icon);
    }
    cfg
        .with_on_window(|window, _dom| {
            // tao draws its own Wayland header ("Dioxus App") under the desktop title bar,
            // and the default menu adds a Window / Edit / Help row. Drop that extra header
            // so the desktop keeps a single title bar.
            #[cfg(target_os = "linux")]
            quiet_linux_titlebar(&window);
            #[cfg(not(target_os = "linux"))]
            let _ = window;
        })
}

/// Group the process under one taskbar identity so the window icon (not a blank
/// console host) shows for release builds.
#[cfg(target_os = "windows")]
fn set_windows_app_user_model_id() {
    use std::os::windows::ffi::OsStrExt;
    let id: Vec<u16> = std::ffi::OsStr::new("DigitalExpl0it.MetisMeshHQ.1")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        let _ = windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(id.as_ptr());
    }
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

    // MapLibre / PMTiles / bridge scripts (release .exe has no assets/ folder;
    // document::Script asset!() URLs 404 without these embedded routes).
    let map_name = path
        .strip_prefix("/map/")
        .or_else(|| {
            path.rsplit_once("/map/")
                .map(|(_, rest)| rest.trim_start_matches('/'))
        })
        .map(|s| s.split(['?', '#']).next().unwrap_or(s))
        .filter(|s| !s.is_empty());
    if let Some(name) = map_name {
        let (body, ctype): (&'static [u8], &str) = match name {
            "maplibre-gl.js" => (
                include_bytes!("../assets/maplibre-gl.js").as_slice(),
                "text/javascript; charset=utf-8",
            ),
            "maplibre-gl.css" => (
                include_bytes!("../assets/maplibre-gl.css").as_slice(),
                "text/css; charset=utf-8",
            ),
            "pmtiles.js" => (
                include_bytes!("../assets/pmtiles.js").as_slice(),
                "text/javascript; charset=utf-8",
            ),
            "map_bridge.js" => (
                include_bytes!("../assets/map_bridge.js").as_slice(),
                "text/javascript; charset=utf-8",
            ),
            _ => {
                return Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .body(Cow::Borrowed(b"map asset not found" as &[u8]))
                    .unwrap_or_else(|_| {
                        Response::new(Cow::Borrowed(b"map asset not found" as &[u8]))
                    });
            }
        };
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, ctype)
            .header(header::CONTENT_LENGTH, body.len().to_string())
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .header(header::CACHE_CONTROL, "public, max-age=86400")
            .body(Cow::Borrowed(body))
            .unwrap_or_else(|_| Response::new(Cow::Borrowed(body)));
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

/// WebKitGTK ignores textarea `spellcheck` until the WebContext has spell
/// checking enabled and at least one language set. Call once the desktop
/// window exists (`dioxus::desktop::window()`), and again if the user turns
/// compose spellcheck on in Settings.
#[cfg(all(feature = "desktop", target_os = "linux"))]
pub fn enable_linux_webview_spellcheck() {
    use dioxus::desktop::wry::WebViewExtUnix;
    use webkit2gtk::{WebContextExt, WebViewExt};

    quiet_libenchant_plugin_noise();

    let desktop = dioxus::desktop::window();
    let wk = desktop.webview.webview();
    let Some(ctx) = wk.context() else {
        hq_log!(warn, "WebKit spellcheck: no WebContext yet");
        return;
    };

    if !ui::compose_spellcheck_enabled() {
        ctx.set_spell_checking_enabled(false);
        return;
    }

    let langs = spellcheck_languages_from_env();
    if !linux_hunspell_dictionary_present(&langs) {
        ctx.set_spell_checking_enabled(false);
        hq_log!(
            info,
            "WebKit spellcheck skipped — no Hunspell dictionary for [{}]. Install e.g. hunspell-en_us (Arch) / hunspell-en-us (Debian) / hunspell-en-US (Fedora).",
            langs.join(", ")
        );
        return;
    }

    ctx.set_spell_checking_enabled(true);
    let lang_refs: Vec<&str> = langs.iter().map(String::as_str).collect();
    ctx.set_spell_checking_languages(&lang_refs);
    hq_log!(
        info,
        "WebKit spellcheck on ({})",
        langs.join(", ")
    );
}

/// Enchant probes every optional backend (aspell, hspell, …). Missing .so files
/// spam `libenchant-WARNING` on Arch/minimal hosts even when Hunspell works.
#[cfg(all(feature = "desktop", target_os = "linux"))]
fn quiet_libenchant_plugin_noise() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        gtk::glib::log_set_handler(
            Some("libenchant"),
            gtk::glib::LogLevels::LEVEL_WARNING,
            false,
            false,
            |_domain, _level, message| {
                if message.contains("Error loading plugin") {
                    return;
                }
                // Keep unexpected enchant warnings visible.
                eprintln!("libenchant-WARNING **: {message}");
            },
        );
    });
}

#[cfg(all(feature = "desktop", target_os = "linux"))]
fn linux_hunspell_dictionary_present(langs: &[String]) -> bool {
    const DICT_DIRS: &[&str] = &[
        "/usr/share/hunspell",
        "/usr/share/myspell/dicts",
        "/usr/share/myspell",
    ];
    for lang in langs {
        let candidates = [
            lang.clone(),
            lang.replace('_', "-"),
            // en_US → also try bare "en" packs on some distros
            lang.split('_').next().unwrap_or(lang).to_string(),
        ];
        for tag in candidates {
            for dir in DICT_DIRS {
                if std::path::Path::new(dir).join(format!("{tag}.dic")).is_file() {
                    return true;
                }
            }
        }
    }
    false
}

#[cfg(all(feature = "desktop", target_os = "linux"))]
fn spellcheck_languages_from_env() -> Vec<String> {
    // WebKit wants hunspell-style tags (en_US), not POSIX locales (en_US.UTF-8).
    let mut out = Vec::new();
    for key in ["LC_MESSAGES", "LANG", "LANGUAGE"] {
        if let Ok(raw) = std::env::var(key) {
            for piece in raw.split([':', ';', ',']) {
                let tag = piece
                    .trim()
                    .split('.')
                    .next()
                    .unwrap_or("")
                    .replace('-', "_");
                if tag.is_empty() || tag.eq_ignore_ascii_case("C") {
                    continue;
                }
                if !out
                    .iter()
                    .any(|s: &String| s.eq_ignore_ascii_case(&tag))
                {
                    out.push(tag);
                }
            }
        }
    }
    if out.is_empty() {
        out.push("en_US".into());
    }
    // Always keep US English as a fallback when another locale is primary.
    if !out.iter().any(|s| s.eq_ignore_ascii_case("en_US") || s.eq_ignore_ascii_case("en"))
    {
        out.push("en_US".into());
    }
    out
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
    // Wayland docks ignore tao window_icon; GTK pixbuf still helps window chrome.
    apply_linux_window_icon(&gtk_window);
}

#[cfg(all(feature = "desktop", target_os = "linux"))]
const LINUX_ICON_PNG: &[u8] = include_bytes!("../assets/app_icon.png");
#[cfg(all(feature = "desktop", target_os = "linux"))]
const LINUX_ICON_NAME: &str = "metis-mesh-hq";

/// Install user `.desktop` + hicolor icon and refresh caches *before* the window
/// maps, so the first launch is not stuck with a generic dock placeholder.
#[cfg(all(feature = "desktop", target_os = "linux"))]
fn ensure_linux_desktop_integration() {
    // Match StartupWMClass / app id before GTK creates the surface.
    // (gdk::set_program_class waits until the window exists — needs a display.)
    gtk::glib::set_prgname(Some(LINUX_ICON_NAME));

    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return;
    };
    let icon_dir = home.join(".local/share/icons/hicolor/256x256/apps");
    let desktop_dir = home.join(".local/share/applications");
    let _ = std::fs::create_dir_all(&icon_dir);
    let _ = std::fs::create_dir_all(&desktop_dir);
    let icon_path = icon_dir.join(format!("{LINUX_ICON_NAME}.png"));
    if std::fs::write(&icon_path, LINUX_ICON_PNG).is_err() {
        return;
    }
    let exe = std::env::current_exe()
        .ok()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| LINUX_ICON_NAME.to_string());
    // Absolute Icon= path so the dock can resolve on first run before theme cache.
    let icon_abs = icon_path.display().to_string();
    let desktop = format!(
        "[Desktop Entry]\n\
Type=Application\n\
Version=1.0\n\
Name=Metis Mesh HQ\n\
Comment=Offline emergency headquarters for MeshCore\n\
Exec=\"{exe}\"\n\
Icon={icon_abs}\n\
Terminal=false\n\
Categories=Network;HamRadio;\n\
StartupNotify=true\n\
StartupWMClass={LINUX_ICON_NAME}\n"
    );
    let desktop_path = desktop_dir.join(format!("{LINUX_ICON_NAME}.desktop"));
    let _ = std::fs::write(&desktop_path, desktop);
    let _ = std::process::Command::new("gtk-update-icon-cache")
        .args(["-f", "-t"])
        .arg(home.join(".local/share/icons/hicolor"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    let _ = std::process::Command::new("update-desktop-database")
        .arg(&desktop_dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// GTK window icon after the surface exists (complements the pre-install above).
#[cfg(all(feature = "desktop", target_os = "linux"))]
fn apply_linux_window_icon(gtk_window: &gtk::ApplicationWindow) {
    use std::io::Cursor;

    use gtk::gdk_pixbuf::Pixbuf;
    use gtk::prelude::*;

    gtk::glib::set_prgname(Some(LINUX_ICON_NAME));
    gtk::gdk::set_program_class("Metis Mesh HQ");

    if let Ok(pixbuf) = Pixbuf::from_read(Cursor::new(LINUX_ICON_PNG)) {
        gtk_window.set_icon(Some(&pixbuf));
        gtk::Window::set_default_icon(&pixbuf);
    }
    gtk_window.set_icon_name(Some(LINUX_ICON_NAME));
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

/// WebView2 / WebKit user-data folder. Must not include the app version — the
/// dioxus default used the exe filename stem (`metis-mesh-hq-0.1.7`), which
/// littered `%LOCALAPPDATA%` with one folder per release.
fn webview_data_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            return PathBuf::from(local).join("MetisMeshHQ").join("webview");
        }
    }
    data_dir().join("webview")
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
