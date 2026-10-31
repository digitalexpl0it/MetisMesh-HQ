//! Operational MapLibre map (desktop host) with mesh + RF overlays.

use dioxus::prelude::*;
use serde_json::{json, Value};
use std::collections::HashSet;

use crate::contact_qr::{contact_add_uri, enqueue_contact_qr, qr_public_url};
use crate::db::unix_now;
use crate::path::{build_line, MapLine};
use crate::pmtiles_path;

use super::icons::{Icon, IconName};
use super::map_host::{
    dark_basemap_style, load_saved_basemap, load_saved_camera, map_bind_node_clicks, map_destroy,
    map_exists, map_fit_bounds, map_fly_to, map_get_camera, map_init, map_set_geojson,
    map_set_nodes, map_set_style, map_set_visibility, map_take_favorite_toggle, map_take_selection,
    save_basemap, save_camera, BasemapKind, MapCamera, MAP_CONTAINER_ID,
};
use super::state;
use super::widgets::{Button, ButtonKind, PageHeading, SelectInput};

#[derive(Clone, Copy, PartialEq, Eq)]
enum NodeFilter {
    All,
    Favorites,
    Users,
    Repeaters,
    Rooms,
    Sensors,
    Enrolled,
    Heard,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TrafficWindow {
    Last25,
    Hour,
    SixHours,
    Day,
}

fn node_type_label(contact_type: u8) -> &'static str {
    match contact_type {
        1 => "Chat",
        2 => "Repeater",
        3 => "Room",
        4 => "Sensor",
        _ => "Node",
    }
}

fn relative_ago(ts: Option<i64>) -> String {
    let Some(ts) = ts else {
        return "unknown".into();
    };
    let now = unix_now();
    let delta = (now - ts).max(0);
    if delta < 60 {
        "just now".into()
    } else if delta < 3600 {
        format!("{}m ago", delta / 60)
    } else if delta < 86_400 {
        format!("{}h ago", delta / 3600)
    } else if delta < 86_400 * 14 {
        format!("{}d ago", delta / 86_400)
    } else {
        format!("{}w ago", delta / (86_400 * 7))
    }
}

fn collection(features: Vec<Value>) -> Value {
    json!({ "type": "FeatureCollection", "features": features })
}

fn line_feature(line: &MapLine) -> Value {
    json!({
        "type": "Feature",
        "properties": {
            "outbound": line.outbound,
            "hops": line.hops,
        },
        "geometry": {
            "type": "LineString",
            "coordinates": line.coords,
        }
    })
}

/// Pin colors by MeshCore contact type (HQ overrides).
fn type_color(enrollment: &str, contact_type: u8) -> &'static str {
    if enrollment == "hq" {
        return "#f472b6";
    }
    match contact_type {
        1 => "#38bdf8", // Chat / companion user
        2 => "#34d399", // Repeater
        3 => "#fbbf24", // Room server
        4 => "#fb923c", // Sensor
        _ => "#94a3b8",
    }
}

fn type_key(enrollment: &str, contact_type: u8) -> &'static str {
    if enrollment == "hq" {
        return "hq";
    }
    match contact_type {
        1 => "chat",
        2 => "repeater",
        3 => "room",
        4 => "sensor",
        _ => "node",
    }
}

fn snr_color(snr: f64) -> &'static str {
    if snr >= 8.0 {
        "#34d399"
    } else if snr >= 3.0 {
        "#fbbf24"
    } else {
        "#f87171"
    }
}

fn noise_color(noise: i64) -> &'static str {
    if noise <= -110 {
        "#34d399"
    } else if noise <= -95 {
        "#fbbf24"
    } else {
        "#f87171"
    }
}

