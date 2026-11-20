//! Session-only Live Stream packet timeline (RAM ring from companion LogData).
//! Cleared when the process exits — nothing is written to SQLite.

use std::collections::VecDeque;

use meshcore_rs::events::LogData;
use meshcore_rs::packets::{PayloadType, RouteType};

use crate::path::{match_hash, split_hops, HopMatch, KnownNode};

pub const LIVE_STREAM_CAP: usize = 512;
pub const LIVE_STREAM_MAX_AGE_SECS: i64 = 30 * 60;

#[derive(Debug, Clone, PartialEq)]
pub struct LiveStreamHop {
    pub lon: f64,
    pub lat: f64,
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LiveStreamEvent {
    pub seq: u64,
    pub at: i64,
    pub snr: f32,
    pub kind: String,
    pub route: String,
    pub hops: Vec<LiveStreamHop>,
}

fn route_label(route: RouteType) -> &'static str {
    match route {
        RouteType::Flood => "flood",
        RouteType::Direct => "direct",
        RouteType::TransportFlood => "tflood",
        RouteType::TransportDirect => "tdirect",
    }
}

fn kind_label(payload: PayloadType) -> &'static str {
    match payload {
        PayloadType::Req => "req",
        PayloadType::Response => "resp",
        PayloadType::TextMsg => "dm",
        PayloadType::Ack => "ack",
        PayloadType::Advert => "advert",
        PayloadType::GroupText => "channel",
        PayloadType::GroupData => "ch-data",
        PayloadType::AnonReq => "anon",
        PayloadType::Path => "path",
        PayloadType::Trace => "trace",
        PayloadType::Multipart => "multi",
        PayloadType::Control => "ctrl",
        PayloadType::RawCustom => "raw",
        PayloadType::Unknown => "?",
    }
}

fn gps_ok(lat: f64, lon: f64) -> bool {
    lat != 0.0
        && lon != 0.0
        && (-90.0..=90.0).contains(&lat)
        && (-180.0..=180.0).contains(&lon)
}

fn push_unique_hop(hops: &mut Vec<LiveStreamHop>, lon: f64, lat: f64, label: Option<String>) {
    if !gps_ok(lat, lon) {
        return;
    }
    if hops
        .last()
        .is_some_and(|h| (h.lon - lon).abs() < 1e-7 && (h.lat - lat).abs() < 1e-7)
    {
        return;
    }
    hops.push(LiveStreamHop { lon, lat, label });
}

/// Build a Live Stream event from overheard LogData when ≥2 GPS points resolve.
///
/// `prepend_hq`: HQ companion coords (lon, lat) for own channel echoes.
pub fn event_from_log(
    log: &LogData,
    nodes: &[KnownNode],
    prepend_hq: Option<(f64, f64)>,
    seq: u64,
    at: i64,
) -> Option<LiveStreamEvent> {
    let header = log.header.as_ref()?;
    let hash_size = header.path_hash_size.max(1) as usize;
    let mut hops = Vec::with_capacity(header.path_len as usize + 2);

    if let Some((lon, lat)) = prepend_hq {
        push_unique_hop(&mut hops, lon, lat, Some("HQ".into()));
    }

    if header.payload_type == PayloadType::Advert {
        if let Some(adv) = log.advertisement.as_ref() {
            if let (Some(lat_u), Some(lon_u)) = (adv.lat, adv.lon) {
                let lat = lat_u as f64 / 1_000_000.0;
                let lon = lon_u as f64 / 1_000_000.0;
                let label = adv
                    .name
                    .as_ref()
                    .map(|n| n.trim())
                    .filter(|n| !n.is_empty())
                    .map(str::to_string);
                push_unique_hop(&mut hops, lon, lat, label);
            }
        }
    }

    if !header.path.is_empty() && header.path_len > 0 {
        for hash in split_hops(&header.path, hash_size) {
            match match_hash(&hash, nodes) {
                HopMatch::One { pubkey } => {
                    if let Some(node) = nodes.iter().find(|n| hex::encode(&n.pubkey) == pubkey) {
                        if let (Some(lat), Some(lon)) = (node.lat, node.lon) {
                            let label = {
                                let n = node.name.trim();
                                if n.is_empty() {
                                    None
                                } else {
                                    Some(n.to_string())
                                }
                            };
                            push_unique_hop(&mut hops, lon, lat, label);
                        }
                    }
                }
                HopMatch::Unknown { .. } | HopMatch::Ambiguous { .. } => {}
            }
        }
    }

    if hops.len() < 2 {
        return None;
    }

    Some(LiveStreamEvent {
        seq,
        at,
        snr: log.snr,
        kind: kind_label(header.payload_type).to_string(),
        route: route_label(header.route_type).to_string(),
        hops,
    })
}

