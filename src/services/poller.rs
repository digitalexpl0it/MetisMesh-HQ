//! Background poll for NWS + USGS + scheduled ZIP weather. Posts via RadioHandle.

use std::sync::Arc;
use std::time::Duration;

use crate::backup;
use crate::db::{unix_now, Db};
use crate::radio::RadioHandle;
use crate::secrets::Secrets;
use crate::services::{
    alerts_channel_slot, cached_zip_coords, emerg_state, fetch_active_alerts,
    fetch_emergency_alerts, fetch_quakes, fetch_weather_line, format_nws_alert, format_quake,
    is_amber_event, quake_bbox, quake_min_magnitude, remember_zip_coords, setting_bool, setting_f64,
    short_wx_label, shorten_url, weather_interval_secs, weather_unit_f, weather_zip_list,
    AMBER_CODES, EMERG_CODES, WeatherQuery,
};
use crate::signup::fit_mesh;

/// Own thread + Tokio runtime (main has no reactor yet when HQ boots).
pub fn spawn_poller(db: Arc<Db>, secrets: Arc<Secrets>, radio: RadioHandle) {
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
            runtime.block_on(async move {
                let mqtt_db = db.clone();
                let push_db = db.clone();
                let pull_db = db.clone();
                let map_db = db.clone();
                let map_radio = radio.clone();
                tokio::spawn(crate::services::mqtt_weather::mqtt_weather_loop(mqtt_db));
                tokio::spawn(crate::services::ecowitt::ecowitt_push_loop(push_db));
                tokio::spawn(crate::services::ecowitt::ecowitt_pull_loop(pull_db));
                crate::services::map_upload::spawn_map_upload_worker(map_db, map_radio);
                poller_loop(db, secrets, radio).await;
            });
        })
        .ok();
}

async fn poller_loop(db: Arc<Db>, secrets: Arc<Secrets>, radio: RadioHandle) {
    // Stagger first run so BLE connect / contact sync win the first minute.
    tokio::time::sleep(Duration::from_secs(45)).await;
    let mut tick = tokio::time::interval(Duration::from_secs(60));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        // Dedicated services thread — blocking ureq is fine here.
        poll_once(&db, &secrets, &radio);
    }
}

fn poll_once(db: &Db, secrets: &Secrets, radio: &RadioHandle) {
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
    if setting_bool(db, "svc_weather_enabled", true)
        && setting_bool(db, "svc_weather_hourly", false)
    {
        if let Err(err) = poll_weather_zips(db, radio) {
            hq_log!(debug, "weather zip poll: {err}");
        }
    }
    if setting_bool(db, "svc_amber_enabled", false) {
        if let Err(err) = poll_emergency(db, radio, "amber", AMBER_CODES, true) {
            hq_log!(debug, "amber poll: {err}");
        }
    }
    if setting_bool(db, "svc_emerg_enabled", false) {
        if let Err(err) = poll_emergency(db, radio, "emerg", EMERG_CODES, false) {
            hq_log!(debug, "emerg poll: {err}");
        }
    }
    backup::maybe_run_scheduled_backup(db, secrets, &crate::data_dir());
}

/// USGS: post only when a *new* quake appears. Empty / already-seen → silent.
fn poll_quakes(db: &Db, radio: &RadioHandle) -> Result<(), String> {
    let (min_lat, max_lat, min_lon, max_lon) = quake_bbox(db);
    let min_mag = quake_min_magnitude(db);
    let events = fetch_quakes(min_lat, max_lat, min_lon, max_lon, min_mag, 15)?;
    let slot = alerts_channel_slot(db);
    for event in events {
        if db.service_event_seen(&event.id).unwrap_or(true) {
            continue;
        }
        let body = format_quake(db, &event);
        let detail = serde_json::json!({
            "mag": event.magnitude,
            "place": event.place,
            "lat": event.lat,
            "lon": event.lon,
            "url": event.url,
        })
        .to_string();
        let _ = db.insert_service_event("quake", &event.id, &body, &detail, "sent");
        let _ = db.audit("svc_quake", "", &body);
        let _ = radio.send_channel(slot, body);
        // One new event per tick keeps the mesh calm.
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
        break;
    }
    Ok(())
}

