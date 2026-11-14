//! Session-only channel speakers (MeshCore-style).
//!
//! Cleared every process start. Populated only when a live channel message
//! arrives — not from history reload or connect-queue drain.
//! Rail lists stay collapsed by default; expand state is also session-only.

use std::collections::{HashMap, HashSet};
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

fn expanded() -> &'static Mutex<HashSet<String>> {
    static EXPANDED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    EXPANDED.get_or_init(|| Mutex::new(HashSet::new()))
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

fn note_into(map: &mut HashMap<String, Vec<ChannelSpeaker>>, channel: &str, name: &str, pubkey: Option<&str>) {
    let channel = channel.trim();
    let name = name.trim();
    if channel.is_empty() || name.is_empty() {
        return;
    }
    let pubkey = pubkey
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase());
    let list = map.entry(channel_key(channel)).or_default();
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

fn list_from(map: &HashMap<String, Vec<ChannelSpeaker>>, channel: &str) -> Vec<ChannelSpeaker> {
    let channel = channel.trim();
    if channel.is_empty() {
        return Vec::new();
    }
    map.get(&channel_key(channel))
        .cloned()
        .unwrap_or_default()
}

fn is_expanded_in(set: &HashSet<String>, channel: &str) -> bool {
    let channel = channel.trim();
    if channel.is_empty() {
        return false;
    }
    set.contains(&channel_key(channel))
}

fn toggle_expanded_in(set: &mut HashSet<String>, channel: &str) -> bool {
    let channel = channel.trim();
    if channel.is_empty() {
        return false;
    }
    let key = channel_key(channel);
    if set.contains(&key) {
        set.remove(&key);
        false
    } else {
        set.insert(key);
        true
    }
}

/// Record a live channel sender under `channel` (most recent first).
pub fn note(channel: &str, name: &str, pubkey: Option<&str>) {
    let mut guard = speakers().lock().unwrap_or_else(|err| err.into_inner());
    note_into(&mut guard, channel, name, pubkey);
}

/// Speakers for a channel this session (most recent first).
pub fn list(channel: &str) -> Vec<ChannelSpeaker> {
    let guard = speakers().lock().unwrap_or_else(|err| err.into_inner());
    list_from(&guard, channel)
}

/// Whether the rail shows speakers under this channel (default: collapsed).
pub fn is_expanded(channel: &str) -> bool {
    let guard = expanded().lock().unwrap_or_else(|err| err.into_inner());
    is_expanded_in(&guard, channel)
}

/// Toggle rail expand for session contacts under a channel. Returns the new open state.
pub fn toggle_expanded(channel: &str) -> bool {
    let mut guard = expanded().lock().unwrap_or_else(|err| err.into_inner());
    toggle_expanded_in(&mut guard, channel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_move_to_front_and_upgrade_key() {
        // Local map — never touch the process-global speakers (avoids Windows
        // parallel-test races that flaked CI on main while the tag job passed).
        let mut map = HashMap::new();
        note_into(&mut map, "Public", "Ada", None);
        note_into(&mut map, "Public", "Bob", Some("aabb"));
        note_into(&mut map, "Public", "Ada", Some("ccdd"));
        let rows = list_from(&map, "Public");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "Ada");
        assert_eq!(rows[0].pubkey.as_deref(), Some("ccdd"));
        assert_eq!(rows[1].name, "Bob");
    }

    #[test]
    fn rail_speakers_collapsed_by_default() {
        let mut set = HashSet::new();
        assert!(!is_expanded_in(&set, "Public"));
        assert!(toggle_expanded_in(&mut set, "Public"));
        assert!(is_expanded_in(&set, "Public"));
        assert!(!toggle_expanded_in(&mut set, "Public"));
        assert!(!is_expanded_in(&set, "Public"));
    }
}
