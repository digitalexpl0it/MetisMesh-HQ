//! Desktop MapLibre host via `document::eval` (dioxus-maplibre is wasm-gated).

use dioxus::prelude::*;
use serde_json::{json, Value};

pub const MAP_ID: &str = "metis-main";
pub const MAP_CONTAINER_ID: &str = "metis-map-container";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BasemapKind {
    Outline,
    Streets,
    Terrain,
    Topo,
}

impl BasemapKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Outline => "outline",
            Self::Streets => "streets",
            Self::Terrain => "terrain",
            Self::Topo => "topo",
        }
    }

    pub fn from_str(value: &str) -> Self {
        match value {
            "streets" => Self::Streets,
            "terrain" => Self::Terrain,
            "topo" => Self::Topo,
            _ => Self::Outline,
        }
    }
}

fn escape_js_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 8);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Evaluate a JS expression and return its value.
///
/// Dioxus desktop runs eval code as the body of an `AsyncFunction`, so a bare
/// `(() => x)()` statement yields `undefined`. The expression must be returned.
async fn eval_value(expr: &str) -> Result<Value, String> {
    document::eval(&format!("return {expr};"))
        .await
        .map_err(|err| format!("{err:?}"))
}

/// Wait until `map_bridge.js` has assigned `window.MetisMap` (scripts load async).
async fn ensure_map_bridge() -> Result<(), String> {
    let js = r#"(async () => {
  var deadline = Date.now() + 20000;
  while (!window.MetisMap) {
    if (Date.now() > deadline) {
      throw new Error(
        "MetisMap bridge missing (maplibregl=" +
          !!window.maplibregl +
          ", pmtiles=" +
          !!window.pmtiles +
          ", scripts=" +
          document.scripts.length +
          ")"
      );
    }
    await new Promise(function (r) { setTimeout(r, 50); });
  }
  return true;
})()"#;
    eval_value(js).await?;
    Ok(())
}

pub async fn map_init(style: &Value, lng: f64, lat: f64, zoom: f64) -> Result<(), String> {
    ensure_map_bridge().await?;
    let style_json = style.to_string();
    let js = format!(
        r#"(async () => {{
  await window.MetisMap.init({id}, {container}, {style}, {lng}, {lat}, {zoom});
  return true;
}})()"#,
        id = escape_js_string(MAP_ID),
        container = escape_js_string(MAP_CONTAINER_ID),
        style = style_json,
        lng = lng,
        lat = lat,
        zoom = zoom,
    );
    eval_value(&js).await?;
    Ok(())
}

pub async fn map_destroy() {
    let js = format!(
        r#"(() => {{
  if (!window.MetisMap) return true;
  // Prefer soft detach (SPA remount). If the document is already hiding
  // (window close), force-destroy so WebKitGTK does not epoxy-assert.
  if (document.hidden && window.MetisMap.destroyAll) {{
    window.MetisMap.destroyAll();
  }} else {{
    window.MetisMap.destroyIfDetached({id});
  }}
  return true;
}})()"#,
        id = escape_js_string(MAP_ID)
    );
    let _ = document::eval(&js).await;
}

pub async fn map_set_geojson(source_id: &str, data: &Value, layers: &Value) {
    let js = format!(
        r#"(() => {{
  if (!window.MetisMap) return false;
  return window.MetisMap.setGeoJson({id}, {source}, {data}, {layers});
}})()"#,
        id = escape_js_string(MAP_ID),
        source = escape_js_string(source_id),
        data = data.to_string(),
        layers = layers.to_string(),
    );
    let _ = document::eval(&js).await;
}

pub async fn map_set_nodes(data: &Value, cluster: bool) {
    let js = format!(
        r#"(() => {{
  if (!window.MetisMap) return false;
  return window.MetisMap.setNodes({id}, {data}, {cluster}, false);
}})()"#,
        id = escape_js_string(MAP_ID),
        data = data.to_string(),
        cluster = if cluster { "true" } else { "false" },
    );
    let _ = document::eval(&js).await;
}

pub async fn map_set_style(style: &Value, camera: Option<MapCamera>) -> Result<(), String> {
    ensure_map_bridge().await?;
    let cam_json = match camera {
        Some(cam) => format!(
            r#"{{"lng":{},"lat":{},"zoom":{}}}"#,
            cam.lng, cam.lat, cam.zoom
        ),
        None => "null".into(),
    };
    let js = format!(
        r#"(async () => {{
  await window.MetisMap.setStyle({id}, {style}, {cam});
  return true;
}})()"#,
        id = escape_js_string(MAP_ID),
        style = style.to_string(),
        cam = cam_json,
    );
    eval_value(&js).await?;
    Ok(())
}

