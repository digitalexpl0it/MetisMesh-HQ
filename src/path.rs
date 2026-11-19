//! Hop hashes, message identity, and map lines.
//!
//! A hop hash is a prefix of a node public key. Two matches stay ambiguous.

use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HopMatch {
    Unknown { hash: String },
    One { pubkey: String },
    Ambiguous { keys: Vec<String> },
}

#[derive(Debug, Clone)]
pub struct KnownNode {
    pub pubkey: Vec<u8>,
    #[allow(dead_code)]
    pub name: String,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MapLine {
    pub coords: Vec<(f64, f64)>,
    pub dashed: bool,
    pub outbound: bool,
    pub hops: u32,
}

pub fn dedupe_key(kind: &str, sender: &str, timestamp: u32, text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(kind.as_bytes());
    hasher.update([0]);
    hasher.update(sender.as_bytes());
    hasher.update([0]);
    hasher.update(timestamp.to_le_bytes());
    hasher.update([0]);
    hasher.update(text.as_bytes());
    hex::encode(hasher.finalize())
}

/// Companion RX `path_len` is the **encoded** Packet path byte (not a raw hop
/// count): low 6 bits = hash count, high 2 bits = hash size code.
/// `0xFF` means the frame arrived direct (companion protocol receive semantics).
pub fn hop_count_from_path_len(path_len: u8) -> Option<u8> {
    if path_len == 0xFF {
        return Some(0);
    }
    let (hops, hash_size) = decode_encoded_path_len(path_len);
    if hash_size >= 4 {
        return None; // reserved hash size
    }
    Some(hops as u8)
}

/// Normalize a DB `hop_count` for display. Older builds stored the raw encoded
/// path byte (e.g. 133 → looked like “133 hops”); decode those on read.
pub fn display_hop_count(stored: Option<i64>) -> Option<i64> {
    let Some(n) = stored else {
        return None;
    };
    if !(0..=255).contains(&n) {
        return None;
    }
    let b = n as u8;
    if b == 0xFF {
        return Some(0);
    }
    // Decoded hop counts are 0..=63. Values with size bits set were mis-stored.
    if b > 63 {
        return hop_count_from_path_len(b).map(i64::from);
    }
    Some(n)
}

/// Advert-path `path_len` packs hop count in bits 0–5 and hash size in bits 6–7.
pub fn decode_encoded_path_len(path_len: u8) -> (usize, usize) {
    let hops = (path_len & 0x3f) as usize;
    let hash_size = ((path_len >> 6) as usize) + 1;
    (hops, hash_size)
}

pub fn split_hops(path: &[u8], hash_size: usize) -> Vec<Vec<u8>> {
    if hash_size == 0 {
        return Vec::new();
    }
    path.chunks(hash_size)
        .filter(|chunk| chunk.len() == hash_size)
        .map(|chunk| chunk.to_vec())
        .collect()
}

pub fn match_hash(hash: &[u8], nodes: &[KnownNode]) -> HopMatch {
    if hash.is_empty() {
        return HopMatch::Unknown {
            hash: String::new(),
        };
    }
    let mut hits: Vec<String> = nodes
        .iter()
        .filter(|node| node.pubkey.len() >= hash.len() && node.pubkey.starts_with(hash))
        .map(|node| hex::encode(&node.pubkey))
        .collect();
    hits.sort();
    hits.dedup();
    match hits.len() {
        0 => HopMatch::Unknown {
            hash: hex::encode(hash),
        },
        1 => HopMatch::One {
            pubkey: hits.remove(0),
        },
        _ => HopMatch::Ambiguous { keys: hits },
    }
}

pub fn resolve_hops(path: &[u8], hash_size: usize, nodes: &[KnownNode]) -> Vec<HopMatch> {
    split_hops(path, hash_size)
        .iter()
        .map(|hash| match_hash(hash, nodes))
        .collect()
}

/// Build one map line. Coordinates are `(longitude, latitude)`.
///
/// A resolved repeater makes the line solid. Endpoint-only traffic stays dashed.
pub fn build_line(
    outbound: bool,
    from: Option<(f64, f64)>,
    to: Option<(f64, f64)>,
    path: &[u8],
    hash_size: usize,
    nodes: &[KnownNode],
    hop_count: u32,
) -> Option<MapLine> {
    let resolved = resolve_hops(path, hash_size, nodes);
    let mut coords = Vec::new();
    if let Some(start) = from {
        coords.push(start);
    }
    let mut placed_hop = false;
    for hop in &resolved {
        if let HopMatch::One { pubkey } = hop {
            if let Some(node) = nodes.iter().find(|node| hex::encode(&node.pubkey) == *pubkey) {
                if let (Some(lat), Some(lon)) = (node.lat, node.lon) {
                    coords.push((lon, lat));
                    placed_hop = true;
                }
            }
        }
    }
    if let Some(end) = to {
        if coords.last() != Some(&end) {
            coords.push(end);
        }
    }
    if coords.len() < 2 {
        return None;
    }
    Some(MapLine {
        coords,
        dashed: !placed_hop,
        outbound,
        hops: hop_count,
    })
}

pub fn parse_advert_path(frame: &[u8]) -> Option<(Vec<u8>, usize)> {
    let start = frame.iter().position(|byte| *byte == 0x16)?;
    let rest = &frame[start..];
    if rest.len() < 6 {
        return None;
    }
    let (hops, hash_size) = decode_encoded_path_len(rest[5]);
    let need = hops.saturating_mul(hash_size);
    let path = rest.get(6..6 + need)?.to_vec();
    if path.len() != need {
        return None;
    }
    Some((path, hash_size))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(byte: u8, lat: Option<f64>) -> KnownNode {
        let mut pubkey = [0u8; 32];
        pubkey[0] = byte;
        KnownNode {
            pubkey: pubkey.to_vec(),
            name: format!("n{byte}"),
            lat,
            lon: lat.map(|value| value + 1.0),
        }
    }

    #[test]
    fn dedupe_changes_when_text_changes() {
        let left = dedupe_key("direct", "aa", 10, "signup");
        let right = dedupe_key("direct", "aa", 10, "status");
        assert_ne!(left, right);
        assert_eq!(left, dedupe_key("direct", "aa", 10, "signup"));
    }

    #[test]
    fn one_byte_hash_can_be_ambiguous() {
        let mut first = node(0x0A, None);
        first.pubkey[1] = 1;
        let mut second = node(0x0A, None);
        second.pubkey[1] = 2;
        let nodes = vec![first, second];
        assert!(matches!(
            match_hash(&[0x0A], &nodes),
            HopMatch::Ambiguous { .. }
        ));
        assert!(matches!(
            match_hash(&[0x0A, 0x01], &nodes),
            HopMatch::One { .. }
        ));
    }

    #[test]
    fn dashed_when_only_endpoints_are_known() {
        let line = build_line(
            false,
            Some((-122.0, 37.0)),
            Some((-121.0, 38.0)),
            &[],
            1,
            &[],
            3,
        )
        .unwrap();
        assert!(line.dashed);
        assert_eq!(line.coords.len(), 2);
        assert_eq!(line.hops, 3);
    }

    #[test]
    fn solid_when_a_repeater_is_placed() {
        let repeater = node(0x44, Some(37.5));
        let line = build_line(
            true,
            Some((-122.0, 37.0)),
            Some((-121.0, 38.0)),
            &[0x44],
            1,
            &[repeater],
            2,
        )
        .unwrap();
        assert!(!line.dashed);
        assert_eq!(line.coords.len(), 3);
    }

    #[test]
    fn advert_path_response_frame() {
        // RESP_CODE_ADVERT_PATH (22): [0x16][ts LE][encoded path_len][path…]
        let mut frame = vec![0x16];
        frame.extend_from_slice(&1_700_000_000u32.to_le_bytes());
        frame.push(0x03); // 3 hops, 1-byte hashes
        frame.extend_from_slice(&[0xAA, 0xBB, 0xCC]);
        let (path, hash_size) = parse_advert_path(&frame).expect("parse");
        assert_eq!(hash_size, 1);
        assert_eq!(path, vec![0xAA, 0xBB, 0xCC]);
    }

    #[test]
    fn encoded_path_len_and_frame() {
        assert_eq!(decode_encoded_path_len(0x45), (5, 2));
        let mut frame = vec![0x16, 0, 0, 0, 0, 0x01, 0xAB];
        let (path, hash_size) = parse_advert_path(&frame).unwrap();
        assert_eq!(hash_size, 1);
        assert_eq!(path, vec![0xAB]);
        frame[5] = 0x41;
        assert!(parse_advert_path(&frame).is_none());
    }

    #[test]
    fn companion_path_len_is_encoded_not_raw_hops() {
        // 133 = 0x85 → hash_count 5, hash_size 3 (not “133 hops”).
        assert_eq!(hop_count_from_path_len(133), Some(5));
        assert_eq!(hop_count_from_path_len(0x45), Some(5));
        assert_eq!(hop_count_from_path_len(2), Some(2));
        assert_eq!(hop_count_from_path_len(0xFF), Some(0));
        assert_eq!(display_hop_count(Some(133)), Some(5));
        assert_eq!(display_hop_count(Some(5)), Some(5));
    }

}