/// Drop oldest by age, then by cap.
pub fn prune_ring(ring: &mut VecDeque<LiveStreamEvent>, now: i64) {
    let cutoff = now.saturating_sub(LIVE_STREAM_MAX_AGE_SECS);
    while ring.front().is_some_and(|e| e.at < cutoff) {
        ring.pop_front();
    }
    while ring.len() > LIVE_STREAM_CAP {
        ring.pop_front();
    }
}

pub fn push_event(ring: &mut VecDeque<LiveStreamEvent>, event: LiveStreamEvent) {
    let now = event.at;
    ring.push_back(event);
    prune_ring(ring, now);
}

#[cfg(test)]
mod tests {
    use super::*;
    use meshcore_rs::events::MeshPacketHeader;

    fn node(hex_key: &str, name: &str, lat: f64, lon: f64) -> KnownNode {
        KnownNode {
            pubkey: hex::decode(hex_key).unwrap(),
            name: name.into(),
            lat: Some(lat),
            lon: Some(lon),
        }
    }

    fn flood_log(path: Vec<u8>, hash_size: u8, ptype: PayloadType) -> LogData {
        let path_len = (path.len() / hash_size.max(1) as usize) as u8;
        LogData {
            snr: 5.0,
            rssi: -70,
            header: Some(MeshPacketHeader {
                route_type: RouteType::Flood,
                payload_type: ptype,
                payload_version: 0,
                transport_code: None,
                path_len,
                path_hash_size: hash_size,
                path,
            }),
            advertisement: None,
            payload: vec![],
        }
    }

    #[test]
    fn drops_when_fewer_than_two_gps_points() {
        let nodes = vec![node("aa", "A", 34.0, -117.0)];
        let log = flood_log(vec![0xaa], 1, PayloadType::GroupText);
        assert!(event_from_log(&log, &nodes, None, 1, 100).is_none());
    }

    #[test]
    fn keeps_two_resolved_hops() {
        let nodes = vec![
            node("aa", "Alpha", 34.0, -117.0),
            node("bb", "Beta", 34.1, -117.1),
        ];
        let log = flood_log(vec![0xaa, 0xbb], 1, PayloadType::GroupText);
        let ev = event_from_log(&log, &nodes, None, 2, 200).expect("event");
        assert_eq!(ev.hops.len(), 2);
        assert_eq!(ev.hops[0].label.as_deref(), Some("Alpha"));
        assert_eq!(ev.kind, "channel");
        assert_eq!(ev.route, "flood");
    }

    #[test]
    fn skips_ambiguous_hash() {
        // Two nodes share prefix 0xcc — ambiguous, must not place.
        let nodes = vec![
            node("cc01", "C1", 34.0, -117.0),
            node("cc02", "C2", 34.2, -117.2),
            node("dd", "D", 34.3, -117.3),
        ];
        let log = flood_log(vec![0xcc, 0xdd], 1, PayloadType::GroupText);
        // Only D resolves → with no HQ prepend, only 1 point → drop.
        assert!(event_from_log(&log, &nodes, None, 3, 300).is_none());
        // With HQ + D → 2 points kept (ambiguous skipped).
        let ev = event_from_log(&log, &nodes, Some((-117.5, 34.5)), 4, 400).unwrap();
        assert_eq!(ev.hops.len(), 2);
        assert_eq!(ev.hops[0].label.as_deref(), Some("HQ"));
        assert_eq!(ev.hops[1].label.as_deref(), Some("D"));
    }

    #[test]
    fn prune_by_cap_and_age() {
        let mut ring = VecDeque::new();
        for i in 0..520 {
            push_event(
                &mut ring,
                LiveStreamEvent {
                    seq: i,
                    at: 1_000_000 + i as i64,
                    snr: 0.0,
                    kind: "channel".into(),
                    route: "flood".into(),
                    hops: vec![
                        LiveStreamHop {
                            lon: -117.0,
                            lat: 34.0,
                            label: None,
                        },
                        LiveStreamHop {
                            lon: -117.1,
                            lat: 34.1,
                            label: None,
                        },
                    ],
                },
            );
        }
        assert!(ring.len() <= LIVE_STREAM_CAP);
        // Age prune: everything older than 30m relative to now.
        let now = 1_000_000 + 519 + LIVE_STREAM_MAX_AGE_SECS + 10;
        prune_ring(&mut ring, now);
        assert!(ring.is_empty());
    }
}