/// A map left behind by a previous visit to the page is not live: its
/// container was removed, so it must be re-initialised rather than restyled.
pub async fn map_exists() -> bool {
    let js = format!(
        r#"(() => !!(window.MetisMap && window.MetisMap.isLive({id}, {container})))()"#,
        id = escape_js_string(MAP_ID),
        container = escape_js_string(MAP_CONTAINER_ID),
    );
    eval_value(&js)
        .await
        .ok()
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

pub async fn map_set_visibility(layer_id: &str, visible: bool) {
    let js = format!(
        r#"(() => {{
  if (!window.MetisMap) return;
  window.MetisMap.setVisibility({id}, {layer}, {visible});
  return true;
}})()"#,
        id = escape_js_string(MAP_ID),
        layer = escape_js_string(layer_id),
        visible = if visible { "true" } else { "false" },
    );
    let _ = document::eval(&js).await;
}

pub async fn map_fit_bounds(west: f64, south: f64, east: f64, north: f64) {
    let js = format!(
        r#"(() => {{
  if (!window.MetisMap) return;
  window.MetisMap.fitBounds({id}, {west}, {south}, {east}, {north}, 56);
  return true;
}})()"#,
        id = escape_js_string(MAP_ID),
        west = west,
        south = south,
        east = east,
        north = north,
    );
    let _ = document::eval(&js).await;
}

pub async fn map_zoom_by(delta: i32) {
    let js = format!(
        r#"(() => {{
  if (!window.MetisMap || !window.MetisMap.zoomBy) return false;
  return window.MetisMap.zoomBy({id}, {delta});
}})()"#,
        id = escape_js_string(MAP_ID),
        delta = delta,
    );
    let _ = document::eval(&js).await;
}

pub async fn map_fly_to(lng: f64, lat: f64, zoom: f64) {
    let js = format!(
        r#"(() => {{
  if (!window.MetisMap) return;
  window.MetisMap.flyTo({id}, {lng}, {lat}, {zoom});
  return true;
}})()"#,
        id = escape_js_string(MAP_ID),
        lng = lng,
        lat = lat,
        zoom = zoom,
    );
    let _ = document::eval(&js).await;
}

/// Soft recenter (keep zoom). No-op while the operator is panning.
pub async fn map_ease_to(lng: f64, lat: f64) {
    let js = format!(
        r#"(() => {{
  if (!window.MetisMap) return false;
  return window.MetisMap.easeTo({id}, {lng}, {lat});
}})()"#,
        id = escape_js_string(MAP_ID),
        lng = lng,
        lat = lat,
    );
    let _ = document::eval(&js).await;
}

pub async fn map_bind_node_clicks(layer_id: &str) {
    let js = format!(
        r#"(() => {{
  if (!window.MetisMap) return;
  window.MetisMap.bindNodeClicks({id}, {layer});
  return true;
}})()"#,
        id = escape_js_string(MAP_ID),
        layer = escape_js_string(layer_id),
    );
    let _ = document::eval(&js).await;
}

pub async fn map_bind_waypoint_clicks(layer_id: &str) {
    let js = format!(
        r#"(() => {{
  if (!window.MetisMap) return;
  window.MetisMap.bindWaypointClicks({id}, {layer});
  return true;
}})()"#,
        id = escape_js_string(MAP_ID),
        layer = escape_js_string(layer_id),
    );
    let _ = document::eval(&js).await;
}

/// Ephemeral sonar rings at a lon/lat (`kind`: message | arrive | move).
pub async fn map_pulse_at(lng: f64, lat: f64, color: Option<&str>, kind: &str) {
    let js = format!(
        r#"(() => {{
  if (!window.MetisMap) return false;
  return window.MetisMap.pulseAt({id}, {lng}, {lat}, {color}, {kind});
}})()"#,
        id = escape_js_string(MAP_ID),
        lng = lng,
        lat = lat,
        color = color
            .map(escape_js_string)
            .unwrap_or_else(|| "null".into()),
        kind = escape_js_string(kind),
    );
    let _ = document::eval(&js).await;
}

async fn eval_string(js: &str) -> Option<String> {
    match eval_value(js).await {
        Ok(value) => {
            let s = value
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| value.to_string().trim_matches('"').to_string());
            if s.is_empty() || s == "null" {
                None
            } else {
                Some(s)
            }
        }
        Err(_) => None,
    }
}

pub async fn map_take_selection() -> Option<String> {
    // Prefer the window global (written by showDetail) — avoids a MetisMap hop.
    eval_string(
        r#"(() => {
  var raw = window.__metisMapSelection || "";
  window.__metisMapSelection = null;
  return raw;
})()"#,
    )
    .await
}

