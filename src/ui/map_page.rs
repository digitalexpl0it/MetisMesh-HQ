//! Operational MapLibre map (desktop host) with mesh + RF overlays.

use dioxus::prelude::*;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

use crate::contact_qr::{contact_add_uri, enqueue_contact_qr, qr_public_url};
use crate::db::unix_now;
use crate::path::{build_line, MapLine};
use crate::pmtiles_path;
use crate::radio::SyncPhase;

use super::icons::{Icon, IconName};
use super::map_host::{
    dark_basemap_style, load_saved_basemap, load_saved_camera, map_bind_node_clicks, map_destroy,
    map_ease_to, map_exists, map_fit_bounds, map_fly_to, map_get_camera, map_init,
    map_pulse_at, map_set_geojson, map_camera_from_global, map_clear_region_draft,
    map_is_moving,
    map_place_region_draft, map_region_draft_seq, map_set_nodes, map_set_style,
    map_set_visibility, map_snapshot_region_draft, map_take_favorite_toggle,
    map_take_selection, region_draft_from_placement, save_basemap, save_camera,
    BasemapKind, MapCamera, RegionDraftConfirm, MAP_CONTAINER_ID,
};
use super::state;
use super::widgets::{Button, ButtonKind, PageHeading, SelectInput};

#[derive(Clone, Copy, PartialEq, Eq)]
enum NodeFilter {
    All,
    Tracked,
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum TrailWindow {
    Hour,
    SixHours,
    Day,
    All,
}

/// Position older than this is drawn faded on the map (tracked movers).
const STALE_AFTER_SECS: i64 = 30 * 60;

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

fn default_region_name(confirm: &RegionDraftConfirm) -> String {
    if confirm.mode == "adjust" {
        if let Some(name) = crate::map_tiles::active_region_id(&state().db).and_then(|id| {
            crate::map_tiles::list_regions(&state().db)
                .into_iter()
                .find(|r| r.id == id)
                .map(|r| r.name)
        }) {
            if !name.trim().is_empty() {
                return name;
            }
        }
    }
    format!("Map area · {:.0} mi", confirm.radius_miles)
}

fn begin_name_step(
    confirm: RegionDraftConfirm,
    mut region_name_input: Signal<String>,
    mut pending_region: Signal<Option<RegionDraftConfirm>>,
    mut draft_placed: Signal<bool>,
    mut show_download: Signal<bool>,
    mut dl_note: Signal<String>,
    mut tick: Signal<u32>,
) {
    region_name_input.set(default_region_name(&confirm));
    pending_region.set(Some(confirm));
    // Leave the square on the map; draft_placed stays false so the name step
    // shows instead of the confirm buttons.
    draft_placed.set(false);
    show_download.set(true);
    dl_note.set(String::new());
    tick.with_mut(|n| *n = n.wrapping_add(1));
}

fn placement_center_fallback() -> (f64, f64) {
    if let Some(cam) = load_saved_camera() {
        return (cam.lng, cam.lat);
    }
    if let Some((_, lat, lon)) = state().db.hq_position().ok().flatten() {
        return (lon, lat);
    }
    // Last resort only — bridge ignores this when useCoords is false.
    (-118.24, 34.05)
}

fn commit_named_region(
    mut pending_region: Signal<Option<RegionDraftConfirm>>,
    region_name_input: Signal<String>,
    mut show_download: Signal<bool>,
    mut dl_note: Signal<String>,
    mut tick: Signal<u32>,
    mut preserve_camera: Signal<Option<MapCamera>>,
    mut basemap: Signal<BasemapKind>,
    mut style_key: Signal<u32>,
) {
    let name = region_name_input().trim().to_string();
    if name.is_empty() || crate::map_tiles::download_status().running {
        return;
    }
    let Some(confirm) = pending_region() else {
        return;
    };
    let db = state().db.clone();
    let replace = if confirm.mode == "adjust" {
        crate::map_tiles::active_region_id(&db)
    } else {
        None
    };
    pending_region.set(None);
    show_download.set(true);
    dl_note.set(format!("Downloading tiles for “{name}”…"));
    crate::map_tiles::start_named_bbox_download(
        db,
        name,
        confirm.bbox,
        replace,
        confirm.shape.clone(),
        confirm.lng,
        confirm.lat,
        confirm.radius_miles,
    );
    spawn(async move {
        map_clear_region_draft().await;
    });
    spawn(async move {
        for _ in 0..180 {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            tick.with_mut(|n| *n = n.wrapping_add(1));
            let s = crate::map_tiles::download_status();
            if !s.running {
                dl_note.set(s.detail.clone());
                if s.phase == "done" {
                    preserve_camera.set(map_get_camera().await);
                    if *basemap.peek() == BasemapKind::Outline {
                        basemap.set(BasemapKind::Streets);
                    }
                    style_key.with_mut(|n| *n = n.wrapping_add(1));
                    crate::toast::push(
                        crate::toast::ToastKind::Success,
                        "Offline map pack ready.",
                    );
                } else if s.phase == "error" {
                    crate::toast::push(crate::toast::ToastKind::Error, s.detail.clone());
                }
                break;
            }
            dl_note.set(s.detail.clone());
        }
    });
}

struct MapModel {
    nodes: Value,
    solid: Value,
    dashed: Value,
    snr: Value,
    noise: Value,
    trails: Value,
    note: String,
    bounds: Option<(f64, f64, f64, f64)>,
    pmtiles_ok: bool,
}

fn build_model(
    filter: NodeFilter,
    window: TrafficWindow,
    trail_window: TrailWindow,
    show_solid: bool,
    show_dashed: bool,
    show_snr: bool,
    show_noise: bool,
    favorites: &HashSet<String>,
    tracked: &HashSet<String>,
    basemap: BasemapKind,
) -> MapModel {
    let db = &state().db;
    let path_nodes = db.known_nodes_for_paths().unwrap_or_default();
    let hq = db.hq_position().ok().flatten();
    let rows = db.list_nodes().unwrap_or_default();
    let rf_by_key = db.latest_rf_by_pubkey().unwrap_or_default();
    let now = unix_now();

    let node_features: Vec<Value> = rows
        .iter()
        .filter_map(|node| {
            let lat = node.lat?;
            let lon = node.lon?;
            let enrollment = node.enrollment.as_str();
            let keep = match filter {
                NodeFilter::All => true,
                NodeFilter::Tracked => {
                    tracked.contains(&node.pubkey) || enrollment == "hq"
                }
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
            let age = node
                .last_advert
                .or(node.last_heard)
                .map(|ts| (now - ts).max(0));
            let stale = tracked.contains(&node.pubkey)
                && age.map(|a| a > STALE_AFTER_SECS).unwrap_or(true);
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
                    "stale": stale,
                    "tracked": tracked.contains(&node.pubkey),
                    "radius": if enrollment == "hq" { 11 } else { 9 },
                },
                "geometry": { "type": "Point", "coordinates": [lon, lat] }
            }))
        })
        .collect();

    let trail_since = match trail_window {
        TrailWindow::Hour => Some(now - 3600),
        TrailWindow::SixHours => Some(now - 6 * 3600),
        TrailWindow::Day => Some(now - 86_400),
        TrailWindow::All => None,
    };
    let trail_keys: Vec<String> = if filter == NodeFilter::Tracked || !tracked.is_empty() {
        tracked.iter().cloned().collect()
    } else {
        Vec::new()
    };
    let show_trails = filter == NodeFilter::Tracked && !trail_keys.is_empty();
    let trail_features: Vec<Value> = if show_trails {
        let points = db
            .positions_for_pubkeys(&trail_keys, trail_since)
            .unwrap_or_default();
        let mut by_key: HashMap<String, Vec<(f64, f64)>> = HashMap::new();
        for p in points {
            if p.lat == 0.0 && p.lon == 0.0 {
                continue;
            }
            by_key
                .entry(p.pubkey)
                .or_default()
                .push((p.lon, p.lat));
        }
        by_key
            .into_iter()
            .filter_map(|(key, coords)| {
                if coords.len() < 2 {
                    return None;
                }
                let node = rows.iter().find(|n| n.pubkey == key);
                let color = node
                    .map(|n| type_color(n.enrollment.as_str(), n.contact_type))
                    .unwrap_or("#38bdf8");
                Some(json!({
                    "type": "Feature",
                    "properties": { "pubkey": key, "color": color },
                    "geometry": {
                        "type": "LineString",
                        "coordinates": coords,
                    }
                }))
            })
            .collect()
    } else {
        Vec::new()
    };

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
    for feat in &node_features {
        let Some(coords) = feat
            .pointer("/geometry/coordinates")
            .and_then(|c| c.as_array())
        else {
            continue;
        };
        if coords.len() < 2 {
            continue;
        }
        let Some(lon) = coords[0].as_f64() else {
            continue;
        };
        let Some(lat) = coords[1].as_f64() else {
            continue;
        };
        any = true;
        west = west.min(lon);
        east = east.max(lon);
        south = south.min(lat);
        north = north.max(lat);
    }
    for line in &trail_features {
        let Some(coords) = line
            .pointer("/geometry/coordinates")
            .and_then(|c| c.as_array())
        else {
            continue;
        };
        for pt in coords {
            let Some(pair) = pt.as_array() else {
                continue;
            };
            if pair.len() < 2 {
                continue;
            }
            let Some(lon) = pair[0].as_f64() else {
                continue;
            };
            let Some(lat) = pair[1].as_f64() else {
                continue;
            };
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
    let note = if filter == NodeFilter::Tracked && tracked.is_empty() {
        "Tracker list is empty. Add devices under Tracker — they must share location to move.".into()
    } else if node_features.is_empty() {
        "No GPS fixes yet. Positions appear when a node advertises coordinates.".into()
    } else if filter == NodeFilter::Tracked {
        format!(
            "{} tracked on map · {} trail(s){}{}",
            node_features.len(),
            trail_features.len(),
            if trail_features.is_empty() {
                " · trails grow after position updates"
            } else {
                ""
            },
            basemap_note
        )
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
        trails: collection(trail_features),
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

fn trail_layers() -> Value {
    json!([{
        "id": "track-trails",
        "type": "line",
        "source": "track-trails",
        "paint": {
            "line-color": ["coalesce", ["get", "color"], "#38bdf8"],
            "line-width": 3,
            "line-opacity": 0.75
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
    let mut trail_window = use_signal(|| TrailWindow::Day);
    // Soft-follow a tracked pubkey (empty = off).
    let mut follow_pubkey = use_signal(String::new);
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
    let mut pending_region = use_signal(|| None::<RegionDraftConfirm>);
    let mut live_draft = use_signal(|| None::<RegionDraftConfirm>);
    let mut draft_placed = use_signal(|| false);
    let mut region_name_input = use_signal(String::new);
    let mut dl_note = use_signal(String::new);

    let _ = tick();
    let tracked = state()
        .db
        .tracked_pubkey_set()
        .unwrap_or_default();
    let model = build_model(
        filter(),
        window(),
        trail_window(),
        show_solid(),
        show_dashed(),
        show_snr(),
        show_noise(),
        &favorites(),
        &tracked,
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
            let coverage = crate::map_tiles::active_coverage(&super::state().db);
            let style = dark_basemap_style(tiles.as_deref(), kind, coverage.as_ref());
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
            trail_window(),
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
        let tracked_now = state()
            .db
            .tracked_pubkey_set()
            .unwrap_or_default();
        // Peek basemap — style swaps own the re-push via tick bump after setStyle.
        let snapshot = build_model(
            filter(),
            window(),
            trail_window(),
            show_solid(),
            show_dashed(),
            show_snr(),
            show_noise(),
            &favorites(),
            &tracked_now,
            *basemap.peek(),
        );
        let nodes = snapshot.nodes.clone();
        let solid = snapshot.solid.clone();
        let dashed = snapshot.dashed.clone();
        let snr = snapshot.snr.clone();
        let noise = snapshot.noise.clone();
        let trails = snapshot.trails.clone();
        let bounds = snapshot.bounds;
        let do_fit = !fitted() && bounds.is_some();
        let do_cluster = cluster();
        spawn(async move {
            map_set_nodes(&nodes, do_cluster).await;
            map_set_geojson("solid-routes", &solid, &solid_layers()).await;
            map_set_geojson("dashed-routes", &dashed, &dashed_layers()).await;
            map_set_geojson("snr-points", &snr, &snr_layers()).await;
            map_set_geojson("noise-points", &noise, &noise_layers()).await;
            map_set_geojson("track-trails", &trails, &trail_layers()).await;
            // GL pin/label layers (HTML markers removed).
            map_bind_node_clicks("node-pins").await;
            map_bind_node_clicks("node-labels").await;
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
            let mut last_draft_seq: u64 = 0;
            // Seed with current max message id so we only sonar *new* traffic.
            let mut last_message_id: i64 = 0;
            let mut message_id_seeded = false;
            // Last known lat/lon for tracked movers — pulse on real position change.
            let mut last_tracked_pos: HashMap<String, (f64, f64)> = HashMap::new();
            let mut tracked_pos_seeded = false;
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
                // Camera from moveend global — skip getCenter while dragging.
                cam_ticks = cam_ticks.wrapping_add(1);
                if cam_ticks % 14 == 0 && !map_is_moving().await {
                    if let Some(cam) = map_camera_from_global().await {
                        save_camera(cam);
                    }
                }
                // ~1s: message sonar + tracked-move sonar.
                if cam_ticks % 7 == 0 && ready() {
                    let recent = state()
                        .db
                        .recent_messages(40)
                        .unwrap_or_default();
                    if !message_id_seeded {
                        last_message_id = recent.iter().map(|m| m.id).max().unwrap_or(0);
                        message_id_seeded = true;
                    } else {
                        let mut fresh: Vec<_> =
                            recent.into_iter().filter(|m| m.id > last_message_id).collect();
                        if !fresh.is_empty() {
                            fresh.sort_by_key(|m| m.id);
                            if let Some(max_id) = fresh.last().map(|m| m.id) {
                                last_message_id = max_id;
                            }
                            let hq = state().db.hq_position().ok().flatten();
                            let nodes = state().db.list_nodes().unwrap_or_default();
                            let mut pulsed = 0usize;
                            for msg in fresh {
                                if pulsed >= 8 {
                                    break;
                                }
                                let pulse = if msg.direction == "tx" {
                                    hq.as_ref().map(|(_, lat, lon)| {
                                        (*lon, *lat, "#f472b6")
                                    })
                                } else {
                                    msg.sender_pubkey.as_ref().and_then(|key| {
                                        nodes.iter().find(|n| n.pubkey == *key).and_then(
                                            |n| match (n.lat, n.lon) {
                                                (Some(lat), Some(lon))
                                                    if lat != 0.0 || lon != 0.0 =>
                                                {
                                                    Some((
                                                        lon,
                                                        lat,
                                                        type_color(
                                                            n.enrollment.as_str(),
                                                            n.contact_type,
                                                        ),
                                                    ))
                                                }
                                                _ => None,
                                            },
                                        )
                                    })
                                };
                                if let Some((lon, lat, color)) = pulse {
                                    map_pulse_at(lon, lat, Some(color), "message").await;
                                    pulsed += 1;
                                }
                            }
                        }
                    }

                    let tracked_keys = state()
                        .db
                        .tracked_pubkey_set()
                        .unwrap_or_default();
                    if !tracked_keys.is_empty() {
                        let nodes = state().db.list_nodes().unwrap_or_default();
                        if !tracked_pos_seeded {
                            for node in &nodes {
                                if !tracked_keys.contains(&node.pubkey) {
                                    continue;
                                }
                                if let (Some(lat), Some(lon)) = (node.lat, node.lon) {
                                    if lat != 0.0 || lon != 0.0 {
                                        last_tracked_pos
                                            .insert(node.pubkey.clone(), (lat, lon));
                                    }
                                }
                            }
                            tracked_pos_seeded = true;
                        } else {
                            let mut move_pulses = 0usize;
                            for node in &nodes {
                                if !tracked_keys.contains(&node.pubkey) {
                                    continue;
                                }
                                let (Some(lat), Some(lon)) = (node.lat, node.lon) else {
                                    continue;
                                };
                                if lat == 0.0 && lon == 0.0 {
                                    continue;
                                }
                                let next = (lat, lon);
                                match last_tracked_pos.get(&node.pubkey) {
                                    Some(prev) if *prev == next => {}
                                    Some(_) => {
                                        last_tracked_pos
                                            .insert(node.pubkey.clone(), next);
                                        if move_pulses < 6 {
                                            map_pulse_at(
                                                lon,
                                                lat,
                                                Some(type_color(
                                                    node.enrollment.as_str(),
                                                    node.contact_type,
                                                )),
                                                "move",
                                            )
                                            .await;
                                            move_pulses += 1;
                                        }
                                        let follow = follow_pubkey();
                                        if !follow.is_empty()
                                            && follow == node.pubkey
                                            && !map_is_moving().await
                                        {
                                            map_ease_to(lon, lat).await;
                                        }
                                    }
                                    None => {
                                        last_tracked_pos
                                            .insert(node.pubkey.clone(), next);
                                    }
                                }
                            }
                        }
                    }
                }
                // Region draft: only when placing and JS seq advanced (mouseup).
                if draft_placed() {
                    let seq = map_region_draft_seq().await;
                    if seq != last_draft_seq {
                        last_draft_seq = seq;
                        if let Some(live) = map_snapshot_region_draft().await {
                            live_draft.set(Some(live));
                        }
                    }
                }
            }
        });
    });

    use_effect(move || {
        spawn(async move {
            loop {
                // Idle overlay refresh — skip mid-pan so eval does not fight drag.
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                if map_is_moving().await {
                    continue;
                }
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

    // Route/SNR/noise capture needs a live companion; Fit/cluster still work offline.
    let radio = state().radio.snapshot();
    let radio_linked = radio.state == "connected"
        && matches!(radio.sync_phase, SyncPhase::Ready | SyncPhase::Idle);
    let rf_title = if radio_linked {
        "Toggle mesh RF overlays from recent traffic / samples"
    } else {
        "Connect a MeshCore companion to capture routes, SNR, and noise"
    };

    rsx! {
        document::Stylesheet { href: asset!("/assets/maplibre-gl.css") }
        document::Script { src: asset!("/assets/maplibre-gl.js") }
        document::Script { src: asset!("/assets/pmtiles.js") }
        document::Script { src: asset!("/assets/map_bridge.js") }
        div { class: "map-page flex h-full min-h-0 flex-col gap-3 overflow-hidden",
            div { class: "map-heading-row",
                div { class: "map-heading-copy",
                    PageHeading { icon: IconName::Map, title: "Map".to_string(), "{model.note}" }
                }
                div { class: "map-overlay-controls",
                    label { class: "map-toggle",
                        input { r#type: "checkbox", checked: cluster(), oninput: move |_| cluster.set(!cluster()) }
                        "Cluster markers"
                    }
                    label {
                        class: if radio_linked { "map-toggle" } else { "map-toggle map-toggle-disabled" },
                        title: "{rf_title}",
                        input {
                            r#type: "checkbox",
                            checked: show_solid(),
                            disabled: !radio_linked,
                            oninput: move |_| {
                                if radio_linked {
                                    show_solid.set(!show_solid());
                                }
                            },
                        }
                        "Solid routes"
                    }
                    label {
                        class: if radio_linked { "map-toggle" } else { "map-toggle map-toggle-disabled" },
                        title: "{rf_title}",
                        input {
                            r#type: "checkbox",
                            checked: show_dashed(),
                            disabled: !radio_linked,
                            oninput: move |_| {
                                if radio_linked {
                                    show_dashed.set(!show_dashed());
                                }
                            },
                        }
                        "Dashed routes"
                    }
                    label {
                        class: if radio_linked { "map-toggle" } else { "map-toggle map-toggle-disabled" },
                        title: "{rf_title}",
                        input {
                            r#type: "checkbox",
                            checked: show_snr(),
                            disabled: !radio_linked,
                            oninput: move |_| {
                                if radio_linked {
                                    show_snr.set(!show_snr());
                                }
                            },
                        }
                        "SNR"
                    }
                    label {
                        class: if radio_linked { "map-toggle" } else { "map-toggle map-toggle-disabled" },
                        title: "{rf_title}",
                        input {
                            r#type: "checkbox",
                            checked: show_noise(),
                            disabled: !radio_linked,
                            oninput: move |_| {
                                if radio_linked {
                                    show_noise.set(!show_noise());
                                }
                            },
                        }
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
                    span {
                        title: if radio_linked {
                            "Poll GPS contacts for noise / RSSI / SNR samples"
                        } else {
                            "Connect a MeshCore companion to probe noise"
                        },
                        Button {
                            kind: ButtonKind::Ghost,
                            disabled: !radio_linked,
                            onclick: move |_| {
                                if radio_linked {
                                    state().radio.probe_noise_samples();
                                }
                            },
                            Icon { name: IconName::Signal, size: 14 }
                            "Probe noise"
                        }
                    }
                }
            }
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
                            "Outline works offline. Download a local area (place + radius) for Streets / Terrain / Topo."
                        }
                    }
                } else {
                    rsx! {}
                }
            }
            div { class: "map-toolbar",
                div { class: "flex flex-wrap gap-2",
                    button {
                        class: filter_chip(filter() == NodeFilter::All),
                        onclick: move |_| {
                            filter.set(NodeFilter::All);
                            follow_pubkey.set(String::new());
                        },
                        "All"
                    }
                    button {
                        class: filter_chip(filter() == NodeFilter::Tracked),
                        title: "Tracked whitelist + HQ (trails from position history)",
                        onclick: move |_| {
                            filter.set(NodeFilter::Tracked);
                            fitted.set(false);
                        },
                        "Tracked"
                    }
                    button {
                        class: filter_chip(filter() == NodeFilter::Favorites),
                        onclick: move |_| {
                            filter.set(NodeFilter::Favorites);
                            follow_pubkey.set(String::new());
                        },
                        "Favorites"
                    }
                    button {
                        class: filter_chip(filter() == NodeFilter::Users),
                        onclick: move |_| {
                            filter.set(NodeFilter::Users);
                            follow_pubkey.set(String::new());
                        },
                        "Users"
                    }
                    button {
                        class: filter_chip(filter() == NodeFilter::Repeaters),
                        onclick: move |_| {
                            filter.set(NodeFilter::Repeaters);
                            follow_pubkey.set(String::new());
                        },
                        "Repeaters"
                    }
                    button {
                        class: filter_chip(filter() == NodeFilter::Rooms),
                        onclick: move |_| {
                            filter.set(NodeFilter::Rooms);
                            follow_pubkey.set(String::new());
                        },
                        "Rooms"
                    }
                    button {
                        class: filter_chip(filter() == NodeFilter::Sensors),
                        onclick: move |_| {
                            filter.set(NodeFilter::Sensors);
                            follow_pubkey.set(String::new());
                        },
                        "Sensors"
                    }
                    button {
                        class: filter_chip(filter() == NodeFilter::Enrolled),
                        onclick: move |_| {
                            filter.set(NodeFilter::Enrolled);
                            follow_pubkey.set(String::new());
                        },
                        "Enrolled"
                    }
                    button {
                        class: filter_chip(filter() == NodeFilter::Heard),
                        onclick: move |_| {
                            filter.set(NodeFilter::Heard);
                            follow_pubkey.set(String::new());
                        },
                        "Heard"
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
                if filter() == NodeFilter::Tracked {
                    div { class: "min-w-[9rem]",
                        SelectInput {
                            id: "map-trail-window".to_string(),
                            value: match trail_window() {
                                TrailWindow::Hour => "1h".to_string(),
                                TrailWindow::SixHours => "6h".to_string(),
                                TrailWindow::Day => "24h".to_string(),
                                TrailWindow::All => "all".to_string(),
                            },
                            onchange: move |event: FormEvent| {
                                trail_window.set(match event.value().as_str() {
                                    "1h" => TrailWindow::Hour,
                                    "6h" => TrailWindow::SixHours,
                                    "all" => TrailWindow::All,
                                    _ => TrailWindow::Day,
                                });
                            },
                            option { value: "1h", style: "background-color:#101418;color:#f4f4f5;", "Trails: 1 hour" }
                            option { value: "6h", style: "background-color:#101418;color:#f4f4f5;", "Trails: 6 hours" }
                            option { value: "24h", style: "background-color:#101418;color:#f4f4f5;", "Trails: 24 hours" }
                            option { value: "all", style: "background-color:#101418;color:#f4f4f5;", "Trails: all" }
                        }
                    }
                    {
                        let follow_opts: Vec<(String, String)> = {
                            let nodes = state().db.list_nodes().unwrap_or_default();
                            let mut rows = state()
                                .db
                                .list_tracked_nodes()
                                .unwrap_or_default();
                            rows.sort_by(|a, b| {
                                a.label.to_lowercase().cmp(&b.label.to_lowercase())
                            });
                            rows.into_iter()
                                .map(|t| {
                                    let label = nodes
                                        .iter()
                                        .find(|n| n.pubkey == t.pubkey)
                                        .map(|n| {
                                            if n.name.trim().is_empty() {
                                                n.prefix.clone()
                                            } else {
                                                n.name.clone()
                                            }
                                        })
                                        .filter(|s| !s.is_empty())
                                        .unwrap_or_else(|| {
                                            if t.label.is_empty() {
                                                t.pubkey.chars().take(12).collect()
                                            } else {
                                                t.label
                                            }
                                        });
                                    (t.pubkey, label)
                                })
                                .collect()
                        };
                        let follow_value = follow_pubkey();
                        rsx! {
                            div { class: "min-w-[11rem]",
                                SelectInput {
                                    id: "map-follow-node".to_string(),
                                    value: follow_value.clone(),
                                    onchange: move |event: FormEvent| {
                                        follow_pubkey.set(event.value());
                                    },
                                    option {
                                        value: "",
                                        style: "background-color:#101418;color:#f4f4f5;",
                                        "Follow: off"
                                    }
                                    for (key, label) in follow_opts {
                                        option {
                                            value: "{key}",
                                            style: "background-color:#101418;color:#f4f4f5;",
                                            "Follow: {label}"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            div { class: "map-stage",
                div {
                    id: "{MAP_CONTAINER_ID}",
                    class: "map-canvas",
                }
                if show_download()
                    || pending_region().is_some()
                    || draft_placed()
                    || !model.pmtiles_ok
                    || crate::map_tiles::download_status().running
                {
                    div { class: "map-download-card",
                        div { class: "map-download-head",
                            span { class: "map-download-title",
                                if pending_region().is_some() {
                                    "Name region"
                                } else if draft_placed() {
                                    "Confirm area"
                                } else {
                                    "Download offline map"
                                }
                            }
                            if model.pmtiles_ok
                                && !crate::map_tiles::download_status().running
                                && pending_region().is_none()
                                && !draft_placed()
                            {
                                button {
                                    class: "map-download-dismiss",
                                    title: "Dismiss",
                                    onclick: move |_| show_download.set(false),
                                    "×"
                                }
                            }
                        }
                        if pending_region().is_some() {
                            // Naming step only.
                            div { class: "map-region-step",
                                input {
                                    class: "map-download-input map-download-input-wide map-region-name-input",
                                    r#type: "text",
                                    maxlength: "60",
                                    placeholder: "Region name",
                                    value: "{region_name_input}",
                                    oninput: move |event: FormEvent| region_name_input.set(event.value()),
                                    onkeydown: move |event: KeyboardEvent| {
                                        if event.key() == Key::Enter {
                                            commit_named_region(
                                                pending_region,
                                                region_name_input,
                                                show_download,
                                                dl_note,
                                                tick,
                                                preserve_camera,
                                                basemap,
                                                style_key,
                                            );
                                        }
                                    },
                                }
                                div { class: "map-download-row map-download-actions",
                                    button {
                                        class: "map-download-go map-download-go-wide",
                                        disabled: crate::map_tiles::download_status().running
                                            || region_name_input().trim().is_empty(),
                                        onclick: move |_| {
                                            commit_named_region(
                                                pending_region,
                                                region_name_input,
                                                show_download,
                                                dl_note,
                                                tick,
                                                preserve_camera,
                                                basemap,
                                                style_key,
                                            );
                                        },
                                        Icon { name: IconName::Check, size: 14 }
                                        "Download"
                                    }
                                    button {
                                        class: "map-download-go map-download-go-ghost",
                                        disabled: crate::map_tiles::download_status().running,
                                        onclick: move |_| {
                                            pending_region.set(None);
                                            live_draft.set(None);
                                            dl_note.set(String::new());
                                            spawn(async move {
                                                map_clear_region_draft().await;
                                            });
                                        },
                                        "Cancel"
                                    }
                                }
                            }
                        } else if draft_placed() {
                            // Confirm step only.
                            div { class: "map-region-step",
                                div { class: "map-download-row map-download-actions",
                                    button {
                                        class: "map-download-go map-download-go-wide",
                                        disabled: crate::map_tiles::download_status().running,
                                        onclick: move |_| {
                                            spawn(async move {
                                                // Painted ring only — never the Place/camera live_draft.
                                                // That fallback was saving the map center instead of the
                                                // square the operator dragged.
                                                let Some(confirm) =
                                                    map_snapshot_region_draft().await
                                                else {
                                                    crate::toast::push(
                                                        crate::toast::ToastKind::Error,
                                                        "No region on the map — place one first.",
                                                    );
                                                    return;
                                                };
                                                live_draft.set(Some(confirm.clone()));
                                                if crate::map_tiles::download_status().running {
                                                    crate::toast::push(
                                                        crate::toast::ToastKind::Warn,
                                                        "Wait for the current map download to finish.",
                                                    );
                                                } else {
                                                    // Keep the square on the map through naming so what
                                                    // you confirm is what downloads.
                                                    begin_name_step(
                                                        confirm,
                                                        region_name_input,
                                                        pending_region,
                                                        draft_placed,
                                                        show_download,
                                                        dl_note,
                                                        tick,
                                                    );
                                                }
                                            });
                                        },
                                        Icon { name: IconName::Check, size: 14 }
                                        "Confirm"
                                    }
                                    button {
                                        class: "map-download-go map-download-go-ghost",
                                        disabled: crate::map_tiles::download_status().running,
                                        onclick: move |_| {
                                            draft_placed.set(false);
                                            live_draft.set(None);
                                            spawn(async move {
                                                map_clear_region_draft().await;
                                                dl_note.set(String::new());
                                            });
                                        },
                                        "Cancel"
                                    }
                                }
                            }
                        } else {
                        // Full tools — hidden during confirm / name steps.
                        div { class: "map-download-radii",
                            for miles in [25u32, 50, 75, 100, 150, 200, 250, 300] {
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
                        div { class: "map-download-row map-download-place-row",
                            button {
                                class: "map-download-go",
                                disabled: crate::map_tiles::download_status().running,
                                onclick: move |_| {
                                    let radius = radius_miles() as f64;
                                    spawn(async move {
                                        // Prefer live map center inside the bridge. Only pass
                                        // coords when we actually have a camera/HQ reading.
                                        let coords = map_get_camera()
                                            .await
                                            .map(|c| (c.lng, c.lat));
                                        let (lng, lat) = match coords {
                                            Some((lng, lat)) => (Some(lng), Some(lat)),
                                            None => (None, None),
                                        };
                                        map_place_region_draft(
                                            "rectangle",
                                            radius,
                                            lng,
                                            lat,
                                            "new",
                                        )
                                        .await;
                                        // Refresh from JS so Rust matches the painted square
                                        // (map center), not a stale fallback.
                                        if let Some(live) = map_snapshot_region_draft().await {
                                            live_draft.set(Some(live));
                                        } else {
                                            let (flng, flat) = placement_center_fallback();
                                            live_draft.set(Some(region_draft_from_placement(
                                                "rectangle",
                                                radius,
                                                lng.unwrap_or(flng),
                                                lat.unwrap_or(flat),
                                                "new",
                                            )));
                                        }
                                        draft_placed.set(true);
                                        pending_region.set(None);
                                        show_download.set(true);
                                        dl_note.set(String::new());
                                        tick.with_mut(|n| *n = n.wrapping_add(1));
                                    });
                                },
                                "Place on map"
                            }
                            if model.pmtiles_ok {
                                button {
                                    class: "map-download-go map-download-go-secondary",
                                    disabled: crate::map_tiles::download_status().running,
                                    onclick: move |_| {
                                        let radius = radius_miles() as f64;
                                        let coverage = crate::map_tiles::active_coverage_bbox(&state().db);
                                        spawn(async move {
                                            let coords = if let Some([w, s, e, n]) = coverage {
                                                Some(((w + e) / 2.0, (s + n) / 2.0))
                                            } else {
                                                map_get_camera().await.map(|c| (c.lng, c.lat))
                                            };
                                            let (lng, lat) = match coords {
                                                Some((lng, lat)) => (Some(lng), Some(lat)),
                                                None => (None, None),
                                            };
                                            map_place_region_draft(
                                                "rectangle",
                                                radius,
                                                lng,
                                                lat,
                                                "adjust",
                                            )
                                            .await;
                                            if let Some(live) = map_snapshot_region_draft().await {
                                                live_draft.set(Some(live));
                                            } else {
                                                let (flng, flat) = placement_center_fallback();
                                                live_draft.set(Some(region_draft_from_placement(
                                                    "rectangle",
                                                    radius,
                                                    lng.unwrap_or(flng),
                                                    lat.unwrap_or(flat),
                                                    "adjust",
                                                )));
                                            }
                                            draft_placed.set(true);
                                            pending_region.set(None);
                                            show_download.set(true);
                                            dl_note.set(String::new());
                                            tick.with_mut(|n| *n = n.wrapping_add(1));
                                        });
                                    },
                                    "Adjust pack"
                                }
                            }
                            button {
                                class: "map-download-go map-download-go-ghost",
                                disabled: crate::map_tiles::download_status().running,
                                onclick: move |_| {
                                    draft_placed.set(false);
                                    live_draft.set(None);
                                    pending_region.set(None);
                                    spawn(async move {
                                        map_clear_region_draft().await;
                                        dl_note.set(String::new());
                                    });
                                },
                                "Clear"
                            }
                        }
                        div { class: "map-download-row",
                            input {
                                class: "map-download-input map-download-input-wide",
                                r#type: "text",
                                maxlength: "80",
                                placeholder: "City or postal code",
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
                        if crate::map_tiles::download_status().running {
                            p { class: "map-download-status",
                                "{crate::map_tiles::download_status().detail}"
                            }
                        }
                        {
                            let packs = crate::map_tiles::list_installed_regions(&state().db);
                            let active = crate::map_tiles::active_region_id(&state().db)
                                .unwrap_or_default();
                            let dl_busy = crate::map_tiles::download_status().running;
                            rsx! {
                                div { class: "map-download-packs",
                                    p { class: "map-download-packs-title", "Installed packs" }
                                    if packs.is_empty() {
                                        p { class: "map-download-packs-empty",
                                            "None yet — download one above."
                                        }
                                    } else {
                                        div { class: "map-download-packs-list",
                                            for region in packs.into_iter() {
                                                {
                                                    let id = region.id.clone();
                                                    let id_use = id.clone();
                                                    let id_del = id.clone();
                                                    let is_active = active == region.id;
                                                    let name = region.name.clone();
                                                    let status = region.status.clone();
                                                    let mb = if region.bytes > 0 {
                                                        format!(" · {} MB", region.bytes / 1_000_000)
                                                    } else {
                                                        String::new()
                                                    };
                                                    rsx! {
                                                        div {
                                                            key: "{id}",
                                                            class: if is_active {
                                                                "map-download-pack-row active"
                                                            } else {
                                                                "map-download-pack-row"
                                                            },
                                                            button {
                                                                class: "map-download-pack-main",
                                                                disabled: dl_busy,
                                                                title: "Use this pack",
                                                                onclick: move |_| {
                                                                    let _ = crate::map_tiles::set_active_region(
                                                                        &state().db,
                                                                        &id_use,
                                                                    );
                                                                    tick.with_mut(|n| *n = n.wrapping_add(1));
                                                                    spawn(async move {
                                                                        preserve_camera.set(map_get_camera().await);
                                                                        style_key.with_mut(|n| *n = n.wrapping_add(1));
                                                                    });
                                                                },
                                                                span { class: "map-download-pack-name", "{name}" }
                                                                span { class: "map-download-pack-meta",
                                                                    if is_active { "Active · " } else { "" }
                                                                    "{status}{mb}"
                                                                }
                                                            }
                                                            button {
                                                                class: "map-download-pack-delete",
                                                                disabled: dl_busy,
                                                                title: "Delete pack",
                                                                onclick: {
                                                                    let active_now = active.clone();
                                                                    move |_| {
                                                                        let was_active = active_now == id_del;
                                                                        let _ = crate::map_tiles::delete_region(
                                                                            &state().db,
                                                                            &id_del,
                                                                        );
                                                                        tick.with_mut(|n| *n = n.wrapping_add(1));
                                                                        if was_active {
                                                                            spawn(async move {
                                                                                preserve_camera.set(map_get_camera().await);
                                                                                if crate::pmtiles_path::current_pmtiles_path().is_none() {
                                                                                    basemap.set(BasemapKind::Outline);
                                                                                }
                                                                                style_key.with_mut(|n| *n = n.wrapping_add(1));
                                                                            });
                                                                        }
                                                                        crate::toast::push(
                                                                            crate::toast::ToastKind::Success,
                                                                            "Deleted offline map pack.",
                                                                        );
                                                                    }
                                                                },
                                                                Icon { name: IconName::Trash, size: 12 }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
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