/// Hourly (configurable) weather for each ZIP in Settings. Posts only when due;
/// one ZIP per poll minute so a long list does not flood the mesh.
fn poll_weather_zips(db: &Db, radio: &RadioHandle) -> Result<(), String> {
    let zips = weather_zip_list(db);
    if zips.is_empty() {
        return Ok(());
    }
    let interval = weather_interval_secs(db).max(60);
    let bucket = unix_now() / interval;
    let slot = alerts_channel_slot(db);
    let fahrenheit = weather_unit_f(db);

    for zip in zips {
        let event_id = format!("wx-sched:{zip}:{bucket}");
        if db.service_event_seen(&event_id).unwrap_or(true) {
            continue;
        }

        let (lat, lon, label) = if let Some(cached) = cached_zip_coords(db, &zip) {
            cached
        } else {
            match crate::map_tiles::geocode_place(&zip) {
                Ok((lat, lon, label)) => {
                    remember_zip_coords(db, &zip, lat, lon, &label);
                    (lat, lon, label)
                }
                Err(err) => {
                    hq_log!(warn, "weather zip {zip}: {err}");
                    let _ = db.insert_service_event(
                        "wx",
                        &event_id,
                        &fit_mesh(&format!("WX {zip}: place lookup failed")),
                        &serde_json::json!({ "zip": zip, "error": err }).to_string(),
                        "sent",
                    );
                    continue;
                }
            }
        };

        let query = WeatherQuery {
            lat,
            lon,
            label: short_wx_label(&label, Some(&zip)),
            fahrenheit,
        };
        let body = match fetch_weather_line(&query) {
            Ok(line) => line,
            Err(err) => {
                hq_log!(warn, "weather zip {zip}: {err}");
                continue;
            }
        };
        let detail = serde_json::json!({
            "zip": zip,
            "lat": lat,
            "lon": lon,
            "bucket": bucket,
        })
        .to_string();
        let _ = db.insert_service_event("wx", &event_id, &body, &detail, "sent");
        let _ = db.audit("svc_wx_sched", &zip, &body);
        let _ = radio.send_channel(slot, body);
        // One ZIP per minute tick.
        break;
    }
    Ok(())
}

/// Amber / civil emergencies for the configured state. Posts only new IDs.
/// When `amber_only` is false, skip CAE if Amber polling is also on (avoid double post).
fn poll_emergency(
    db: &Db,
    radio: &RadioHandle,
    kind: &str,
    codes: &str,
    amber_only: bool,
) -> Result<(), String> {
    let state = emerg_state(db);
    let alerts = fetch_emergency_alerts(&state, codes)?;
    let auto_key = if amber_only {
        "svc_amber_auto_relay"
    } else {
        "svc_emerg_auto_relay"
    };
    let auto = setting_bool(db, auto_key, false);
    let amber_also = setting_bool(db, "svc_amber_enabled", false);
    let slot = alerts_channel_slot(db);
    for alert in alerts {
        if !amber_only && amber_also && is_amber_event(&alert) {
            continue;
        }
        if db.service_event_seen(&alert.id).unwrap_or(true) {
            continue;
        }
        let short = shorten_url(&alert.url);
        let body = format_nws_alert(&alert, short.as_deref());
        let detail = serde_json::json!({
            "event": alert.event,
            "code": alert.event_code,
            "area": alert.area,
            "state": state,
            "url": alert.url,
        })
        .to_string();
        if auto {
            let _ = db.insert_service_event(kind, &alert.id, &body, &detail, "sent");
            let _ = db.audit(&format!("svc_{kind}"), "", &body);
            let _ = radio.send_channel(slot, body);
        } else {
            let _ = db.insert_service_event(kind, &alert.id, &body, &detail, "pending");
            let _ = db.audit(&format!("svc_{kind}_pending"), "", &fit_mesh(&alert.event));
        }
        break;
    }
    Ok(())
}