pub async fn map_take_favorite_toggle() -> Option<String> {
    eval_string(
        r#"(() => {
  var raw = window.__metisMapFavoriteToggle || "";
  window.__metisMapFavoriteToggle = null;
  return raw;
})()"#,
    )
    .await
}

/// True while the operator is panning/zooming (JS sets `__metisMapMoving`).
pub async fn map_is_moving() -> bool {
    eval_value(r#"!!window.__metisMapMoving"#)
        .await
        .ok()
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Camera JSON published on moveend/zoomend (no live getCenter call).
pub async fn map_camera_from_global() -> Option<MapCamera> {
    let raw = eval_string(r#"(() => (window.__metisMapCamera || ""))()"#).await?;
    parse_camera_json(&raw)
}

pub async fn map_region_draft_seq() -> u64 {
    eval_string(r#"(() => String(window.__metisRegionDraftSeq || 0))"#)
        .await
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

#[derive(Clone, Debug, PartialEq)]
pub struct RegionDraftConfirm {
    pub shape: String,
    pub mode: String,
    pub radius_miles: f64,
    pub lng: f64,
    pub lat: f64,
    pub bbox: [f64; 4],
}

pub async fn map_place_region_draft(
    shape: &str,
    radius_miles: f64,
    lng: Option<f64>,
    lat: Option<f64>,
    mode: &str,
) {
    // Only pin explicit coords when both are finite; otherwise the bridge uses
    // the live MapLibre center (avoids the old hardcoded LA fallback).
    let coords = match (lng, lat) {
        (Some(lng), Some(lat)) if lng.is_finite() && lat.is_finite() => format!(
            "useCoords: true, lng: {lng}, lat: {lat},"
        ),
        _ => "useCoords: false,".to_string(),
    };
    let js = format!(
        r#"(() => {{
  if (!window.MetisMap) return false;
  return window.MetisMap.placeRegionDraft({id}, {{
    shape: {shape},
    radiusMiles: {radius},
    {coords}
    mode: {mode}
  }});
}})()"#,
        id = escape_js_string(MAP_ID),
        shape = escape_js_string(shape),
        radius = radius_miles,
        coords = coords,
        mode = escape_js_string(mode),
    );
    let _ = document::eval(&js).await;
}

pub async fn map_clear_region_draft() {
    let js = format!(
        r#"(() => {{
  if (!window.MetisMap) return false;
  return window.MetisMap.clearRegionDraft({id});
}})()"#,
        id = escape_js_string(MAP_ID),
    );
    let _ = document::eval(&js).await;
}

/// Parse `west|south|east|north|lng|lat|radius|mode|shape` from the bridge wire.
fn parse_region_confirm_wire(raw: &str) -> Option<RegionDraftConfirm> {
    let parts: Vec<&str> = raw.trim().split('|').collect();
    if parts.len() < 7 {
        return None;
    }
    let w: f64 = parts[0].parse().ok()?;
    let s: f64 = parts[1].parse().ok()?;
    let e: f64 = parts[2].parse().ok()?;
    let n: f64 = parts[3].parse().ok()?;
    if !(w < e && s < n) {
        return None;
    }
    let lng: f64 = parts[4].parse().unwrap_or((w + e) / 2.0);
    let lat: f64 = parts[5].parse().unwrap_or((s + n) / 2.0);
    let radius_miles: f64 = parts[6].parse().unwrap_or(50.0);
    let mode = parts
        .get(7)
        .map(|m| m.to_string())
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| "new".into());
    let shape = parts
        .get(8)
        .map(|m| m.to_string())
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| "rectangle".into());
    Some(RegionDraftConfirm {
        shape,
        mode,
        radius_miles: radius_miles.max(5.0).min(300.0),
        lng,
        lat,
        bbox: [w, s, e, n],
    })
}

/// Build a draft payload in Rust (used when JS snapshot is unavailable).
pub fn region_draft_from_placement(
    shape: &str,
    radius_miles: f64,
    lng: f64,
    lat: f64,
    mode: &str,
) -> RegionDraftConfirm {
    let radius = radius_miles.max(5.0).min(300.0);
    let bbox = crate::map_tiles::bbox_from_center_radius(lat, lng, radius);
    RegionDraftConfirm {
        shape: shape.to_string(),
        mode: mode.to_string(),
        radius_miles: radius,
        lng,
        lat,
        bbox,
    }
}

