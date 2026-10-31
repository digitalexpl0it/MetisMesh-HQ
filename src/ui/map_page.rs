//! Operational MapLibre map (desktop host) with mesh + RF overlays.

use dioxus::prelude::*;
use serde_json::{json, Value};

use crate::db::unix_now;
use crate::path::{build_line, MapLine};
use crate::pmtiles_path;

use super::icons::{Icon, IconName};
use super::map_host::{
    dark_basemap_style, map_bind_node_clicks, map_destroy, map_fit_bounds, map_fly_to, map_init,
    map_set_geojson, map_set_visibility, map_take_selection, MAP_CONTAINER_ID,
};
use super::state;
use super::widgets::{Button, ButtonKind, PageHeading, SelectInput};

#[derive(Clone, Copy, PartialEq, Eq)]
enum NodeFilter {
    All,
    Enrolled,
    Heard,
    Gps,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TrafficWindow {
    Last25,
    Hour,
    SixHours,
    Day,
}

#[derive(Clone, Default)]
struct SelectedNode {
    name: String,
    enrollment: String,
    type_label: String,
    pubkey: String,
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

fn enrollment_color(enrollment: &str) -> &'static str {
    match enrollment {
        "hq" => "#f472b6",
        "enrolled" => "#34d399",
        "pending" => "#fbbf24",
        "denied" => "#f87171",
        _ => "#38bdf8",
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
) -> MapModel {
    let db = &state().db;
    let path_nodes = db.known_nodes_for_paths().unwrap_or_default();
    let hq = db.hq_position().ok().flatten();
    let rows = db.list_nodes().unwrap_or_default();

    let node_features: Vec<Value> = rows
        .iter()
        .filter_map(|node| {
            let lat = node.lat?;
            let lon = node.lon?;
            let enrollment = node.enrollment.as_str();
            let keep = match filter {
                NodeFilter::All => true,
                NodeFilter::Enrolled => enrollment == "enrolled" || enrollment == "hq",
                NodeFilter::Heard => enrollment == "heard",
                NodeFilter::Gps => true,
            };
            if !keep {
                return None;
            }
            let type_label = if enrollment == "hq" {
                "HQ"
            } else {
                node_type_label(node.contact_type)
            };
            Some(json!({
                "type": "Feature",
                "properties": {
                    "name": node.name,
                    "enrollment": enrollment,
                    "type_label": type_label,
                    "contact_type": node.contact_type,
                    "pubkey": node.pubkey,
                    "color": enrollment_color(enrollment),
                    "radius": if enrollment == "hq" { 9 } else { 6 },
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
                let point = if outbound {
                    hq_pos
                } else {
                    node_pos
                };
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
            if pmtiles_ok {
                " · offline PMTiles basemap"
            } else {
                " · land outline basemap (optional PMTiles in Settings)"
            }
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

fn node_layers() -> Value {
    json!([{
        "id": "node-dots",
        "type": "circle",
        "source": "nodes",
        "paint": {
            "circle-radius": ["coalesce", ["get", "radius"], 6],
            "circle-color": ["coalesce", ["get", "color"], "#38bdf8"],
            "circle-stroke-width": 1.5,
            "circle-stroke-color": "#e2e8f0"
        }
    }])
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

#[component]
pub fn MapPage() -> Element {
    let mut filter = use_signal(|| NodeFilter::All);
    let mut window = use_signal(|| TrafficWindow::Last25);
    let mut show_solid = use_signal(|| true);
    let mut show_dashed = use_signal(|| true);
    let mut show_snr = use_signal(|| false);
    let mut show_noise = use_signal(|| false);
    let mut tick = use_signal(|| 0u32);
    let mut ready = use_signal(|| false);
    let mut fitted = use_signal(|| false);
    let mut selected = use_signal(|| None::<SelectedNode>);
    let mut map_error = use_signal(String::new);

    let _ = tick();
    let model = build_model(
        filter(),
        window(),
        show_solid(),
        show_dashed(),
        show_snr(),
        show_noise(),
    );

    // Init MapLibre once scripts are present.
    use_effect(move || {
        spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(180)).await;
            let style = dark_basemap_style(pmtiles_path::basemap_protocol_url());
            // Prefer HQ / first GPS node for initial camera.
            let (lng, lat, zoom) = state()
                .db
                .hq_position()
                .ok()
                .flatten()
                .map(|(_, la, lo)| (lo, la, 9.0))
                .or_else(|| {
                    state().db.list_nodes().ok().and_then(|rows| {
                        rows.into_iter().find_map(|n| Some((n.lon?, n.lat?, 9.0)))
                    })
                })
                .unwrap_or((0.0, 20.0, 1.5));
            match map_init(&style, lng, lat, zoom).await {
                Ok(()) => {
                    map_error.set(String::new());
                    ready.set(true);
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
            map_destroy().await;
        });
    });

    // Push GeoJSON whenever filters/tick change and the map is ready.
    use_effect(move || {
        let _ = (
            tick(),
            filter(),
            window(),
            show_solid(),
            show_dashed(),
            show_snr(),
            show_noise(),
        );
        if !ready() {
            return;
        }
        let snapshot = build_model(
            filter(),
            window(),
            show_solid(),
            show_dashed(),
            show_snr(),
            show_noise(),
        );
        let nodes = snapshot.nodes.clone();
        let solid = snapshot.solid.clone();
        let dashed = snapshot.dashed.clone();
        let snr = snapshot.snr.clone();
        let noise = snapshot.noise.clone();
        let bounds = snapshot.bounds;
        let do_fit = !fitted() && bounds.is_some();
        spawn(async move {
            map_set_geojson("nodes", &nodes, &node_layers()).await;
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

    // Visibility toggles.
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

    // Live refresh + click selection poll.
    use_effect(move || {
        spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                tick.with_mut(|n| *n = n.wrapping_add(1));
                if let Some(raw) = map_take_selection().await {
                    if let Ok(value) = serde_json::from_str::<Value>(&raw) {
                        selected.set(Some(SelectedNode {
                            name: value
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("Node")
                                .into(),
                            enrollment: value
                                .get("enrollment")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .into(),
                            type_label: value
                                .get("type_label")
                                .and_then(|v| v.as_str())
                                .unwrap_or("Node")
                                .into(),
                            pubkey: value
                                .get("pubkey")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .into(),
                        }));
                    }
                }
            }
        });
    });

    let rows = state().db.list_nodes().unwrap_or_default();

    rsx! {
        document::Stylesheet { href: asset!("/assets/maplibre-gl.css") }
        document::Script { src: asset!("/assets/maplibre-gl.js") }
        document::Script { src: asset!("/assets/pmtiles.js") }
        document::Script { src: asset!("/assets/map_bridge.js") }
        div { class: "flex h-full min-h-0 flex-col gap-3",
            PageHeading { icon: IconName::Map, title: "Map".to_string(), "{model.note}" }
            if !map_error().is_empty() {
                p { class: "text-sm text-destructive", "{map_error}" }
            }
            if !model.pmtiles_ok {
                p { class: "text-xs text-muted-foreground",
                    "Showing bundled land outlines. Add a regional PMTiles path in Settings → Offline map tiles for a full offline basemap."
                }
            }
            div { class: "map-toolbar",
                div { class: "flex flex-wrap gap-2",
                    button {
                        class: if filter() == NodeFilter::All { "map-chip map-chip-active" } else { "map-chip" },
                        onclick: move |_| filter.set(NodeFilter::All),
                        "All"
                    }
                    button {
                        class: if filter() == NodeFilter::Enrolled { "map-chip map-chip-active" } else { "map-chip" },
                        onclick: move |_| filter.set(NodeFilter::Enrolled),
                        "Enrolled"
                    }
                    button {
                        class: if filter() == NodeFilter::Heard { "map-chip map-chip-active" } else { "map-chip" },
                        onclick: move |_| filter.set(NodeFilter::Heard),
                        "Heard"
                    }
                    button {
                        class: if filter() == NodeFilter::Gps { "map-chip map-chip-active" } else { "map-chip" },
                        onclick: move |_| filter.set(NodeFilter::Gps),
                        "Has GPS"
                    }
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
                            spawn(async move { map_fit_bounds(w, s, e, n).await; });
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
                div { class: "map-legend",
                    p { class: "map-legend-title", "Legend" }
                    div { class: "map-legend-row", span { class: "map-swatch", style: "background:#f472b6;" } "HQ" }
                    div { class: "map-legend-row", span { class: "map-swatch", style: "background:#34d399;" } "Enrolled" }
                    div { class: "map-legend-row", span { class: "map-swatch", style: "background:#38bdf8;" } "Heard" }
                    div { class: "map-legend-row", span { class: "map-swatch", style: "background:#fbbf24;" } "Pending" }
                    div { class: "map-legend-row", span { class: "map-line", style: "background:#34d399;" } "Resolved hops" }
                    div { class: "map-legend-row", span { class: "map-line map-line-dashed", style: "background:#fbbf24;" } "Endpoints only" }
                }
                if let Some(node) = selected() {
                    div { class: "map-popup-panel",
                        p { class: "text-sm font-medium", "{node.name}" }
                        p { class: "text-xs text-muted-foreground",
                            "{node.type_label} · {node.enrollment}"
                        }
                        if !node.pubkey.is_empty() {
                            p { class: "mt-1 break-all font-mono text-[0.65rem] text-muted-foreground", "{node.pubkey}" }
                        }
                        Button {
                            kind: ButtonKind::Ghost,
                            onclick: move |_| selected.set(None),
                            "Close"
                        }
                    }
                }
            }
            div { class: "map-node-list",
                for node in rows.iter().filter(|n| n.lat.is_some() && n.lon.is_some()).take(24) {
                    {
                        let name = node.name.clone();
                        let enrollment = node.enrollment.as_str().to_string();
                        let type_label = if enrollment == "hq" {
                            "HQ".to_string()
                        } else {
                            node_type_label(node.contact_type).to_string()
                        };
                        let color = enrollment_color(enrollment.as_str());
                        let pubkey = node.pubkey.clone();
                        let lat = node.lat.unwrap_or(0.0);
                        let lon = node.lon.unwrap_or(0.0);
                        rsx! {
                            button {
                                key: "{pubkey}",
                                class: "map-node-row",
                                onclick: move |_| {
                                    selected.set(Some(SelectedNode {
                                        name: name.clone(),
                                        enrollment: enrollment.clone(),
                                        type_label: type_label.clone(),
                                        pubkey: pubkey.clone(),
                                    }));
                                    spawn(async move { map_fly_to(lon, lat, 12.0).await; });
                                },
                                span { class: "map-swatch", style: "background:{color};" }
                                span { class: "min-w-0 flex-1 truncate text-left text-sm", "{name}" }
                                span { class: "shrink-0 text-[0.65rem] text-muted-foreground", "{type_label}" }
                                span { class: "font-mono text-[0.65rem] text-muted-foreground", "{lat:.3}, {lon:.3}" }
                            }
                        }
                    }
                }
            }
        }
    }
}
