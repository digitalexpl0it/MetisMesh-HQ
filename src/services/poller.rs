//! Background poll for NWS + USGS. Posts via RadioHandle channel send.

use std::sync::Arc;
use std::time::Duration;

use crate::db::Db;
use crate::radio::RadioHandle;
use crate::services::{
    alerts_channel_slot, fetch_active_alerts, fetch_quakes, format_nws_alert, format_quake,
    quake_bbox, quake_min_magnitude, setting_bool, setting_f64, shorten_url,
};
use crate::signup::fit_mesh;

/// Own thread + Tokio runtime (main has no reactor yet when HQ boots).
pub fn spawn_poller(db: Arc<Db>, radio: RadioHandle) {
    std::thread::Builder::new()
        .name("metis-services".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(err) => {
                    eprintln!("services runtime: {err}");
                    return;
                }
            };
            runtime.block_on(poller_loop(db, radio));
        })
        .ok();
}

async fn poller_loop(db: Arc<Db>, radio: RadioHandle) {
    // Stagger first run so BLE connect / contact sync win the first minute.
    tokio::time::sleep(Duration::from_secs(45)).await;
    let mut tick = tokio::time::interval(Duration::from_secs(60));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        // Dedicated services thread — blocking ureq is fine here.
        poll_once(&db, &radio);
    }
}

fn poll_once(db: &Db, radio: &RadioHandle) {
    if setting_bool(db, "svc_quake_enabled", false) {
        if let Err(err) = poll_quakes(db, radio) {
            hq_log!(debug, "quake poll: {err}");
        }
    }
    if setting_bool(db, "svc_nws_enabled", false) {
        if let Err(err) = poll_nws(db, radio) {
            hq_log!(debug, "nws poll: {err}");
        }
    }
}

fn poll_quakes(db: &Db, radio: &RadioHandle) -> Result<(), String> {
    let (min_lat, max_lat, min_lon, max_lon) = quake_bbox(db);
    let min_mag = quake_min_magnitude(db);
    let events = fetch_quakes(min_lat, max_lat, min_lon, max_lon, min_mag, 15)?;
    let slot = alerts_channel_slot(db);
    for event in events {
        if db.service_event_seen(&event.id).unwrap_or(true) {
            continue;
        }
        let body = format_quake(&event);
        let detail = serde_json::json!({
            "mag": event.magnitude,
            "place": event.place,
            "lat": event.lat,
            "lon": event.lon,
            "url": event.url,
        })
        .to_string();
        // Quakes auto-relay when the service is enabled (operator opted in).
        let _ = db.insert_service_event("quake", &event.id, &body, &detail, "sent");
        let _ = db.audit("svc_quake", "", &body);
        let _ = radio.send_channel(slot, body);
        // Avoid blasting many historical events in one tick.
        break;
    }
    Ok(())
}

fn poll_nws(db: &Db, radio: &RadioHandle) -> Result<(), String> {
    let lat = setting_f64(db, "hq_site_lat", f64::NAN);
    let lon = setting_f64(db, "hq_site_lon", f64::NAN);
    let (lat, lon) = if lat.is_finite() && lon.is_finite() {
        (lat, lon)
    } else {
        // Fall back to companion advert coords stored in settings if present.
        let lat = setting_f64(db, "hq_lat", f64::NAN);
        let lon = setting_f64(db, "hq_lon", f64::NAN);
        if lat.is_finite() && lon.is_finite() {
            (lat, lon)
        } else {
            return Err("NWS needs HQ site lat/lon in Settings → Services".into());
        }
    };
    let alerts = fetch_active_alerts(lat, lon)?;
    let auto = setting_bool(db, "svc_nws_auto_relay", false);
    let slot = alerts_channel_slot(db);
    for alert in alerts {
        if db.service_event_seen(&alert.id).unwrap_or(true) {
            continue;
        }
        let short = shorten_url(&alert.url);
        let body = format_nws_alert(&alert, short.as_deref());
        let detail = serde_json::json!({
            "event": alert.event,
            "severity": alert.severity,
            "area": alert.area,
            "url": alert.url,
            "short_url": short,
        })
        .to_string();
        if auto {
            let _ = db.insert_service_event("nws", &alert.id, &body, &detail, "sent");
            let _ = db.audit("svc_nws", "", &body);
            let _ = radio.send_channel(slot, body);
        } else {
            let _ = db.insert_service_event("nws", &alert.id, &body, &detail, "pending");
            let _ = db.audit("svc_nws_pending", "", &fit_mesh(&alert.event));
        }
        // One new alert per poll tick keeps the mesh calm.
        break;
    }
    Ok(())
}
