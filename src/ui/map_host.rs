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

pub async fn map_init(style: &Value, lng: f64, lat: f64, zoom: f64) -> Result<(), String> {
    let style_json = style.to_string();
    let js = format!(
        r#"(async () => {{
  if (!window.MetisMap) throw new Error("MetisMap bridge missing");
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
    document::eval(&js)
        .await
        .map_err(|err| format!("{err:?}"))?;
    Ok(())
}

pub async fn map_destroy() {
    let js = format!(
        r#"(() => {{
  if (window.MetisMap) window.MetisMap.destroy({id});
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
    let cam_json = match camera {
        Some(cam) => format!(
            r#"{{"lng":{},"lat":{},"zoom":{}}}"#,
            cam.lng, cam.lat, cam.zoom
        ),
        None => "null".into(),
    };
    let js = format!(
        r#"(async () => {{
  if (!window.MetisMap) throw new Error("MetisMap bridge missing");
  await window.MetisMap.setStyle({id}, {style}, {cam});
  return true;
}})()"#,
        id = escape_js_string(MAP_ID),
        style = style.to_string(),
        cam = cam_json,
    );
    document::eval(&js)
        .await
        .map_err(|err| format!("{err:?}"))?;
    Ok(())
}

pub async fn map_exists() -> bool {
    let js = format!(
        r#"(() => !!(window.MetisMap && window.MetisMap.maps[{id}]))()"#,
        id = escape_js_string(MAP_ID),
    );
    match document::eval(&js).await {
        Ok(value) => value.as_bool().unwrap_or(false),
        Err(_) => false,
    }
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

async fn eval_string(js: &str) -> Option<String> {
    match document::eval(js).await {
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
    eval_string(r#"(() => (window.MetisMap ? window.MetisMap.takeSelection() : ""))()"#).await
}

pub async fn map_take_favorite_toggle() -> Option<String> {
    eval_string(r#"(() => (window.MetisMap ? window.MetisMap.takeFavoriteToggle() : ""))()"#)
        .await
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

/// Outline GeoJSON is served over `metis://` (not inlined) so style swaps stay small
/// enough for `document::eval` and do not blank the WebView.
fn outline_geojson_url(name: &str) -> String {
    format!("metis://local/geojson/{name}")
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
    /// Street-name “pill”: dark type on light halo.
    street_label: &'static str,
    street_halo: &'static str,
    city_label: &'static str,
    city_halo: &'static str,
    neighbourhood_label: &'static str,
    highway_shield_text: &'static str,
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
            street_label: "#052e16",
            street_halo: "#d1fae5",
            city_label: "#a7f3d0",
            city_halo: "#0c1410",
            neighbourhood_label: "#86efac",
            highway_shield_text: "#0b1220",
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
            street_label: "#1c1917",
            street_halo: "#fef3c7",
            city_label: "#fde68a",
            city_halo: "#16130e",
            neighbourhood_label: "#d6b981",
            highway_shield_text: "#1c1917",
        },
        // Streets: orange freeways, slate arterials, pill street labels, cyan cities.
        _ => VectorTheme {
            bg: "#0b1220",
            earth: "#1a2740",
            water: "#0d1b2a",
            park: "#1a3a32",
            forest: "#163028",
            urban: "#243044",
            building: "#2a3548",
            highway: "#f97316",
            highway_casing: "#7c2d12",
            major: "#cbd5e1",
            minor: "#475569",
            path: "#334155",
            boundary: "#64748b",
            street_label: "#0f172a",
            street_halo: "#f1f5f9",
            city_label: "#38bdf8",
            city_halo: "#0b1220",
            neighbourhood_label: "#7dd3fc",
            highway_shield_text: "#0f172a",
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
        // Major street names only — dark type + thick light halo ≈ pill contrast.
        json!({
            "id":"road_labels_major",
            "type":"symbol",
            "source":"protomaps",
            "source-layer":"roads",
            "minzoom": 12,
            "filter": ["==", ["get", "kind"], "major_road"],
            "layout": {
                "symbol-placement": "line",
                "symbol-spacing": 220,
                "text-field": name,
                "text-font": ["Noto Sans Medium"],
                "text-size": ["interpolate", ["linear"], ["zoom"], 12, 11, 16, 14],
                "text-max-angle": 25,
                "text-padding": 2,
                "text-optional": true
            },
            "paint": {
                "text-color": t.street_label,
                "text-halo-color": t.street_halo,
                "text-halo-width": 3.2,
                "text-halo-blur": 0.15
            }
        }),
        // Freeway / highway shields (sprite) + fallback ref text.
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
                "icon-image": ["coalesce", ["get", "network"], ""],
                "icon-size": 1.0,
                "icon-rotation-alignment": "viewport",
                "icon-optional": true,
                "icon-allow-overlap": false,
                "text-field": ["coalesce", ["get", "shield_text"], ["get", "ref"]],
                "text-font": ["Noto Sans Medium"],
                "text-size": 11,
                "text-optional": true,
                "text-rotation-alignment": "viewport",
                "text-offset": [0.0, 0.0]
            },
            "paint": {
                "text-color": t.highway_shield_text,
                "text-halo-color": t.street_halo,
                "text-halo-width": 2.4
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

pub fn dark_basemap_style(pmtiles_url: Option<&str>, kind: BasemapKind) -> Value {
    let mut sources = serde_json::Map::new();
    let (bg, land, border) = outline_colors(kind);
    let mut layers = vec![serde_json::json!({
        "id": "background",
        "type": "background",
        "paint": { "background-color": bg }
    })];

    let use_vector = pmtiles_url.is_some() && kind != BasemapKind::Outline;
    let mut glyphs: Option<Value> = None;
    let mut sprite: Option<Value> = None;
    if let (Some(url), true) = (pmtiles_url, use_vector) {
        sources.insert(
            "protomaps".into(),
            serde_json::json!({
                "type": "vector",
                "url": format!("pmtiles://{url}"),
                "attribution": "© OpenStreetMap © Protomaps"
            }),
        );
        glyphs = Some(json!(
            "https://protomaps.github.io/basemaps-assets/fonts/{fontstack}/{range}.pbf"
        ));
        sprite = Some(json!(match kind {
            BasemapKind::Terrain => {
                "https://protomaps.github.io/basemaps-assets/sprites/v4/dark"
            }
            BasemapKind::Topo => {
                "https://protomaps.github.io/basemaps-assets/sprites/v4/light"
            }
            _ => "https://protomaps.github.io/basemaps-assets/sprites/v4/dark",
        }));
        layers = vector_basemap_layers(kind);
    } else {
        sources.insert(
            "world-land".into(),
            serde_json::json!({
                "type": "geojson",
                "data": outline_geojson_url("world-land"),
            }),
        );
        sources.insert(
            "admin1-lines".into(),
            serde_json::json!({
                "type": "geojson",
                "data": outline_geojson_url("admin1-lines"),
            }),
        );
        sources.insert(
            "admin0-borders".into(),
            serde_json::json!({
                "type": "geojson",
                "data": outline_geojson_url("admin0-borders"),
            }),
        );
        layers.push(serde_json::json!({
            "id": "world-land-fill",
            "type": "fill",
            "source": "world-land",
            "paint": {
                "fill-color": land,
                "fill-opacity": 1.0
            }
        }));
        layers.push(serde_json::json!({
            "id": "world-land-outline",
            "type": "line",
            "source": "world-land",
            "paint": {
                "line-color": border,
                "line-width": 0.7,
                "line-opacity": 0.55
            }
        }));
        // Country borders (Natural Earth 110m admin 0).
        layers.push(serde_json::json!({
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
        }));
        // State / province borders (Natural Earth 110m admin 1).
        layers.push(serde_json::json!({
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
        }));
    }

    let mut style = serde_json::json!({
        "version": 8,
        "sources": sources,
        "layers": layers,
    });
    if let Some(g) = glyphs {
        style
            .as_object_mut()
            .expect("style object")
            .insert("glyphs".into(), g);
    }
    if let Some(s) = sprite {
        style
            .as_object_mut()
            .expect("style object")
            .insert("sprite".into(), s);
    }
    style
}
