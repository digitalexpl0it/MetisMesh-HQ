//! Correlate companion `LogData` GRP_TXT self-echoes to recent channel TX
//! (MeshCore-style “heard repeaters”).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use meshcore_rs::events::LogData;
use meshcore_rs::packets::{PayloadType, RouteType};

use crate::channel_crypto::{self, try_decrypt_group_text};
use crate::path::split_hops;

const ECHO_WINDOW: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct PendingChannelEcho {
    pub message_id: i64,
    /// Mesh channel slot at send time (logging / future filters).
    #[allow(dead_code)]
    pub slot: u8,
    pub wire: String,
    pub secret: [u8; 16],
    pub channel_hash: u8,
    pub sent_at: Instant,
}

#[derive(Debug, Clone)]
pub struct HeardHop {
    pub hash: Vec<u8>,
    pub hash_size: u8,
}

#[derive(Debug, Clone)]
pub struct EchoMatch {
    pub message_id: i64,
    pub hops: Vec<HeardHop>,
    pub snr: f32,
    pub rssi: i16,
}

/// Drop expired pending sends.
pub fn prune_pending(pending: &mut VecDeque<PendingChannelEcho>) {
    let now = Instant::now();
    while pending
        .front()
        .is_some_and(|p| now.duration_since(p.sent_at) > ECHO_WINDOW)
    {
        pending.pop_front();
    }
}

/// Register a successful channel TX for echo listening.
pub fn push_pending(
    pending: &mut VecDeque<PendingChannelEcho>,
    message_id: i64,
    slot: u8,
    wire: String,
    secret: [u8; 16],
) {
    prune_pending(pending);
    pending.push_back(PendingChannelEcho {
        message_id,
        slot,
        wire,
        channel_hash: channel_crypto::channel_hash(&secret),
        secret,
        sent_at: Instant::now(),
    });
}

/// Try to attribute a LogData frame to a pending channel send.
/// Oldest matching pending wins (exact wire text + channel hash + MAC).
pub fn correlate_log_data(
    pending: &mut VecDeque<PendingChannelEcho>,
    log: &LogData,
) -> Option<EchoMatch> {
    prune_pending(pending);
    let header = log.header.as_ref()?;
    if header.payload_type != PayloadType::GroupText {
        return None;
    }
    match header.route_type {
        RouteType::Flood | RouteType::TransportFlood => {}
        _ => return None,
    }
    if header.path_len == 0 || header.path.is_empty() {
        return None;
    }
    let payload = &log.payload;
    if payload.len() < 19 {
        return None;
    }
    let hash_byte = payload[0];
    let hash_size = header.path_hash_size.max(1) as usize;
    let hops_raw = split_hops(&header.path, hash_size);
    if hops_raw.is_empty() {
        return None;
    }

    // Oldest-first exact match.
    let mut matched_idx = None;
    for (idx, pend) in pending.iter().enumerate() {
        if pend.channel_hash != hash_byte {
            continue;
        }
        let Some(body) = try_decrypt_group_text(&pend.secret, payload) else {
            continue;
        };
        if body == pend.wire {
            matched_idx = Some(idx);
            break;
        }
    }
    let idx = matched_idx?;
    let message_id = pending[idx].message_id;
    let hops = hops_raw
        .into_iter()
        .map(|hash| HeardHop {
            hash,
            hash_size: hash_size as u8,
        })
        .collect();
    Some(EchoMatch {
        message_id,
        hops,
        snr: log.snr,
        rssi: log.rssi,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel_crypto::encrypt_group_text;
    use meshcore_rs::events::MeshPacketHeader;

    fn secret() -> [u8; 16] {
        *b"0123456789abcdef"
    }

    fn flood_log(path: Vec<u8>, hash_size: u8, payload: Vec<u8>) -> LogData {
        let path_len = (path.len() / hash_size.max(1) as usize) as u8;
        LogData {
            snr: 7.5,
            rssi: -80,
            header: Some(MeshPacketHeader {
                route_type: RouteType::Flood,
                payload_type: PayloadType::GroupText,
                payload_version: 0,
                transport_code: None,
                path_len,
                path_hash_size: hash_size,
                path,
            }),
            advertisement: None,
            payload,
        }
    }

    #[test]
    fn oldest_pending_wins() {
        let secret = secret();
        let mut pending = VecDeque::new();
        push_pending(&mut pending, 1, 0, "HQ: first".into(), secret);
        push_pending(&mut pending, 2, 0, "HQ: second".into(), secret);
        let payload = encrypt_group_text(&secret, 100, "HQ: first").unwrap();
        let log = flood_log(vec![0xaa], 1, payload);
        let m = correlate_log_data(&mut pending, &log).expect("match");
        assert_eq!(m.message_id, 1);
        assert_eq!(m.hops.len(), 1);
        assert_eq!(m.hops[0].hash, vec![0xaa]);
    }

    #[test]
    fn wrong_text_no_match() {
        let secret = secret();
        let mut pending = VecDeque::new();
        push_pending(&mut pending, 1, 0, "HQ: mine".into(), secret);
        let payload = encrypt_group_text(&secret, 100, "Other: theirs").unwrap();
        let log = flood_log(vec![0x11], 1, payload);
        assert!(correlate_log_data(&mut pending, &log).is_none());
    }

    #[test]
    fn empty_path_skipped() {
        let secret = secret();
        let mut pending = VecDeque::new();
        push_pending(&mut pending, 1, 0, "HQ: x".into(), secret);
        let payload = encrypt_group_text(&secret, 100, "HQ: x").unwrap();
        let mut log = flood_log(vec![], 1, payload);
        if let Some(h) = log.header.as_mut() {
            h.path_len = 0;
            h.path.clear();
        }
        assert!(correlate_log_data(&mut pending, &log).is_none());
    }

    #[test]
    fn multi_byte_hash_split() {
        let secret = secret();
        let mut pending = VecDeque::new();
        push_pending(&mut pending, 9, 1, "HQ: path".into(), secret);
        let payload = encrypt_group_text(&secret, 1, "HQ: path").unwrap();
        let log = flood_log(vec![0x01, 0x02, 0x03, 0x04], 2, payload);
        let m = correlate_log_data(&mut pending, &log).unwrap();
        assert_eq!(m.hops.len(), 2);
        assert_eq!(m.hops[0].hash, vec![0x01, 0x02]);
        assert_eq!(m.hops[1].hash, vec![0x03, 0x04]);
    }
}