/// Snapshot the painted draft. Uses a short pipe-delimited string — the same
/// eval path as getCamera. Object / long-JSON returns were returning null on desktop.
pub async fn map_snapshot_region_draft() -> Option<RegionDraftConfirm> {
    let raw = eval_string(&format!(
        r#"(() => {{
  try {{
    if (!window.MetisMap) return "";
    if (window.MetisMap.snapshotRegionDraftWire) {{
      return window.MetisMap.snapshotRegionDraftWire({id}) || "";
    }}
    // Older bridge fallback.
    if (window.MetisMap.snapshotRegionDraft) window.MetisMap.snapshotRegionDraft({id});
    var w = window.__metisDraftW, s = window.__metisDraftS,
        e = window.__metisDraftE, n = window.__metisDraftN;
    if (![w,s,e,n].every(function (x) {{ return typeof x === "number" && isFinite(x); }})) {{
      return "";
    }}
    return [w,s,e,n,window.__metisDraftLng,window.__metisDraftLat,window.__metisDraftR||50,window.__metisDraftMode||"new",window.__metisDraftShape||"rectangle"].join("|");
  }} catch (err) {{
    return "";
  }}
}})()"#,
        id = escape_js_string(MAP_ID),
    ))
    .await?;
    parse_region_confirm_wire(&raw)
}


#[derive(Clone, Copy, Debug)]
pub struct MapCamera {
    pub lng: f64,
    pub lat: f64,
    pub zoom: f64,
}

pub async fn map_get_camera() -> Option<MapCamera> {
    let raw = eval_string(
        r#"(() => (window.MetisMap ? window.MetisMap.getCamera("metis-main") : ""))()"#,
    )
    .await?;
    parse_camera_json(&raw)
}

fn parse_camera_json(raw: &str) -> Option<MapCamera> {
    let value: Value = serde_json::from_str(raw).ok()?;
    Some(MapCamera {
        lng: value.get("lng")?.as_f64()?,
        lat: value.get("lat")?.as_f64()?,
        zoom: value.get("zoom")?.as_f64()?,
    })
}

pub fn load_saved_camera() -> Option<MapCamera> {
    let raw = super::state().db.setting("map_view").ok().flatten()?;
    parse_camera_json(&raw)
}

pub fn save_camera(cam: MapCamera) {
    let raw = format!(
        r#"{{"lng":{:.6},"lat":{:.6},"zoom":{:.3}}}"#,
        cam.lng, cam.lat, cam.zoom
    );
    let _ = super::state().db.set_setting("map_view", &raw);
}

pub fn load_saved_basemap() -> BasemapKind {
    super::state()
        .db
        .setting("map_basemap")
        .ok()
        .flatten()
        .map(|s| BasemapKind::from_str(&s))
        .unwrap_or(BasemapKind::Outline)
}

pub fn save_basemap(kind: BasemapKind) {
    let _ = super::state()
        .db
        .set_setting("map_basemap", kind.as_str());
}

/// Outline GeoJSON is served over the `metis` custom protocol (not inlined) so
/// style swaps stay small enough for `document::eval` and do not blank the WebView.
/// On Windows wry exposes that as `http://metis.localhost` — WebView2 will not
/// fetch `metis://` for MapLibre sources.
fn outline_geojson_url(name: &str) -> String {
    crate::brand::metis_asset_url(&format!("geojson/{name}"))
}

fn outline_colors(kind: BasemapKind) -> (&'static str, &'static str, &'static str) {
    // background, land fill, border line
    match kind {
        BasemapKind::Outline => ("#0b1220", "#1a2740", "#5b6b86"),
        BasemapKind::Streets => ("#0c1424", "#1e2d48", "#7dd3fc"),
        BasemapKind::Terrain => ("#0d1510", "#1f3a28", "#86efac"),
        BasemapKind::Topo => ("#12100c", "#2a2418", "#fbbf24"),
    }
}

fn insert_outline_sources(sources: &mut serde_json::Map<String, Value>) {
    sources.insert(
        "world-land".into(),
        json!({
            "type": "geojson",
            "data": outline_geojson_url("world-land"),
        }),
    );
    sources.insert(
        "admin1-lines".into(),
        json!({
            "type": "geojson",
            "data": outline_geojson_url("admin1-lines"),
        }),
    );
    sources.insert(
        "admin0-borders".into(),
        json!({
            "type": "geojson",
            "data": outline_geojson_url("admin0-borders"),
        }),
    );
}

