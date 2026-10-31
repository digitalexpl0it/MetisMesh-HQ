//! Desktop MapLibre host via `document::eval` (dioxus-maplibre is wasm-gated).

use dioxus::prelude::*;
use serde_json::Value;

pub const MAP_ID: &str = "metis-main";
pub const MAP_CONTAINER_ID: &str = "metis-map-container";

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

pub async fn map_take_selection() -> Option<String> {
    let js = r#"(() => (window.MetisMap ? window.MetisMap.takeSelection() : ""))()"#;
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

fn world_land_geojson() -> Value {
    serde_json::from_str(include_str!("../../assets/world-land.geojson"))
        .unwrap_or_else(|_| serde_json::json!({ "type": "FeatureCollection", "features": [] }))
}

pub fn dark_basemap_style(pmtiles_url: Option<&str>) -> Value {
    let mut sources = serde_json::Map::new();
    let mut layers = vec![serde_json::json!({
        "id": "background",
        "type": "background",
        "paint": { "background-color": "rgb(11, 18, 32)" }
    })];

    if let Some(url) = pmtiles_url {
        // Prefer raster PMTiles (common offline extract). Vector archives still load as
        // a source; operators can swap style later.
        sources.insert(
            "basemap".into(),
            serde_json::json!({
                "type": "raster",
                "url": format!("pmtiles://{url}"),
                "tileSize": 256,
            }),
        );
        layers.push(serde_json::json!({
            "id": "basemap-raster",
            "type": "raster",
            "source": "basemap",
            "paint": { "raster-opacity": 0.92 }
        }));
    } else {
        // Bundled Natural Earth 110m land — geography without CDN or PMTiles.
        sources.insert(
            "world-land".into(),
            serde_json::json!({
                "type": "geojson",
                "data": world_land_geojson(),
            }),
        );
        layers.push(serde_json::json!({
            "id": "world-land-fill",
            "type": "fill",
            "source": "world-land",
            "paint": {
                "fill-color": "#1a2740",
                "fill-opacity": 1.0
            }
        }));
        layers.push(serde_json::json!({
            "id": "world-land-outline",
            "type": "line",
            "source": "world-land",
            "paint": {
                "line-color": "#2f4060",
                "line-width": 0.6
            }
        }));
    }

    serde_json::json!({
        "version": 8,
        "sources": sources,
        "layers": layers,
    })
}
