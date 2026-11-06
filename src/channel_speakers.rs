//! Session-only channel speakers (MeshCore-style).
//!
//! Cleared every process start. Populated only when a live channel message
//! arrives — not from history reload or connect-queue drain.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Someone who has posted to a channel during this HQ session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelSpeaker {
    pub name: String,
    pub pubkey: Option<String>,
}

fn speakers() -> &'static Mutex<HashMap<String, Vec<ChannelSpeaker>>> {
    static SPEAKERS: OnceLock<Mutex<HashMap<String, Vec<ChannelSpeaker>>>> = OnceLock::new();
    SPEAKERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn channel_key(name: &str) -> String {
    name.trim().to_string()
}

fn same_speaker(a: &ChannelSpeaker, name: &str, pubkey: Option<&str>) -> bool {
    if let (Some(left), Some(right)) = (a.pubkey.as_deref(), pubkey) {
        return left.eq_ignore_ascii_case(right);
    }
    a.name.eq_ignore_ascii_case(name)
}

/// Record a live channel sender under `channel` (most recent first).
pub fn note(channel: &str, name: &str, pubkey: Option<&str>) {
    let channel = channel.trim();
    let name = name.trim();
    if channel.is_empty() || name.is_empty() {
        return;
    }
    let pubkey = pubkey
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase());
    let mut guard = speakers().lock().unwrap_or_else(|err| err.into_inner());
    let list = guard.entry(channel_key(channel)).or_default();
    if let Some(idx) = list
        .iter()
        .position(|s| same_speaker(s, name, pubkey.as_deref()))
    {
        let mut existing = list.remove(idx);
        if existing.name != name {
            existing.name = name.to_string();
        }
        if existing.pubkey.is_none() {
            if let Some(pk) = pubkey {
                existing.pubkey = Some(pk);
            }
        }
        list.insert(0, existing);
    } else {
        list.insert(
            0,
            ChannelSpeaker {
                name: name.to_string(),
                pubkey,
            },
        );
    }
}

/// Speakers for a channel this session (most recent first).
pub fn list(channel: &str) -> Vec<ChannelSpeaker> {
    let channel = channel.trim();
    if channel.is_empty() {
        return Vec::new();
    }
    speakers()
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .get(&channel_key(channel))
        .cloned()
        .unwrap_or_default()
}

/// Drop every session speaker (tests / explicit reset).
#[cfg(test)]
pub fn clear_all() {
    speakers()
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_move_to_front_and_upgrade_key() {
        clear_all();
        note("Public", "Ada", None);
        note("Public", "Bob", Some("aabb"));
        note("Public", "Ada", Some("ccdd"));
        let rows = list("Public");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "Ada");
        assert_eq!(rows[0].pubkey.as_deref(), Some("ccdd"));
        assert_eq!(rows[1].name, "Bob");
        clear_all();
    }
}