/// World land + borders (Natural Earth). Used alone for Outline mode, and under
/// PMTiles so zooming past a local pack still shows continents instead of a void.
fn outline_underlay_layers(kind: BasemapKind) -> Vec<Value> {
    let (_bg, land, border) = outline_colors(kind);
    vec![
        json!({
            "id": "world-land-fill",
            "type": "fill",
            "source": "world-land",
            "paint": {
                "fill-color": land,
                "fill-opacity": 1.0
            }
        }),
        json!({
            "id": "world-land-outline",
            "type": "line",
            "source": "world-land",
            "paint": {
                "line-color": border,
                "line-width": 0.7,
                "line-opacity": 0.55
            }
        }),
        json!({
            "id": "admin0-borders",
            "type": "line",
            "source": "admin0-borders",
            "paint": {
                "line-color": border,
                "line-width": match kind {
                    BasemapKind::Topo => 1.4,
                    BasemapKind::Streets => 1.2,
                    _ => 1.15,
                },
                "line-opacity": 0.95
            }
        }),
        json!({
            "id": "admin1-borders",
            "type": "line",
            "source": "admin1-lines",
            "paint": {
                "line-color": border,
                "line-width": match kind {
                    BasemapKind::Topo => 0.85,
                    BasemapKind::Streets => 0.7,
                    _ => 0.6,
                },
                "line-opacity": 0.7
            }
        }),
    ]
}

fn coverage_feature(coverage: &crate::map_tiles::ActiveCoverage) -> Value {
    let [w, s, e, n] = coverage.bbox;
    json!({
        "type": "Feature",
        "properties": { "shape": "rectangle" },
        "geometry": {
            "type": "Polygon",
            "coordinates": [[[w, s], [e, s], [e, n], [w, n], [w, s]]]
        }
    })
}

fn coverage_layers(kind: BasemapKind) -> Vec<Value> {
    let accent = match kind {
        BasemapKind::Terrain => "#86efac",
        BasemapKind::Topo => "#fbbf24",
        _ => "#34d399",
    };
    vec![
        json!({
            "id": "tile-coverage-fill",
            "type": "fill",
            "source": "tile-coverage",
            "paint": {
                "fill-color": accent,
                "fill-opacity": 0.07
            }
        }),
        json!({
            "id": "tile-coverage-line",
            "type": "line",
            "source": "tile-coverage",
            "paint": {
                "line-color": accent,
                "line-width": 2.0,
                "line-dasharray": [2, 2],
                "line-opacity": 0.85
            }
        }),
    ]
}

fn name_field() -> Value {
    json!(["coalesce", ["get", "name:en"], ["get", "name"], ["get", "ref"]])
}

struct VectorTheme {
    bg: &'static str,
    earth: &'static str,
    water: &'static str,
    park: &'static str,
    forest: &'static str,
    urban: &'static str,
    building: &'static str,
    highway: &'static str,
    highway_casing: &'static str,
    major: &'static str,
    minor: &'static str,
    path: &'static str,
    boundary: &'static str,
    /// Street-name text on a thick light halo (line labels; not icon-text-fit).
    street_label: &'static str,
    street_halo: &'static str,
    city_label: &'static str,
    city_halo: &'static str,
    neighbourhood_label: &'static str,
}

fn vector_theme(kind: BasemapKind) -> VectorTheme {
    match kind {
        BasemapKind::Terrain => VectorTheme {
            bg: "#0c1410",
            earth: "#1a2e22",
            water: "#14304a",
            park: "#1f4a32",
            forest: "#163828",
            urban: "#2a3430",
            building: "#2f3d38",
            highway: "#fb923c",
            highway_casing: "#7c2d12",
            major: "#86efac",
            minor: "#4d7c5c",
            path: "#3f6b4e",
            boundary: "#6ee7b7",
            street_label: "#0b1220",
            street_halo: "#ffffff",
            city_label: "#a7f3d0",
            city_halo: "#0c1410",
            neighbourhood_label: "#86efac",
        },
        BasemapKind::Topo => VectorTheme {
            bg: "#16130e",
            earth: "#2a2418",
            water: "#1a2a38",
            park: "#2f3a24",
            forest: "#24331c",
            urban: "#3a3228",
            building: "#3d342a",
            highway: "#f59e0b",
            highway_casing: "#78350f",
            major: "#e7c56a",
            minor: "#8a7350",
            path: "#6b5d4a",
            boundary: "#fbbf24",
            street_label: "#0b1220",
            street_halo: "#ffffff",
            city_label: "#fde68a",
            city_halo: "#16130e",
            neighbourhood_label: "#d6b981",
        },
        // Streets: orange freeways, muted arterials (so cities/labels read).
        _ => VectorTheme {
            bg: "#0b1220",
            earth: "#1a2740",
            water: "#0d1b2a",
            park: "#1a3a32",
            forest: "#163028",
            urban: "#243044",
            building: "#2a3548",
            highway: "#c2410c",
            highway_casing: "#7c2d12",
            // Darker than labels/cities so the grid doesn’t wash out text.
            major: "#3d4f66",
            minor: "#2a384c",
            path: "#243044",
            boundary: "#4b5c73",
            street_label: "#0b1220",
            street_halo: "#ffffff",
            city_label: "#38bdf8",
            city_halo: "#0b1220",
            neighbourhood_label: "#7dd3fc",
        },
    }
}