fn load_favorites() -> HashSet<String> {
    state()
        .db
        .setting("map_favorites")
        .ok()
        .flatten()
        .map(|raw| {
            raw.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn save_favorites(favs: &HashSet<String>) {
    let mut list: Vec<_> = favs.iter().cloned().collect();
    list.sort();
    let _ = state().db.set_setting("map_favorites", &list.join(","));
}

struct MapModel {
    nodes: Value,
    solid: Value,
    dashed: Value,
    snr: Value,
    noise: Value,
    note: String,
    bounds: Option<(f64, f64, f64, f64)>,
    pmtiles_ok: bool,
}

fn build_model(
    filter: NodeFilter,
    window: TrafficWindow,
    show_solid: bool,
    show_dashed: bool,
    show_snr: bool,
    show_noise: bool,
    favorites: &HashSet<String>,
    basemap: BasemapKind,
) -> MapModel {
    let db = &state().db;
    let path_nodes = db.known_nodes_for_paths().unwrap_or_default();
    let hq = db.hq_position().ok().flatten();
    let rows = db.list_nodes().unwrap_or_default();
    let rf_by_key = db.latest_rf_by_pubkey().unwrap_or_default();

    let node_features: Vec<Value> = rows
        .iter()
        .filter_map(|node| {
            let lat = node.lat?;
            let lon = node.lon?;
            let enrollment = node.enrollment.as_str();
            let keep = match filter {
                NodeFilter::All => true,
                NodeFilter::Favorites => {
                    favorites.contains(&node.pubkey) || enrollment == "hq"
                }
                NodeFilter::Users => node.contact_type == 1 || enrollment == "hq",
                NodeFilter::Repeaters => node.contact_type == 2,
                NodeFilter::Rooms => node.contact_type == 3,
                NodeFilter::Sensors => node.contact_type == 4,
                NodeFilter::Enrolled => enrollment == "enrolled" || enrollment == "hq",
                NodeFilter::Heard => enrollment == "heard",
            };
            if !keep {
                return None;
            }
            let type_label = if enrollment == "hq" {
                "HQ"
            } else {
                node_type_label(node.contact_type)
            };
            let favorited = favorites.contains(&node.pubkey);
            let color = type_color(enrollment, node.contact_type);
            let share_type = if enrollment == "hq" {
                1
            } else if (1..=4).contains(&node.contact_type) {
                node.contact_type
            } else {
                1
            };
            let contact_uri = contact_add_uri(&node.name, &node.pubkey, share_type);
            // Read-only: background worker writes data/qr + nodes.qr_file / qr_uri.
            let qr_url = match (node.qr_file.as_deref(), node.qr_uri.as_deref()) {
                (Some(file), Some(stored)) if stored == contact_uri => {
                    qr_public_url(file).unwrap_or_default()
                }
                (Some(file), _) => qr_public_url(file).unwrap_or_default(),
                _ => String::new(),
            };
            let hops = if node.path_len > 0 && node.path_len < 64 {
                Some(node.path_len)
            } else if !node.hop_bytes.is_empty() {
                Some(node.hop_bytes.len() as i64)
            } else {
                None
            };
            let rf = rf_by_key.get(&node.pubkey);
            Some(json!({
                "type": "Feature",
                "properties": {
                    "name": node.name,
                    "enrollment": enrollment,
                    "type_label": type_label,
                    "type_key": type_key(enrollment, node.contact_type),
                    "contact_type": node.contact_type,
                    "pubkey": node.pubkey,
                    "prefix": node.prefix,
                    "favorite": favorited,
                    "lat": lat,
                    "lon": lon,
                    "last_heard_label": relative_ago(node.last_heard),
                    "last_advert_label": relative_ago(node.last_advert),
                    "hops": hops,
                    "noise": rf.map(|s| s.0),
                    "rssi": rf.map(|s| s.1),
                    "snr": rf.map(|s| s.2),
                    "contact_uri": contact_uri,
                    "qr_url": qr_url,
                    "color": color,
                    "radius": if enrollment == "hq" { 11 } else { 9 },
                },
                "geometry": { "type": "Point", "coordinates": [lon, lat] }
            }))
        })
        .collect();

    let since = match window {
        TrafficWindow::Last25 => None,
        TrafficWindow::Hour => Some(unix_now() - 3600),
        TrafficWindow::SixHours => Some(unix_now() - 6 * 3600),
        TrafficWindow::Day => Some(unix_now() - 86_400),
    };
    let limit = match window {
        TrafficWindow::Last25 => 25,
        _ => 200,
    };
    let messages = db
        .recent_messages_window(limit, since)
        .unwrap_or_default();

    let mut solid = Vec::new();
    let mut dashed = Vec::new();
    let mut snr_features = Vec::new();
    let hq_pos = hq.as_ref().map(|(_, lat, lon)| (*lon, *lat));

    for message in &messages {
        let node_pos = message.sender_pubkey.as_ref().and_then(|key| {
            path_nodes
                .iter()
                .find(|node| hex::encode(&node.pubkey) == *key)
                .and_then(|node| Some((node.lon?, node.lat?)))
        });
        let outbound = message.direction == "tx";
        let (from, to) = if outbound {
            (hq_pos, node_pos)
        } else {
            (node_pos, hq_pos)
        };
        if let Some(line) = build_line(
            outbound,
            from,
            to,
            &message.path,
            message.hash_size.max(1) as usize,
            &path_nodes,
            message.hop_count.unwrap_or(0) as u32,
        ) {
            let feature = line_feature(&line);
            if line.dashed {
                if show_dashed {
                    dashed.push(feature);
                }
            } else if show_solid {
                solid.push(feature);
            }
        }

        if show_snr {
            if let Some(snr) = message.snr {
                let point = if outbound { hq_pos } else { node_pos };
                if let Some((lon, lat)) = point {
                    snr_features.push(json!({
                        "type": "Feature",
                        "properties": {
                            "snr": snr,
                            "color": snr_color(snr),
                            "label": format!("{snr:.1} dB"),
                        },
                        "geometry": { "type": "Point", "coordinates": [lon, lat] }
                    }));
                }
            }
        }
    }

    let noise_features: Vec<Value> = if show_noise {
        db.recent_rf_samples(80)
            .unwrap_or_default()
            .into_iter()
            .map(|sample| {
                json!({
                    "type": "Feature",
                    "properties": {
                        "name": sample.name,
                        "noise": sample.noise_floor,
                        "rssi": sample.last_rssi,
                        "color": noise_color(sample.noise_floor),
                        "label": format!("{} dBm", sample.noise_floor),
                    },
                    "geometry": { "type": "Point", "coordinates": [sample.lon, sample.lat] }
                })
            })
            .collect()
    } else {
        Vec::new()
    };

    let mut west = f64::INFINITY;
    let mut south = f64::INFINITY;
    let mut east = f64::NEG_INFINITY;
    let mut north = f64::NEG_INFINITY;
    let mut any = false;
    for node in &rows {
        if let (Some(lat), Some(lon)) = (node.lat, node.lon) {
            any = true;
            west = west.min(lon);
            east = east.max(lon);
            south = south.min(lat);
            north = north.max(lat);
        }
    }
    let bounds = if any {
        Some((west, south, east, north))
    } else {
        None
    };
    let pmtiles_ok = pmtiles_path::current_pmtiles_path().is_some();
    let basemap_note = match (basemap, pmtiles_ok) {
        (BasemapKind::Outline, _) => " · outline + state borders",
        (_, true) => " · offline PMTiles",
        (BasemapKind::Streets, false)
        | (BasemapKind::Terrain, false)
        | (BasemapKind::Topo, false) => " · outline (set PMTiles for street/terrain/topo tiles)",
    };
    let note = if node_features.is_empty() {
        "No GPS fixes yet. Positions appear when a node advertises coordinates.".into()
    } else if hq_pos.is_none() {
        "Routes need a headquarters fix. Set the radio location under Device → Identity.".into()
    } else {
        format!(
            "{} GPS nodes · {} solid · {} dashed{}",
            node_features.len(),
            solid.len(),
            dashed.len(),
            basemap_note
        )
    };

    MapModel {
        nodes: collection(node_features),
        solid: collection(solid),
        dashed: collection(dashed),
        snr: collection(snr_features),
        noise: collection(noise_features),
        note,
        bounds,
        pmtiles_ok,
    }
}

fn solid_layers() -> Value {
    json!([{
        "id": "solid-lines",
        "type": "line",
        "source": "solid-routes",
        "paint": { "line-color": "#34d399", "line-width": 3 }
    }])
}

fn dashed_layers() -> Value {
    json!([{
        "id": "dashed-lines",
        "type": "line",
        "source": "dashed-routes",
        "paint": {
            "line-color": "#fbbf24",
            "line-width": 2,
            "line-dasharray": [2, 2]
        }
    }])
}

fn snr_layers() -> Value {
    json!([{
        "id": "snr-dots",
        "type": "circle",
        "source": "snr-points",
        "paint": {
            "circle-radius": 10,
            "circle-color": ["coalesce", ["get", "color"], "#fbbf24"],
            "circle-opacity": 0.55,
            "circle-stroke-width": 1,
            "circle-stroke-color": "#0b1220"
        }
    }])
}

fn noise_layers() -> Value {
    json!([{
        "id": "noise-dots",
        "type": "circle",
        "source": "noise-points",
        "paint": {
            "circle-radius": 12,
            "circle-color": ["coalesce", ["get", "color"], "#a78bfa"],
            "circle-opacity": 0.45,
            "circle-stroke-width": 1,
            "circle-stroke-color": "#e2e8f0"
        }
    }])
}

fn filter_chip(active: bool) -> &'static str {
    if active {
        "map-chip map-chip-active"
    } else {
        "map-chip"
    }
}

fn default_camera() -> (MapCamera, bool) {
    // Returns (camera, skip_fit_bounds).
    if let Some(cam) = load_saved_camera() {
        return (cam, true);
    }
    if let Some((_, lat, lon)) = state().db.hq_position().ok().flatten() {
        return (
            MapCamera {
                lng: lon,
                lat,
                zoom: 11.0,
            },
            true,
        );
    }
    if let Some((lon, lat)) = state().db.list_nodes().ok().and_then(|rows| {
        rows.into_iter().find_map(|n| Some((n.lon?, n.lat?)))
    }) {
        return (
            MapCamera {
                lng: lon,
                lat,
                zoom: 10.0,
            },
            true,
        );
    }
    (
        MapCamera {
            lng: -100.0,
            lat: 38.0,
            zoom: 3.5,
        },
        false,
    )
}

#[component]
pub fn MapPage() -> Element {
    let mut filter = use_signal(|| NodeFilter::All);
    let mut window = use_signal(|| TrafficWindow::Last25);
    let mut basemap = use_signal(load_saved_basemap);
    let mut cluster = use_signal(|| true);
    let mut show_solid = use_signal(|| true);
    let mut show_dashed = use_signal(|| true);
    let mut show_snr = use_signal(|| false);
    let mut show_noise = use_signal(|| false);
    let mut favorites = use_signal(load_favorites);
    let mut tick = use_signal(|| 0u32);
    let mut ready = use_signal(|| false);
    let mut fitted = use_signal(|| false);
    let mut style_key = use_signal(|| 0u32);
    let mut preserve_camera = use_signal(|| None::<MapCamera>);
    let mut map_error = use_signal(String::new);
    let mut show_download = use_signal(|| pmtiles_path::current_pmtiles_path().is_none());
    let mut zip_input = use_signal(String::new);
    let mut radius_miles = use_signal(|| 50u32);
    let mut dl_note = use_signal(String::new);

    let _ = tick();
    let model = build_model(
        filter(),
        window(),
        show_solid(),
        show_dashed(),
        show_snr(),
        show_noise(),
        &favorites(),
        basemap(),
    );

    // Init once; later basemap switches use setStyle (no flashy destroy).
    // Subscribe only to style_key so basemap.set + style_key++ do not double-run.
    use_effect(move || {
        let key = style_key();
        let kind = basemap.peek().clone();
        let preserved = preserve_camera.peek().clone();
        spawn(async move {
            // Let flex layout assign map-stage height before MapLibre measures.
            tokio::time::sleep(std::time::Duration::from_millis(180)).await;
            let tiles = pmtiles_path::basemap_protocol_url();
            let style = dark_basemap_style(tiles.as_deref(), kind);
            let live = map_get_camera().await;
            let (cam, skip_fit) = if let Some(cam) = preserved.or(live) {
                (cam, true)
            } else if let Some(saved) = load_saved_camera() {
                (saved, true)
            } else {
                default_camera()
            };
            let exists = map_exists().await;
            let result = if exists {
                map_set_style(&style, Some(cam)).await
            } else {
                map_init(&style, cam.lng, cam.lat, cam.zoom).await
            };
            match result {
                Ok(()) => {
                    map_error.set(String::new());
                    ready.set(true);
                    // Never auto-fit after a style swap — keep the user's view.
                    fitted.set(true);
                    let _ = skip_fit;
                    preserve_camera.set(None);
                    save_camera(cam);
                    save_basemap(kind);
                    // Force overlay re-push after style swap (sources were wiped).
                    let _ = key;
                    tick.with_mut(|n| *n = n.wrapping_add(1));
                }
                Err(err) => {
                    map_error.set(format!("Map failed to start: {err}"));
                    ready.set(false);
                }
            }
        });
    });

    use_drop(move || {
        spawn(async move {
            if let Some(cam) = map_get_camera().await {
                save_camera(cam);
            }
            map_destroy().await;
        });
    });

    use_effect(move || {
        let _ = (
            tick(),
            filter(),
            window(),
            show_solid(),
            show_dashed(),
            show_snr(),
            show_noise(),
            cluster(),
            favorites(),
        );
        if !ready() {
            return;
        }
        // Peek basemap — style swaps own the re-push via tick bump after setStyle.
        let snapshot = build_model(
            filter(),
            window(),
            show_solid(),
            show_dashed(),
            show_snr(),
            show_noise(),
            &favorites(),
            *basemap.peek(),
        );
        let nodes = snapshot.nodes.clone();
        let solid = snapshot.solid.clone();
        let dashed = snapshot.dashed.clone();
        let snr = snapshot.snr.clone();
        let noise = snapshot.noise.clone();
        let bounds = snapshot.bounds;
        let do_fit = !fitted() && bounds.is_some();
        let do_cluster = cluster();
        spawn(async move {
            map_set_nodes(&nodes, do_cluster).await;
            map_set_geojson("solid-routes", &solid, &solid_layers()).await;
            map_set_geojson("dashed-routes", &dashed, &dashed_layers()).await;
            map_set_geojson("snr-points", &snr, &snr_layers()).await;
            map_set_geojson("noise-points", &noise, &noise_layers()).await;
            map_bind_node_clicks("node-dots").await;
            if do_fit {
                if let Some((w, s, e, n)) = bounds {
                    map_fit_bounds(w, s, e, n).await;
                    fitted.set(true);
                }
            }
        });
    });

    use_effect(move || {
        if !ready() {
            return;
        }
        let solid = show_solid();
        let dashed = show_dashed();
        let snr = show_snr();
        let noise = show_noise();
        spawn(async move {
            map_set_visibility("solid-lines", solid).await;
            map_set_visibility("dashed-lines", dashed).await;
            map_set_visibility("snr-dots", snr).await;
            map_set_visibility("noise-dots", noise).await;
        });
    });

    use_effect(move || {
        spawn(async move {
            let mut cam_ticks: u32 = 0;
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                if let Some(key) = map_take_favorite_toggle().await {
                    if !key.is_empty() {
                        favorites.with_mut(|set| {
                            if set.contains(&key) {
                                set.remove(&key);
                            } else {
                                set.insert(key);
                            }
                            save_favorites(set);
                        });
                    }
                }
                // If a pin opened without a cached QR yet, enqueue background encode
                // (next map refresh picks up qr_url). Never encode on this thread.
                if let Some(raw) = map_take_selection().await {
                    if let Ok(props) = serde_json::from_str::<Value>(&raw) {
                        let needs_qr = props
                            .get("qr_url")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .is_empty();
                        if needs_qr {
                            if let Some(key) = props.get("pubkey").and_then(|v| v.as_str()) {
                                enqueue_contact_qr(key);
                            }
                        }
                    }
                }
                // Camera persistence: every ~2s, not every poll (was locking the webview).
                cam_ticks = cam_ticks.wrapping_add(1);
                if cam_ticks % 14 == 0 {
                    if let Some(cam) = map_get_camera().await {
                        save_camera(cam);
                    }
                }
            }
        });
    });

    use_effect(move || {
        spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                tick.with_mut(|n| *n = n.wrapping_add(1));
            }
        });
    });

    let set_basemap = move |kind: BasemapKind| {
        if basemap() == kind {
            return;
        }
        spawn(async move {
            let cam = map_get_camera().await;
            preserve_camera.set(cam);
            // Set basemap before bumping style_key; the style effect peeks basemap.
            basemap.set(kind);
            save_basemap(kind);
            style_key.with_mut(|n| *n = n.wrapping_add(1));
        });
    };

    rsx! {
        document::Stylesheet { href: asset!("/assets/maplibre-gl.css") }
        document::Script { src: asset!("/assets/maplibre-gl.js") }
        document::Script { src: asset!("/assets/pmtiles.js") }
        document::Script { src: asset!("/assets/map_bridge.js") }
        div { class: "flex h-full min-h-0 flex-col gap-3 overflow-hidden",
            PageHeading { icon: IconName::Map, title: "Map".to_string(), "{model.note}" }
            if !map_error().is_empty() {
                p { class: "text-sm text-destructive", "{map_error}" }
            }
            {
                let dl = crate::map_tiles::download_status();
                if dl.running {
                    rsx! {
                        p { class: "text-xs text-muted-foreground", "{dl.detail}" }
                    }
                } else if !dl_note().is_empty() {
                    rsx! {
                        p { class: "text-xs text-muted-foreground", "{dl_note}" }
                    }
                } else if !model.pmtiles_ok {
                    rsx! {
                        p { class: "text-xs text-muted-foreground",
                            "Outline works offline. Download a local area (ZIP + radius) for Streets / Terrain / Topo."
                        }
                    }
                } else {
                    rsx! {}
                }
            }
            div { class: "map-toolbar",
                div { class: "flex flex-wrap gap-2",
                    button { class: filter_chip(filter() == NodeFilter::All), onclick: move |_| filter.set(NodeFilter::All), "All" }
                    button { class: filter_chip(filter() == NodeFilter::Favorites), onclick: move |_| filter.set(NodeFilter::Favorites), "Favorites" }
                    button { class: filter_chip(filter() == NodeFilter::Users), onclick: move |_| filter.set(NodeFilter::Users), "Users" }
                    button { class: filter_chip(filter() == NodeFilter::Repeaters), onclick: move |_| filter.set(NodeFilter::Repeaters), "Repeaters" }
                    button { class: filter_chip(filter() == NodeFilter::Rooms), onclick: move |_| filter.set(NodeFilter::Rooms), "Rooms" }
                    button { class: filter_chip(filter() == NodeFilter::Sensors), onclick: move |_| filter.set(NodeFilter::Sensors), "Sensors" }
                    button { class: filter_chip(filter() == NodeFilter::Enrolled), onclick: move |_| filter.set(NodeFilter::Enrolled), "Enrolled" }
                    button { class: filter_chip(filter() == NodeFilter::Heard), onclick: move |_| filter.set(NodeFilter::Heard), "Heard" }
                }
                div { class: "min-w-[10rem]",
                    SelectInput {
                        id: "map-traffic-window".to_string(),
                        value: match window() {
                            TrafficWindow::Last25 => "25".to_string(),
                            TrafficWindow::Hour => "1h".to_string(),
                            TrafficWindow::SixHours => "6h".to_string(),
                            TrafficWindow::Day => "24h".to_string(),
                        },
                        onchange: move |event: FormEvent| {
                            window.set(match event.value().as_str() {
                                "1h" => TrafficWindow::Hour,
                                "6h" => TrafficWindow::SixHours,
                                "24h" => TrafficWindow::Day,
                                _ => TrafficWindow::Last25,
                            });
                            fitted.set(false);
                        },
                        option { value: "25", style: "background-color:#101418;color:#f4f4f5;", "Last 25 messages" }
                        option { value: "1h", style: "background-color:#101418;color:#f4f4f5;", "Last 1 hour" }
                        option { value: "6h", style: "background-color:#101418;color:#f4f4f5;", "Last 6 hours" }
                        option { value: "24h", style: "background-color:#101418;color:#f4f4f5;", "Last 24 hours" }
                    }
                }
                label { class: "map-toggle",
                    input { r#type: "checkbox", checked: cluster(), oninput: move |_| cluster.set(!cluster()) }
                    "Cluster markers"
                }
                label { class: "map-toggle",
                    input { r#type: "checkbox", checked: show_solid(), oninput: move |_| show_solid.set(!show_solid()) }
                    "Solid routes"
                }
                label { class: "map-toggle",
                    input { r#type: "checkbox", checked: show_dashed(), oninput: move |_| show_dashed.set(!show_dashed()) }
                    "Dashed routes"
                }
                label { class: "map-toggle",
                    input { r#type: "checkbox", checked: show_snr(), oninput: move |_| show_snr.set(!show_snr()) }
                    "SNR"
                }
                label { class: "map-toggle",
                    input { r#type: "checkbox", checked: show_noise(), oninput: move |_| show_noise.set(!show_noise()) }
                    "Noise"
                }
                Button {
                    kind: ButtonKind::Ghost,
                    onclick: move |_| {
                        if let Some((w, s, e, n)) = model.bounds {
                            spawn(async move {
                                if (e - w).abs() < 1e-5 && (n - s).abs() < 1e-5 {
                                    map_fly_to(w, s, 12.0).await;
                                } else {
                                    map_fit_bounds(w, s, e, n).await;
                                }
                            });
                        }
                    },
                    Icon { name: IconName::Locate, size: 14 }
                    "Fit"
                }
                Button {
                    kind: ButtonKind::Ghost,
                    onclick: move |_| state().radio.probe_noise_samples(),
                    Icon { name: IconName::Signal, size: 14 }
                    "Probe noise"
                }
            }
            div { class: "map-stage",
                div {
                    id: "{MAP_CONTAINER_ID}",
                    class: "map-canvas",
                }
                if show_download() || !model.pmtiles_ok || crate::map_tiles::download_status().running {
                    div { class: "map-download-card",
                        div { class: "map-download-head",
                            span { class: "map-download-title", "Download offline map" }
                            if model.pmtiles_ok && !crate::map_tiles::download_status().running {
                                button {
                                    class: "map-download-dismiss",
                                    title: "Dismiss",
                                    onclick: move |_| show_download.set(false),
                                    "×"
                                }
                            }
                        }
                        p { class: "map-download-copy",
                            "One pack unlocks Streets, Terrain, and Topo. Enter a US ZIP and radius, or leave ZIP blank to use the map center (or HQ)."
                        }
                        div { class: "map-download-row",
                            input {
                                class: "map-download-input",
                                r#type: "text",
                                maxlength: "10",
                                placeholder: "ZIP (optional)",
                                value: "{zip_input}",
                                oninput: move |event: FormEvent| zip_input.set(event.value()),
                            }
                            button {
                                class: "map-download-go",
                                disabled: crate::map_tiles::download_status().running,
                                onclick: move |_| {
                                    let zip = zip_input();
                                    let radius = radius_miles() as f64;
                                    let db = state().db.clone();
                                    spawn(async move {
                                        let cam = map_get_camera().await;
                                        let center = cam.map(|c| (c.lat, c.lng));
                                        crate::map_tiles::start_area_download(
                                            db,
                                            zip,
                                            radius,
                                            center,
                                        );
                                        dl_note.set("Download started…".into());
                                        show_download.set(true);
                                        for _ in 0..180 {
                                            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                                            tick.with_mut(|n| *n = n.wrapping_add(1));
                                            let s = crate::map_tiles::download_status();
                                            if !s.running {
                                                if s.phase == "done" {
                                                    dl_note.set(s.detail.clone());
                                                    // Refresh style so Streets picks up the new archive.
                                                    preserve_camera.set(map_get_camera().await);
                                                    if *basemap.peek() == BasemapKind::Outline {
                                                        basemap.set(BasemapKind::Streets);
                                                    }
                                                    style_key.with_mut(|n| *n = n.wrapping_add(1));
                                                } else if s.phase == "error" {
                                                    dl_note.set(s.detail.clone());
                                                }
                                                break;
                                            }
                                            dl_note.set(s.detail.clone());
                                        }
                                    });
                                },
                                if crate::map_tiles::download_status().running { "Working…" } else { "Download" }
                            }
                        }
                        div { class: "map-download-radii",
                            for miles in [25u32, 50, 75, 100, 150] {
                                {
                                    let selected = radius_miles() == miles;
                                    rsx! {
                                        button {
                                            key: "{miles}",
                                            class: if selected { "map-radius-chip active" } else { "map-radius-chip" },
                                            onclick: move |_| radius_miles.set(miles),
                                            "{miles} mi"
                                        }
                                    }
                                }
                            }
                        }
                        if crate::map_tiles::download_status().running {
                            p { class: "map-download-status",
                                "{crate::map_tiles::download_status().detail}"
                            }
                        }
                    }
                } else if model.pmtiles_ok {
                    button {
                        class: "map-download-fab",
                        title: "Download another offline map area",
                        onclick: move |_| show_download.set(true),
                        "Offline map"
                    }
                }
                div { class: "map-legend",
                    p { class: "map-legend-title", "Legend" }
                    div { class: "map-legend-row", span { class: "map-type-swatch map-type-hq" } "HQ" }
                    div { class: "map-legend-row", span { class: "map-type-swatch map-type-chat" } "Chat" }
                    div { class: "map-legend-row", span { class: "map-type-swatch map-type-repeater" } "Repeater" }
                    div { class: "map-legend-row", span { class: "map-type-swatch map-type-room" } "Room" }
                    div { class: "map-legend-row", span { class: "map-type-swatch map-type-sensor" } "Sensor" }
                    div { class: "map-legend-row", span { class: "map-cluster-swatch metis-cluster-cool" } "Cluster (5+)" }
                    div { class: "map-legend-row", span { class: "map-line", style: "background:#34d399;" } "Resolved hops" }
                    div { class: "map-legend-row", span { class: "map-line map-line-dashed", style: "background:#fbbf24;" } "Endpoints only" }
                }
                div { class: "map-basemap-switcher", title: "Basemap",
                    button {
                        class: if basemap() == BasemapKind::Outline { "map-basemap-btn active" } else { "map-basemap-btn" },
                        onclick: move |_| set_basemap(BasemapKind::Outline),
                        title: "Outline",
                        span { class: "map-basemap-icon map-basemap-outline" }
                        span { "Outline" }
                    }
                    button {
                        class: if basemap() == BasemapKind::Streets { "map-basemap-btn active" } else { "map-basemap-btn" },
                        onclick: move |_| set_basemap(BasemapKind::Streets),
                        title: "Streets",
                        span { class: "map-basemap-icon map-basemap-streets" }
                        span { "Streets" }
                    }
                    button {
                        class: if basemap() == BasemapKind::Terrain { "map-basemap-btn active" } else { "map-basemap-btn" },
                        onclick: move |_| set_basemap(BasemapKind::Terrain),
                        title: "Terrain",
                        span { class: "map-basemap-icon map-basemap-terrain" }
                        span { "Terrain" }
                    }
                    button {
                        class: if basemap() == BasemapKind::Topo { "map-basemap-btn active" } else { "map-basemap-btn" },
                        onclick: move |_| set_basemap(BasemapKind::Topo),
                        title: "Topo",
                        span { class: "map-basemap-icon map-basemap-topo" }
                        span { "Topo" }
                    }
                }
            }
        }
    }
}