fn vector_basemap_layers(kind: BasemapKind) -> Vec<Value> {
    let t = vector_theme(kind);
    let name = name_field();
    let landcover_color = json!([
        "match", ["get", "kind"],
        "forest", t.forest,
        "urban_area", t.urban,
        "farmland", t.park,
        "grassland", t.park,
        "scrub", t.earth,
        "barren", t.urban,
        "glacier", t.water,
        t.earth
    ]);
    let landuse_color = json!([
        "match", ["get", "kind"],
        "forest", t.forest,
        "wood", t.forest,
        "park", t.park,
        "national_park", t.park,
        "nature_reserve", t.park,
        "grass", t.park,
        "meadow", t.park,
        "garden", t.park,
        "cemetery", t.park,
        "pitch", t.park,
        "residential", t.urban,
        "industrial", t.urban,
        "commercial", t.urban,
        "hospital", t.urban,
        "school", t.urban,
        "university", t.urban,
        "aerodrome", t.urban,
        "runway", t.minor,
        t.earth
    ]);

    // Streets mode: when zoomed in, keep freeways + major arterials only.
    let streets_focus = matches!(kind, BasemapKind::Streets | BasemapKind::Outline);

    vec![
        json!({"id":"background","type":"background","paint":{"background-color": t.bg}}),
        json!({"id":"earth","type":"fill","source":"protomaps","source-layer":"earth","paint":{"fill-color": t.earth}}),
        json!({"id":"landcover","type":"fill","source":"protomaps","source-layer":"landcover","paint":{"fill-color": landcover_color,"fill-opacity": if matches!(kind, BasemapKind::Terrain | BasemapKind::Topo) { 0.85 } else { 0.55 }}}),
        json!({"id":"landuse","type":"fill","source":"protomaps","source-layer":"landuse","paint":{"fill-color": landuse_color,"fill-opacity": 0.55}}),
        json!({"id":"water","type":"fill","source":"protomaps","source-layer":"water","paint":{"fill-color": t.water}}),
        json!({"id":"waterway","type":"line","source":"protomaps","source-layer":"water","filter":["==",["geometry-type"],"LineString"],"paint":{"line-color": t.water,"line-width":["interpolate",["linear"],["zoom"],8,0.5,14,2.0],"line-opacity":0.9}}),
        json!({"id":"buildings","type":"fill","source":"protomaps","source-layer":"buildings","minzoom":14,"paint":{"fill-color": t.building,"fill-opacity":0.55,"fill-outline-color": t.minor}}),
        // Local streets: mid zooms only in Streets mode (hidden when zoomed in).
        json!({
            "id":"roads-path",
            "type":"line",
            "source":"protomaps",
            "source-layer":"roads",
            "minzoom": 13,
            "maxzoom": if streets_focus { 14.5 } else { 24.0 },
            "filter":["==",["get","kind"],"path"],
            "paint":{"line-color": t.path,"line-width":["interpolate",["linear"],["zoom"],13,0.3,16,1.0],"line-opacity":0.55}
        }),
        json!({
            "id":"roads-minor",
            "type":"line",
            "source":"protomaps",
            "source-layer":"roads",
            "minzoom": 11,
            "maxzoom": if streets_focus { 14.0 } else { 24.0 },
            "filter":["==",["get","kind"],"minor_road"],
            "paint":{"line-color": t.minor,"line-width":["interpolate",["linear"],["zoom"],11,0.35,13,0.9,14,1.4],"line-opacity":0.75}
        }),
        json!({"id":"roads-major","type":"line","source":"protomaps","source-layer":"roads","filter":["==",["get","kind"],"major_road"],"paint":{"line-color": t.major,"line-width":["interpolate",["linear"],["zoom"],8,0.9,12,2.0,16,4.5],"line-opacity":0.95}}),
        json!({"id":"roads-highway-casing","type":"line","source":"protomaps","source-layer":"roads","filter":["==",["get","kind"],"highway"],"paint":{"line-color": t.highway_casing,"line-width":["interpolate",["linear"],["zoom"],6,2.2,10,4.5,14,8.0],"line-opacity":0.95}}),
        json!({"id":"roads-highway","type":"line","source":"protomaps","source-layer":"roads","filter":["==",["get","kind"],"highway"],"paint":{"line-color": t.highway,"line-width":["interpolate",["linear"],["zoom"],6,1.2,10,2.8,14,5.5],"line-opacity":1.0}}),
        json!({"id":"roads-rail","type":"line","source":"protomaps","source-layer":"roads","filter":["==",["get","kind"],"rail"],"paint":{"line-color": t.path,"line-width":0.8,"line-dasharray":[2,2],"line-opacity":0.45}}),
        json!({"id":"boundaries","type":"line","source":"protomaps","source-layer":"boundaries","paint":{"line-color": t.boundary,"line-width":["match",["get","kind"],"country",1.2,"region",0.9,0.5],"line-opacity":0.7}}),
        // Major street names — black type on a thick white halo.
        // (Stretchable icon-text-fit pills break on line placement: empty white
        // boxes where text couldn't sit on the road. Point-placed device pills
        // stay in map_bridge.js.)
        json!({
            "id":"road_labels_major",
            "type":"symbol",
            "source":"protomaps",
            "source-layer":"roads",
            "minzoom": 12,
            "filter": [
                "all",
                ["==", ["get", "kind"], "major_road"],
                ["!=", ["coalesce", ["get", "name:en"], ["get", "name"], ["get", "ref"], ""], ""]
            ],
            "layout": {
                "symbol-placement": "line",
                "symbol-spacing": 280,
                "text-field": name,
                "text-font": ["Noto Sans Medium"],
                "text-size": ["interpolate", ["linear"], ["zoom"], 12, 12, 16, 15],
                "text-rotation-alignment": "map",
                "text-max-angle": 30,
                "text-padding": 4,
                "text-optional": true
            },
            "paint": {
                "text-color": t.street_label,
                "text-halo-color": t.street_halo,
                "text-halo-width": 4.5,
                "text-halo-blur": 0.2
            }
        }),
        // Freeway / highway shields — Protomaps sprites are named
        // `{network}-{N}char` (e.g. US:I-2char), not bare `network`.
        json!({
            "id":"highway_shields",
            "type":"symbol",
            "source":"protomaps",
            "source-layer":"roads",
            "minzoom": 7,
            "filter": ["all",
                ["==", ["get", "kind"], "highway"],
                ["any", ["has", "shield_text"], ["has", "ref"]]
            ],
            "layout": {
                "symbol-placement": "line",
                "symbol-spacing": 260,
                "icon-image": [
                    "concat",
                    // Custom sprites: blue interstate vs white highway (see map_bridge.js).
                    ["match", ["coalesce", ["get", "network"], ""],
                        "US:I", "metis-shield-i-",
                        "metis-shield-hwy-"
                    ],
                    ["to-string", ["case",
                        [">=", ["length", ["to-string", ["coalesce", ["get", "shield_text"], ["get", "ref"], "1"]]], 5], 5,
                        [">=", ["length", ["to-string", ["coalesce", ["get", "shield_text"], ["get", "ref"], "1"]]], 4], 4,
                        [">=", ["length", ["to-string", ["coalesce", ["get", "shield_text"], ["get", "ref"], "1"]]], 3], 3,
                        [">=", ["length", ["to-string", ["coalesce", ["get", "shield_text"], ["get", "ref"], "1"]]], 2], 2,
                        1
                    ]],
                    "char"
                ],
                "icon-size": 1.05,
                "icon-rotation-alignment": "viewport",
                "icon-optional": true,
                "icon-allow-overlap": false,
                "text-field": ["coalesce", ["get", "shield_text"], ["get", "ref"]],
                "text-font": ["Noto Sans Medium"],
                "text-size": 11,
                "text-optional": true,
                "text-rotation-alignment": "viewport",
                "text-offset": [0.0, 0.15]
            },
            "paint": {
                // White digits on blue interstates; dark on white highway shields.
                "text-color": ["match", ["coalesce", ["get", "network"], ""],
                    "US:I", "#ffffff",
                    "#0f172a"
                ],
                "text-halo-width": 0
            }
        }),
        json!({
            "id":"place_neighbourhood",
            "type":"symbol",
            "source":"protomaps",
            "source-layer":"places",
            "minzoom": 12,
            "filter": ["in", ["get", "kind"], ["literal", ["neighbourhood", "macrohood"]]],
            "layout": {
                "text-field": name,
                "text-font": ["Noto Sans Regular"],
                "text-size": 11,
                "text-transform": "uppercase",
                "text-letter-spacing": 0.08,
                "text-optional": true
            },
            "paint": {
                "text-color": t.neighbourhood_label,
                "text-halo-color": t.city_halo,
                "text-halo-width": 1.8,
                "text-opacity": 0.9
            }
        }),
        json!({
            "id":"place_city",
            "type":"symbol",
            "source":"protomaps",
            "source-layer":"places",
            "filter": ["==", ["get", "kind"], "locality"],
            "layout": {
                "text-field": name,
                "text-font": ["Noto Sans Medium"],
                "text-size": ["interpolate", ["linear"], ["zoom"],
                    4, 11,
                    8, 14,
                    12, 18
                ],
                "text-optional": true,
                "text-padding": 6
            },
            "paint": {
                "text-color": t.city_label,
                "text-halo-color": t.city_halo,
                "text-halo-width": 2.2
            }
        }),
        json!({
            "id":"place_region",
            "type":"symbol",
            "source":"protomaps",
            "source-layer":"places",
            "maxzoom": 8,
            "filter": ["in", ["get", "kind"], ["literal", ["region", "country"]]],
            "layout": {
                "text-field": name,
                "text-font": ["Noto Sans Medium"],
                "text-size": ["match", ["get", "kind"], "country", 14, 12],
                "text-transform": "uppercase",
                "text-letter-spacing": 0.1,
                "text-optional": true
            },
            "paint": {
                "text-color": t.city_label,
                "text-halo-color": t.city_halo,
                "text-halo-width": 1.8,
                "text-opacity": 0.9
            }
        }),
        json!({
            "id":"water_labels",
            "type":"symbol",
            "source":"protomaps",
            "source-layer":"water",
            "filter": ["==", ["geometry-type"], "Point"],
            "layout": {
                "text-field": name,
                "text-font": ["Noto Sans Italic"],
                "text-size": 12,
                "text-optional": true
            },
            "paint": {
                "text-color": "#7dd3fc",
                "text-halo-color": t.city_halo,
                "text-halo-width": 1.6
            }
        }),
    ]
}

/// Build the MapLibre style.
///
/// `coverage` is the active offline pack. Streets tiles fill the pack square;
/// a dashed ring marks that area. Outline underlay remains so zooming out past
/// the pack is not a void.
pub fn dark_basemap_style(
    pmtiles_url: Option<&str>,
    kind: BasemapKind,
    coverage: Option<&crate::map_tiles::ActiveCoverage>,
) -> Value {
    let mut sources = serde_json::Map::new();
    let (bg, _land, _border) = outline_colors(kind);
    let mut layers = vec![json!({
        "id": "background",
        "type": "background",
        "paint": { "background-color": bg }
    })];

    // Always load outline GeoJSON so vector styles can fall back outside the pack.
    insert_outline_sources(&mut sources);
    layers.extend(outline_underlay_layers(kind));

    let use_vector = pmtiles_url.is_some() && kind != BasemapKind::Outline;
    // Glyphs for node/cluster labels even on Outline (pins are GL symbol layers).
    let glyphs = json!(
        "https://protomaps.github.io/basemaps-assets/fonts/{fontstack}/{range}.pbf"
    );
    let mut sprite: Option<Value> = None;
    if let (Some(url), true) = (pmtiles_url, use_vector) {
        let mut protomaps = json!({
            "type": "vector",
            "url": format!("pmtiles://{url}"),
            "attribution": "© OpenStreetMap © Protomaps"
        });
        // Limit tile requests to the placed pack. Skip world-scale overview
        // zooms even if an older extract still contains them — those tiles
        // spill far past the dashed square when zoomed out.
        if let Some(cov) = coverage {
            let [w, s, e, n] = cov.bbox;
            if w < e && s < n {
                let minzoom = crate::map_tiles::min_zoom_for_bbox(cov.bbox);
                if let Some(obj) = protomaps.as_object_mut() {
                    obj.insert("bounds".into(), json!([w, s, e, n]));
                    obj.insert("minzoom".into(), json!(minzoom));
                    obj.insert("maxzoom".into(), json!(12));
                }
            }
        }
        sources.insert("protomaps".into(), protomaps);
        sprite = Some(json!(match kind {
            BasemapKind::Terrain => {
                "https://protomaps.github.io/basemaps-assets/sprites/v4/dark"
            }
            BasemapKind::Topo => {
                "https://protomaps.github.io/basemaps-assets/sprites/v4/light"
            }
            _ => "https://protomaps.github.io/basemaps-assets/sprites/v4/dark",
        }));

        // Skip vector "background" — outline underlay already painted the world.
        for layer in vector_basemap_layers(kind).into_iter().skip(1) {
            layers.push(layer);
        }

        // Dashed pack ring — matches the operator-placed square.
        if let Some(cov) = coverage {
            let [w, s, e, n] = cov.bbox;
            if w < e && s < n {
                sources.insert(
                    "tile-coverage".into(),
                    json!({
                        "type": "geojson",
                        "data": {
                            "type": "FeatureCollection",
                            "features": [coverage_feature(cov)]
                        }
                    }),
                );
                layers.extend(coverage_layers(kind));
            }
        }
    }

    let mut style = json!({
        "version": 8,
        "sources": sources,
        "layers": layers,
        "glyphs": glyphs,
    });
    if let Some(s) = sprite {
        style
            .as_object_mut()
            .expect("style object")
            .insert("sprite".into(), s);
    }
    style
}
