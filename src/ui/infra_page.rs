//! Operator-claimed mesh infrastructure (repeaters / rooms / sensors).
//! Traffic goes through the primary companion over LoRa — never treat these as pool radios.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use dioxus::prelude::*;

use crate::db::{unix_now, ClaimedInfraRow, InfraStatusLatest};
use crate::signup::Enrollment;

use super::cli_page::{
    commands_for, is_destructive_cli, CliMode, STATUS_CLI_FOLLOWUP,
};
use super::icons::{Icon, IconName};
use super::state;
use super::widgets::{
    Badge, Button, ButtonKind, Dialog, PageHeading, SelectInput, TextInput,
};
use crate::radio::RadioStatus;

const PROBE_COOLDOWN_SECS: i64 = 60;
const STALE_HEARD_SECS: i64 = 30 * 60;

fn pending_infra_slot() -> &'static Mutex<Option<String>> {
    static SLOT: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// Overview (and others) can deep-link into a claimed node detail.
pub fn open_infra_detail(pubkey: String) {
    if let Ok(mut guard) = pending_infra_slot().lock() {
        *guard = Some(pubkey);
    }
}

fn take_pending_infra() -> Option<String> {
    pending_infra_slot()
        .lock()
        .ok()
        .and_then(|mut g| g.take())
}

fn node_title(name: &str, requested: &Option<String>) -> String {
    if let Some(req) = requested {
        if !req.trim().is_empty() && req != name {
            return format!("{name} ({req})");
        }
    }
    if name.trim().is_empty() {
        "unnamed".into()
    } else {
        name.to_string()
    }
}

fn relative_ago(ts: Option<i64>) -> String {
    let Some(ts) = ts else {
        return "never".into();
    };
    let delta = (unix_now() - ts).max(0);
    if delta < 60 {
        "just now".into()
    } else if delta < 3600 {
        format!("{}m ago", delta / 60)
    } else if delta < 86_400 {
        format!("{}h ago", delta / 3600)
    } else {
        format!("{}d ago", delta / 86_400)
    }
}

fn fuzzy_score(needle: &str, haystacks: &[&str]) -> Option<u32> {
    let needle = needle.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return Some(0);
    }
    let mut best = None;
    for hay in haystacks {
        let h = hay.to_ascii_lowercase();
        if h.contains(&needle) {
            let score = 1000u32.saturating_sub((h.len() as u32).saturating_sub(needle.len() as u32));
            best = Some(best.map_or(score, |b: u32| b.max(score)));
        }
    }
    best
}

fn kind_label(kind: &str) -> &'static str {
    match kind {
        "room" => "Room",
        "sensor" => "Sensor",
        _ => "Repeater",
    }
}

fn kind_tone(kind: &str) -> &'static str {
    match kind {
        "room" => "pending",
        "sensor" => "signal",
        _ => "muted",
    }
}

fn append_transcript(mut transcript: Signal<Vec<String>>, text: &str) {
    transcript.with_mut(|lines| {
        for part in text.lines() {
            lines.push(mask_cli_line(part));
        }
        if lines.len() > 400 {
            let skip = lines.len() - 400;
            lines.drain(0..skip);
        }
    });
}

fn mask_cli_line(line: &str) -> String {
    let lower = line.to_ascii_lowercase();
    if lower.contains("password") && !lower.starts_with('>') {
        // Avoid echoing new passwords from `password` / guest.password replies.
        if let Some(idx) = line.find(':') {
            return format!("{}: [secret]", &line[..idx]);
        }
        return "[secret]".into();
    }
    line.to_string()
}

fn battery_pct_from_mv(mv: i64) -> u8 {
    const MIN: i64 = 3000;
    const MAX: i64 = 4200;
    if mv <= MIN {
        0
    } else if mv >= MAX {
        100
    } else {
        (((mv - MIN) * 100) / (MAX - MIN)) as u8
    }
}

fn format_uptime(secs: i64) -> String {
    let secs = secs.max(0);
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3600;
    let mins = (secs % 3600) / 60;
    let s = secs % 60;
    if days > 0 {
        format!("{days}d {hours}h {mins}m")
    } else if hours > 0 {
        format!("{hours}h {mins}m {s}s")
    } else {
        format!("{mins}m {s}s")
    }
}

fn format_airtime(ms: i64) -> String {
    let secs = (ms.max(0) / 1000) as i64;
    format_uptime(secs)
}

fn parse_tx_power(reply: &str) -> Option<i64> {
    let cleaned = reply
        .trim()
        .trim_start_matches(|c: char| !c.is_ascii_digit() && c != '-');
    let num: String = cleaned
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '-')
        .collect();
    num.parse().ok().filter(|v| (-10..=30).contains(v))
}

/// Lines that must not clear `pending_cli_capture` (send-ack / local waiting chrome).
fn is_cli_capture_noise(body: &str) -> bool {
    let t = body.trim();
    if t.is_empty() {
        return true;
    }
    let lower = t.to_ascii_lowercase();
    lower == "… waiting for reply"
        || lower == "... waiting for reply"
        || lower.starts_with("cli command sent to contact")
        || lower.starts_with("cli command sent —")
        || lower.starts_with("cli command sent -")
}

fn display_title(row: &ClaimedInfraRow, nodes: &[crate::db::NodeRow]) -> String {
    if let Some(node) = nodes.iter().find(|n| n.pubkey == row.pubkey) {
        let title = node_title(&node.name, &node.requested_name);
        if !title.is_empty() && title != "unnamed" {
            return title;
        }
    }
    if !row.label.trim().is_empty() {
        return row.label.clone();
    }
    row.pubkey.chars().take(12).collect()
}

fn persist_status_data(
    pubkey: &str,
    info: &meshcore_rs::events::StatusData,
    prior_tx: Option<i64>,
) {
    crate::radio::persist_infra_status_probe(&state().db, pubkey, info, prior_tx);
}

#[component]
pub fn InfraPage(radio: RadioStatus) -> Element {
    let mut tick = use_signal(|| 0u64);
    let mut infra_tab = use_signal(|| "claimed".to_string());
    let mut detail_tab = use_signal(|| "status".to_string());
    let mut settings_panel = use_signal(|| None::<String>);
    let mut advanced_cli_open = use_signal(|| false);
    let mut query = use_signal(String::new);
    let mut selected = use_signal(|| None::<String>);
    let mut notes_draft = use_signal(String::new);
    let mut editing_notes = use_signal(|| false);
    let mut kind_draft = use_signal(|| "repeater".to_string());
    let mut note = use_signal(String::new);
    let mut claim_kind = use_signal(|| String::new());
    let mut claim_target = use_signal(|| None::<(String, String, Option<String>, bool)>);
    let mut claim_password = use_signal(String::new);
    let mut claim_busy = use_signal(|| false);
    let mut claim_error = use_signal(String::new);

    let mut draft = use_signal(String::new);
    let mut preset = use_signal(String::new);
    let mut transcript = use_signal(|| {
        vec![
            "Remote CLI for verified infrastructure — traffic goes through the primary companion."
                .to_string(),
            "Claim requires an admin password over LoRa (not stored). ACL on the node authorizes later stats/CLI."
                .to_string(),
        ]
    });
    let mut confirm_open = use_signal(|| false);
    let mut pending = use_signal(String::new);
    let mut last_seq = use_signal(|| 0u64);
    let mut poll = use_signal(|| 0u32);
    let mut poll_started = use_signal(|| false);
    let last_probe = use_signal(HashMap::<String, i64>::new);
    let probe_busy = use_signal(|| false);
    let mut discover_note = use_signal(String::new);
    let mut need_relogin = use_signal(|| false);
    let mut pending_cli_capture = use_signal(|| None::<(String, String)>);
    let status_note = use_signal(String::new);

    // Settings form drafts.
    let pos_lat = use_signal(String::new);
    let pos_lon = use_signal(String::new);
    let acl_key = use_signal(String::new);
    let acl_perm = use_signal(|| "3".to_string());
    let mut admin_pw = use_signal(String::new);
    let mut guest_pw = use_signal(String::new);
    let region_name = use_signal(String::new);
    let region_parent = use_signal(|| "*".to_string());
    let flood_advert_hrs = use_signal(String::new);
    let advert_mins = use_signal(String::new);
    let tx_dbm = use_signal(String::new);
    let mut settings_dialog = use_signal(|| None::<String>); // admin_pw | guest_pw

    if !poll_started() {
        poll_started.set(true);
        spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                poll.with_mut(|n| *n = n.wrapping_add(1));
            }
        });
    }

    // Deep-link from Overview.
    if let Some(key) = take_pending_infra() {
        selected.set(Some(key));
        infra_tab.set("claimed".into());
        detail_tab.set("status".into());
    }

    let _ = tick();
    let linked = radio.state == "connected";
    let claimed = state().db.list_claimed_infra().unwrap_or_default();
    let claimed_keys: std::collections::HashSet<_> =
        claimed.iter().map(|c| c.pubkey.clone()).collect();
    let nodes = state().db.list_nodes().unwrap_or_default();
    let needle = query();

    let selected_row = selected().and_then(|key| {
        claimed
            .iter()
            .find(|r| r.pubkey == key)
            .cloned()
    });
    let selected_node = selected_row.as_ref().and_then(|row| {
        nodes.iter().find(|n| n.pubkey == row.pubkey).cloned()
    });
    let status_latest = selected_row
        .as_ref()
        .and_then(|r| state().db.get_infra_status_latest(&r.pubkey).ok().flatten());

    let mut candidates: Vec<_> = nodes
        .iter()
        .filter(|n| n.enrollment != Enrollment::Hq && !claimed_keys.contains(&n.pubkey))
        .filter(|n| matches!(n.contact_type, 2 | 3 | 4) || !claim_kind().is_empty())
        .filter_map(|node| {
            let title = node_title(&node.name, &node.requested_name);
            let score = fuzzy_score(
                &needle,
                &[
                    title.as_str(),
                    node.name.as_str(),
                    node.prefix.as_str(),
                    node.pubkey.as_str(),
                ],
            )?;
            Some((score, node, title))
        })
        .collect();
    candidates.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.2.to_lowercase().cmp(&b.2.to_lowercase()))
    });
    candidates.truncate(40);

    let mode = selected_row
        .as_ref()
        .map(|r| CliMode::from_infra_kind(&r.kind))
        .or_else(|| {
            selected_node
                .as_ref()
                .map(|n| CliMode::from_contact_type(n.contact_type))
        })
        .unwrap_or(CliMode::Repeater);
    let catalog = commands_for(mode);
    let placeholder = catalog
        .first()
        .map(|(cmd, _)| (*cmd).to_string())
        .unwrap_or_else(|| "ver".into());

    use_effect(move || {
        let _ = poll();
        let snap = state().radio.snapshot();
        if snap.cli_seq == 0 || snap.cli_seq == last_seq() || snap.cli_line.is_empty() {
            return;
        }
        last_seq.set(snap.cli_seq);
        let line = snap.cli_line.clone();
        append_transcript(transcript, &line);
        if let Some((pubkey, cmd)) = pending_cli_capture() {
            let body = line.trim();
            // Remote send-acks used to clear capture before the mesh CLI_DATA reply.
            if !is_cli_capture_noise(body) {
                match cmd.as_str() {
                    "get tx" => {
                        if let Some(dbm) = parse_tx_power(body) {
                            let _ = state().db.update_infra_status_cli_fields(
                                &pubkey,
                                Some(dbm),
                                None,
                                None,
                                None,
                                None,
                            );
                            tick.with_mut(|n| *n += 1);
                        }
                    }
                    "ver" => {
                        let _ = state().db.update_infra_status_cli_fields(
                            &pubkey,
                            None,
                            Some(body),
                            None,
                            None,
                            None,
                        );
                        tick.with_mut(|n| *n += 1);
                    }
                    "clock" => {
                        let _ = state().db.update_infra_status_cli_fields(
                            &pubkey,
                            None,
                            None,
                            None,
                            Some(body),
                            None,
                        );
                        tick.with_mut(|n| *n += 1);
                    }
                    "get radio" => {
                        let _ = state().db.update_infra_status_cli_fields(
                            &pubkey,
                            None,
                            None,
                            Some(body),
                            None,
                            None,
                        );
                        tick.with_mut(|n| *n += 1);
                    }
                    "neighbors" => {
                        let _ = state().db.update_infra_status_cli_fields(
                            &pubkey,
                            None,
                            None,
                            None,
                            None,
                            Some(body),
                        );
                        tick.with_mut(|n| *n += 1);
                    }
                    _ => {}
                }
                pending_cli_capture.set(None);
            }
        }
    });

    rsx! {
        div { class: "mx-auto flex h-full min-h-0 max-w-5xl flex-col gap-4",
            PageHeading {
                icon: IconName::Signal,
                title: "Infrastructure".to_string(),
                "Claim only after admin login over LoRa through the primary companion. Password is not stored — the node ACL authorizes later stats and remote CLI."
            }
            if !note().is_empty() {
                p { class: "text-xs text-amber-200", "{note}" }
            }
            if !linked {
                p { class: "rounded-md border border-border bg-card/40 px-3 py-2 text-sm text-muted-foreground",
                    "Connect a primary companion to probe or run remote CLI on claimed nodes."
                }
            }

            {
                match selected_row.clone() {
                    Some(row) => {
                        let pubkey = row.pubkey.clone();
                        let title = display_title(&row, &nodes);
                        let prefix = selected_node
                            .as_ref()
                            .map(|n| n.prefix.clone())
                            .unwrap_or_else(|| row.pubkey.chars().take(12).collect());
                        let last_heard = selected_node.as_ref().and_then(|n| n.last_heard);
                        let heard_label = relative_ago(last_heard);
                        let stale = last_heard
                            .map(|t| unix_now() - t > STALE_HEARD_SECS)
                            .unwrap_or(true);
                        let has_gps = selected_node.as_ref().map(|n| n.has_gps).unwrap_or(false);
                        let card_class = if stale {
                            "rounded-lg border border-amber-500/30 bg-card/40 p-4"
                        } else {
                            "rounded-lg border border-border bg-card/40 p-4"
                        };
                        let last_probe_at = last_probe().get(&pubkey).copied().unwrap_or(0);
                        let probe_wait = (PROBE_COOLDOWN_SECS - (unix_now() - last_probe_at)).max(0);
                        let verified = row.is_verified();
                        let can_probe = linked && verified && !probe_busy() && probe_wait == 0;
                        let verify_label = title.clone();
                        let verify_kind = Some(row.kind.clone());
                        let on_status = detail_tab() == "status";
                        let on_settings = detail_tab() == "settings";
                        let on_cli = detail_tab() == "cli";
                        let latest = status_latest.clone();
                        rsx! {
                            section { class: "{card_class}",
                                div { class: "mb-3 flex flex-wrap items-start justify-between gap-3",
                                    div { class: "min-w-0",
                                        div { class: "mb-1 flex flex-wrap items-center gap-2",
                                            h3 { class: "text-base font-semibold", "{title}" }
                                            Badge { tone: kind_tone(&row.kind).to_string(), "{kind_label(&row.kind)}" }
                                            if verified {
                                                Badge { tone: "enrolled".to_string(), "Verified" }
                                            } else {
                                                Badge { tone: "pending".to_string(), "Unverified" }
                                            }
                                            if stale {
                                                Badge { tone: "pending".to_string(), "Stale" }
                                            }
                                        }
                                        p { class: "font-mono text-xs text-muted-foreground", "{prefix}" }
                                        p { class: "text-xs text-muted-foreground",
                                            "Last heard {heard_label}"
                                            if has_gps { " · GPS fix" } else { " · no GPS" }
                                        }
                                        if !verified {
                                            p { class: "mt-1 text-xs text-amber-200",
                                                "Verify admin over LoRa before status refresh or settings."
                                            }
                                        } else if need_relogin() {
                                            p { class: "mt-1 text-xs text-amber-200",
                                                "CLI timed out — ACL may have expired. Verify admin again (claim stays)."
                                            }
                                        }
                                    }
                                    div { class: "flex flex-wrap gap-2",
                                        Button {
                                            kind: ButtonKind::Ghost,
                                            onclick: move |_| {
                                                selected.set(None);
                                                editing_notes.set(false);
                                                need_relogin.set(false);
                                                settings_panel.set(None);
                                                infra_tab.set("claimed".into());
                                            },
                                            "Back to list"
                                        }
                                        Button {
                                            kind: if verified { ButtonKind::Ghost } else { ButtonKind::Primary },
                                            disabled: !linked || claim_busy(),
                                            onclick: {
                                                let key = pubkey.clone();
                                                let label = verify_label.clone();
                                                let kind = verify_kind.clone();
                                                move |_| {
                                                    claim_error.set(String::new());
                                                    claim_password.set(String::new());
                                                    claim_target.set(Some((
                                                        key.clone(),
                                                        label.clone(),
                                                        kind.clone(),
                                                        true,
                                                    )));
                                                }
                                            },
                                            Icon { name: IconName::Key, size: 14 }
                                            if verified { "Re-verify" } else { "Verify admin…" }
                                        }
                                        Button {
                                            kind: ButtonKind::Ghost,
                                            onclick: {
                                                let key = pubkey.clone();
                                                move |_| {
                                                    let _ = state().db.unclaim_infra(&key);
                                                    selected.set(None);
                                                    note.set("Unclaimed.".into());
                                                    tick.with_mut(|n| *n += 1);
                                                }
                                            },
                                            Icon { name: IconName::Close, size: 14 }
                                            "Unclaim"
                                        }
                                    }
                                }

                                div { class: "mb-4 flex flex-wrap items-end gap-2",
                                    div { class: "min-w-[10rem]",
                                        label { class: "mb-1 block text-xs text-muted-foreground", "Kind" }
                                        SelectInput {
                                            id: "infra-kind".to_string(),
                                            value: kind_draft(),
                                            onchange: {
                                                let key = pubkey.clone();
                                                move |event: FormEvent| {
                                                    let value = event.value();
                                                    kind_draft.set(value.clone());
                                                    match state().db.update_claimed_kind(&key, &value) {
                                                        Ok(()) => tick.with_mut(|n| *n += 1),
                                                        Err(err) => note.set(err.to_string()),
                                                    }
                                                }
                                            },
                                            option { value: "repeater", style: "background-color:#101418;color:#f4f4f5;", "Repeater" }
                                            option { value: "room", style: "background-color:#101418;color:#f4f4f5;", "Room" }
                                            option { value: "sensor", style: "background-color:#101418;color:#f4f4f5;", "Sensor" }
                                        }
                                    }
                                    div { class: "min-w-0 flex-1",
                                        label { class: "mb-1 block text-xs text-muted-foreground", "Notes (no passwords)" }
                                        if editing_notes() {
                                            div { class: "flex flex-col gap-2 sm:flex-row",
                                                TextInput {
                                                    value: notes_draft(),
                                                    placeholder: "e.g. admin enabled · church roof".to_string(),
                                                    kind: "text".to_string(),
                                                    oninput: move |event: FormEvent| notes_draft.set(event.value()),
                                                }
                                                Button {
                                                    kind: ButtonKind::Primary,
                                                    onclick: {
                                                        let key = pubkey.clone();
                                                        move |_| {
                                                            match state().db.update_claimed_notes(&key, &notes_draft()) {
                                                                Ok(()) => {
                                                                    editing_notes.set(false);
                                                                    note.set(String::new());
                                                                    tick.with_mut(|n| *n += 1);
                                                                }
                                                                Err(err) => note.set(err.to_string()),
                                                            }
                                                        }
                                                    },
                                                    "Save"
                                                }
                                                Button {
                                                    kind: ButtonKind::Ghost,
                                                    onclick: move |_| editing_notes.set(false),
                                                    "Cancel"
                                                }
                                            }
                                        } else {
                                            div { class: "flex flex-wrap items-center gap-2",
                                                span { class: "text-sm text-muted-foreground",
                                                    {
                                                        if row.notes.is_empty() {
                                                            "—".to_string()
                                                        } else {
                                                            row.notes.clone()
                                                        }
                                                    }
                                                }
                                                Button {
                                                    kind: ButtonKind::Ghost,
                                                    onclick: {
                                                        let notes = row.notes.clone();
                                                        move |_| {
                                                            notes_draft.set(notes.clone());
                                                            editing_notes.set(true);
                                                        }
                                                    },
                                                    "Edit notes"
                                                }
                                            }
                                        }
                                    }
                                }

                                div { class: "settings-subtabs mb-3",
                                    button {
                                        class: if on_status { "settings-subtab settings-subtab-active" } else { "settings-subtab" },
                                        onclick: move |_| detail_tab.set("status".into()),
                                        "Status"
                                    }
                                    button {
                                        class: if on_settings { "settings-subtab settings-subtab-active" } else { "settings-subtab" },
                                        onclick: move |_| detail_tab.set("settings".into()),
                                        "Settings"
                                    }
                                    button {
                                        class: if on_cli { "settings-subtab settings-subtab-active" } else { "settings-subtab" },
                                        onclick: move |_| {
                                            detail_tab.set("cli".into());
                                            advanced_cli_open.set(true);
                                        },
                                        "Command line"
                                    }
                                }

                                if on_status {
                                    div { class: "mb-3 flex flex-wrap items-center gap-2",
                                        Button {
                                            disabled: !can_probe,
                                            onclick: {
                                                let key = pubkey.clone();
                                                move |_| {
                                                    start_status_refresh(
                                                        linked,
                                                        key.clone(),
                                                        last_probe,
                                                        probe_busy,
                                                        status_note,
                                                        transcript,
                                                        pending_cli_capture,
                                                        need_relogin,
                                                        tick,
                                                    );
                                                }
                                            },
                                            Icon { name: IconName::Scan, size: 14 }
                                            {
                                                if probe_busy() {
                                                    "Refreshing…".to_string()
                                                } else if probe_wait > 0 {
                                                    format!("Refresh ({probe_wait}s)")
                                                } else {
                                                    "Refresh status".to_string()
                                                }
                                            }
                                        }
                                        Button {
                                            kind: ButtonKind::Ghost,
                                            disabled: !linked,
                                            onclick: move |_| {
                                                discover_note.set(String::new());
                                                let rx = state().radio.discover_nodes(2);
                                                spawn(async move {
                                                    match rx.await {
                                                        Ok(Ok(summary)) => {
                                                            discover_note.set(format!(
                                                                "Discover: {} returned, {} added",
                                                                summary.returned, summary.added
                                                            ));
                                                            tick.with_mut(|n| *n += 1);
                                                        }
                                                        Ok(Err(err)) => discover_note.set(err),
                                                        Err(_) => {
                                                            discover_note.set(
                                                                "Discover cancelled or radio unavailable.".into(),
                                                            );
                                                        }
                                                    }
                                                });
                                            },
                                            Icon { name: IconName::Scan, size: 14 }
                                            "Discover repeaters"
                                        }
                                        if !status_note().is_empty() {
                                            span { class: "text-xs text-muted-foreground", "{status_note}" }
                                        }
                                        if !discover_note().is_empty() {
                                            span { class: "text-xs text-muted-foreground", "{discover_note}" }
                                        }
                                    }
                                    if !verified {
                                        p { class: "mb-3 rounded-md border border-border bg-card/40 px-3 py-2 text-sm text-muted-foreground",
                                            "Status refresh unlocks after admin verification."
                                        }
                                    }
                                    { status_panel(latest.as_ref()) }
                                }

                                if on_settings {
                                    if !verified {
                                        p { class: "mb-3 text-sm text-amber-200",
                                            "Verify admin first. Passwords are typed here only — never stored in HQ."
                                        }
                                    }
                                    { settings_list(
                                        verified && linked,
                                        linked,
                                        &pubkey,
                                        selected_node.as_ref(),
                                        latest.as_ref(),
                                        settings_panel,
                                        pos_lat,
                                        pos_lon,
                                        acl_key,
                                        acl_perm,
                                        region_name,
                                        region_parent,
                                        flood_advert_hrs,
                                        advert_mins,
                                        tx_dbm,
                                        settings_dialog,
                                        transcript,
                                        pending_cli_capture,
                                        need_relogin,
                                    ) }
                                }

                                if on_cli || advanced_cli_open() {
                                    section { class: "rounded-md border border-border bg-background/40 p-3",
                                        div { class: "mb-2 flex flex-wrap items-center justify-between gap-2",
                                            h4 { class: "text-sm font-semibold", "Advanced · Remote CLI · {mode.label()}" }
                                            Button {
                                                kind: ButtonKind::Ghost,
                                                onclick: move |_| transcript.set(Vec::new()),
                                                "Clear"
                                            }
                                        }
                                        if !verified {
                                            p { class: "mb-2 text-xs text-amber-200",
                                                "Verify admin first."
                                            }
                                        }
                                        div { class: "mb-2",
                                            label { class: "mb-1 block text-xs text-muted-foreground",
                                                "Preset ({mode.label()})"
                                            }
                                            SelectInput {
                                                id: "infra-cli-preset".to_string(),
                                                value: preset(),
                                                onchange: move |event: FormEvent| {
                                                    let value = event.value();
                                                    preset.set(value.clone());
                                                    if !value.is_empty() {
                                                        draft.set(value);
                                                    }
                                                },
                                                option {
                                                    value: "",
                                                    style: "background-color:#101418;color:#f4f4f5;",
                                                    "Select a command…"
                                                }
                                                for (cmd, hint) in catalog.iter() {
                                                    option {
                                                        key: "{cmd}",
                                                        value: "{cmd}",
                                                        style: "background-color:#101418;color:#f4f4f5;",
                                                        "{cmd} — {hint}"
                                                    }
                                                }
                                            }
                                        }
                                        pre { class: "infra-cli-transcript mb-2 max-h-56 overflow-y-auto rounded-md border border-border bg-[#0b1220] p-3 font-mono text-xs text-zinc-200",
                                            {
                                                transcript().join("\n")
                                            }
                                        }
                                        div { class: "flex flex-wrap gap-2",
                                            div { class: "min-w-0 flex-1",
                                                TextInput {
                                                    value: draft(),
                                                    placeholder: placeholder.clone(),
                                                    kind: "text".to_string(),
                                                    oninput: move |event: FormEvent| {
                                                        let v = event.value();
                                                        draft.set(v.clone());
                                                        if catalog.iter().any(|(c, _)| *c == v) {
                                                            preset.set(v);
                                                        } else if !preset().is_empty() && preset() != v {
                                                            preset.set(String::new());
                                                        }
                                                    },
                                                }
                                            }
                                            Button {
                                                disabled: !linked || !verified || draft().trim().is_empty(),
                                                onclick: {
                                                    let key = pubkey.clone();
                                                    move |_| {
                                                        let text = draft().trim().to_string();
                                                        if text.is_empty() {
                                                            return;
                                                        }
                                                        if is_destructive_cli(&text) {
                                                            pending.set(text);
                                                            confirm_open.set(true);
                                                            return;
                                                        }
                                                        draft.set(String::new());
                                                        preset.set(String::new());
                                                        run_infra_cli_cmd(
                                                            linked,
                                                            transcript,
                                                            pending_cli_capture,
                                                            need_relogin,
                                                            key.clone(),
                                                            text,
                                                        );
                                                    }
                                                },
                                                Icon { name: IconName::Check, size: 14 }
                                                "Send"
                                            }
                                        }
                                    }
                                }

                                Dialog {
                                    open: confirm_open(),
                                    title: "Confirm destructive command".to_string(),
                                    on_close: move |_| {
                                        confirm_open.set(false);
                                        pending.set(String::new());
                                    },
                                    div { class: "flex flex-col gap-3",
                                        p { class: "text-sm text-muted-foreground",
                                            "This command can reboot or clear stats on the remote node:"
                                        }
                                        p { class: "font-mono text-sm text-foreground", "{pending}" }
                                        div { class: "flex flex-wrap justify-end gap-2",
                                            Button {
                                                kind: ButtonKind::Danger,
                                                onclick: {
                                                    let key = pubkey.clone();
                                                    move |_| {
                                                        let text = pending();
                                                        confirm_open.set(false);
                                                        pending.set(String::new());
                                                        draft.set(String::new());
                                                        preset.set(String::new());
                                                        run_infra_cli_cmd(
                                                            linked,
                                                            transcript,
                                                            pending_cli_capture,
                                                            need_relogin,
                                                            key.clone(),
                                                            text,
                                                        );
                                                    }
                                                },
                                                "Send anyway"
                                            }
                                            Button {
                                                kind: ButtonKind::Ghost,
                                                onclick: move |_| {
                                                    confirm_open.set(false);
                                                    pending.set(String::new());
                                                },
                                                "Cancel"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    None => rsx! {
                        div { class: "settings-subtabs",
                            button {
                                class: if infra_tab() == "claimed" {
                                    "settings-subtab settings-subtab-active"
                                } else {
                                    "settings-subtab"
                                },
                                onclick: move |_| infra_tab.set("claimed".into()),
                                "Claimed ({claimed.len()})"
                            }
                            button {
                                class: if infra_tab() == "claim" {
                                    "settings-subtab settings-subtab-active"
                                } else {
                                    "settings-subtab"
                                },
                                onclick: move |_| infra_tab.set("claim".into()),
                                "Claim"
                            }
                        }

                        if infra_tab() == "claimed" {
                            section { class: "rounded-lg border border-border bg-card/40 p-4",
                                h3 { class: "mb-1 text-sm font-semibold", "Claimed ({claimed.len()})" }
                                p { class: "mb-3 text-xs text-muted-foreground",
                                    "Only admin-verified nodes stay claimed. Password is typed at claim time and never stored — later stats use the node ACL."
                                }
                                if claimed.is_empty() {
                                    p { class: "text-sm text-muted-foreground",
                                        "Nothing claimed yet. Open the Claim tab, pick a repeater / room / sensor, and verify admin over LoRa."
                                    }
                                } else {
                                    div { class: "flex flex-col gap-2",
                                        for (row_i, row) in claimed.iter().enumerate() {
                                            {
                                                let title = display_title(row, &nodes);
                                                let node = nodes.iter().find(|n| n.pubkey == row.pubkey);
                                                let prefix = node
                                                    .map(|n| n.prefix.clone())
                                                    .unwrap_or_else(|| row.pubkey.chars().take(12).collect());
                                                let last = node.and_then(|n| n.last_heard);
                                                let last_label = relative_ago(last);
                                                let stale = last
                                                    .map(|t| unix_now() - t > STALE_HEARD_SECS)
                                                    .unwrap_or(true);
                                                let zebra = if row_i % 2 == 1 { " row-zebra" } else { "" };
                                                let row_class = if stale {
                                                    format!(
                                                        "flex flex-wrap items-center justify-between gap-2 rounded-md border border-amber-500/25 px-3 py-2{zebra}"
                                                    )
                                                } else {
                                                    format!(
                                                        "flex flex-wrap items-center justify-between gap-2 rounded-md border border-border px-3 py-2{zebra}"
                                                    )
                                                };
                                                let key = row.pubkey.clone();
                                                let kind = row.kind.clone();
                                                let verified = row.is_verified();
                                                rsx! {
                                                    div { class: "{row_class}",
                                                        div { class: "min-w-0",
                                                            div { class: "flex flex-wrap items-center gap-2",
                                                                p { class: "truncate font-medium", "{title}" }
                                                                Badge { tone: kind_tone(&kind).to_string(), "{kind_label(&kind)}" }
                                                                if verified {
                                                                    Badge { tone: "enrolled".to_string(), "Verified" }
                                                                } else {
                                                                    Badge { tone: "pending".to_string(), "Unverified" }
                                                                }
                                                                if stale {
                                                                    span { class: "text-[0.65rem] uppercase tracking-wide text-amber-200", "Stale" }
                                                                }
                                                            }
                                                            p { class: "font-mono text-xs text-muted-foreground", "{prefix}" }
                                                            p { class: "text-xs text-muted-foreground",
                                                                "Last heard {last_label} · claimed {relative_ago(Some(row.added_at))}"
                                                            }
                                                        }
                                                        Button {
                                                            kind: ButtonKind::Primary,
                                                            onclick: move |_| {
                                                                kind_draft.set(kind.clone());
                                                                editing_notes.set(false);
                                                                need_relogin.set(false);
                                                                detail_tab.set("status".into());
                                                                selected.set(Some(key.clone()));
                                                            },
                                                            Icon { name: IconName::ChevronRight, size: 14 }
                                                            "Open"
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        if infra_tab() == "claim" {
                            section { class: "rounded-lg border border-border bg-card/40 p-4",
                                h3 { class: "mb-1 text-sm font-semibold", "Claim from roster" }
                                p { class: "mb-3 text-xs text-muted-foreground",
                                    "Requires primary companion + admin password over LoRa. Wrong or guest password does not add the node."
                                }
                                div { class: "mb-3 flex flex-wrap items-end gap-2",
                                    div { class: "min-w-0 flex-1 max-w-md",
                                        TextInput {
                                            value: query(),
                                            placeholder: "Search name, prefix, pubkey…".to_string(),
                                            kind: "text".to_string(),
                                            oninput: move |event: FormEvent| query.set(event.value()),
                                        }
                                    }
                                    div { class: "min-w-[10rem]",
                                        label { class: "mb-1 block text-xs text-muted-foreground", "Claim as" }
                                        SelectInput {
                                            id: "infra-claim-kind".to_string(),
                                            value: claim_kind(),
                                            onchange: move |event: FormEvent| claim_kind.set(event.value()),
                                            option {
                                                value: "",
                                                style: "background-color:#101418;color:#f4f4f5;",
                                                "Auto from type"
                                            }
                                            option { value: "repeater", style: "background-color:#101418;color:#f4f4f5;", "Repeater" }
                                            option { value: "room", style: "background-color:#101418;color:#f4f4f5;", "Room" }
                                            option { value: "sensor", style: "background-color:#101418;color:#f4f4f5;", "Sensor" }
                                        }
                                    }
                                }
                                if candidates.is_empty() {
                                    p { class: "text-sm text-muted-foreground",
                                        if needle.trim().is_empty() {
                                            "No unclaimed repeaters, rooms, or sensors on the roster yet."
                                        } else {
                                            "No matches."
                                        }
                                    }
                                } else {
                                    div { class: "flex flex-col gap-2",
                                        for (row_i, (_score, node, title)) in candidates.into_iter().enumerate() {
                                            {
                                                let key = node.pubkey.clone();
                                                let label = title.clone();
                                                let prefix = node.prefix.clone();
                                                let type_label = match node.contact_type {
                                                    2 => "Repeater",
                                                    3 => "Room",
                                                    4 => "Sensor",
                                                    _ => "Node",
                                                };
                                                let override_kind = claim_kind();
                                                let row_class = if row_i % 2 == 1 {
                                                    "row-zebra flex flex-wrap items-center justify-between gap-2 rounded-md border border-border px-3 py-2"
                                                } else {
                                                    "flex flex-wrap items-center justify-between gap-2 rounded-md border border-border px-3 py-2"
                                                };
                                                rsx! {
                                                    div { class: "{row_class}",
                                                        div { class: "min-w-0",
                                                            p { class: "truncate font-medium", "{title}" }
                                                            p { class: "font-mono text-xs text-muted-foreground", "{prefix}" }
                                                            p { class: "text-xs text-muted-foreground",
                                                                "{type_label} · heard {relative_ago(node.last_heard)}"
                                                            }
                                                        }
                                                        Button {
                                                            kind: ButtonKind::Primary,
                                                            disabled: !linked,
                                                            onclick: move |_| {
                                                                let kind = if override_kind.is_empty() {
                                                                    None
                                                                } else {
                                                                    Some(override_kind.clone())
                                                                };
                                                                claim_error.set(String::new());
                                                                claim_password.set(String::new());
                                                                claim_target.set(Some((
                                                                    key.clone(),
                                                                    label.clone(),
                                                                    kind,
                                                                    false,
                                                                )));
                                                            },
                                                            Icon { name: IconName::Key, size: 14 }
                                                            "Claim…"
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            Dialog {
                open: claim_target().is_some(),
                title: if claim_target().as_ref().map(|t| t.3).unwrap_or(false) {
                    "Verify admin".to_string()
                } else {
                    "Claim infrastructure".to_string()
                },
                on_close: move |_| {
                    if !claim_busy() {
                        claim_target.set(None);
                        claim_password.set(String::new());
                        claim_error.set(String::new());
                    }
                },
                div { class: "flex flex-col gap-3",
                    p { class: "text-sm text-muted-foreground",
                        {
                            if let Some((_, label, _, reverify)) = claim_target() {
                                if reverify {
                                    "Re-enter the node admin password. HQ never stores it — success refreshes the ACL for this companion.".to_string()
                                } else {
                                    format!(
                                        "Enter the MeshCore admin password for {label}. Guest password is refused. Nothing is written until login + ACL proof succeed."
                                    )
                                }
                            } else {
                                "Enter the admin password.".to_string()
                            }
                        }
                    }
                    if let Some((pubkey, _, _, _)) = claim_target() {
                        p { class: "font-mono text-xs text-muted-foreground",
                            "{pubkey.chars().take(12).collect::<String>()}…"
                        }
                    }
                    TextInput {
                        value: claim_password(),
                        placeholder: "Admin password (max 15)".to_string(),
                        kind: "password".to_string(),
                        maxlength: Some(15),
                        oninput: move |event: FormEvent| claim_password.set(event.value()),
                    }
                    if !claim_error().is_empty() {
                        p { class: "text-xs text-amber-200", "{claim_error}" }
                    }
                    if !linked {
                        p { class: "text-xs text-amber-200",
                            "Connect a primary companion before verifying."
                        }
                    }
                    div { class: "flex flex-wrap justify-end gap-2",
                        Button {
                            kind: ButtonKind::Ghost,
                            disabled: claim_busy(),
                            onclick: move |_| {
                                claim_target.set(None);
                                claim_password.set(String::new());
                                claim_error.set(String::new());
                            },
                            "Cancel"
                        }
                        Button {
                            disabled: !linked
                                || claim_busy()
                                || claim_password().trim().is_empty()
                                || claim_target().is_none(),
                            onclick: move |_| {
                                let Some((pubkey, label, kind, reverify)) = claim_target() else {
                                    return;
                                };
                                let password = claim_password();
                                if password.trim().is_empty() {
                                    claim_error.set("Admin password required.".into());
                                    return;
                                }
                                if password.as_bytes().len() > 15 {
                                    claim_error.set("Password max 15 bytes.".into());
                                    return;
                                }
                                claim_busy.set(true);
                                claim_error.set(String::new());
                                let rx = state().radio.login_infra(pubkey.clone(), password);
                                spawn(async move {
                                    let result = rx.await.unwrap_or_else(|_| {
                                        Err("Verify cancelled or radio unavailable.".into())
                                    });
                                    match result {
                                        Ok(()) => {
                                            let write = if reverify
                                                && state()
                                                    .db
                                                    .claimed_pubkey_set()
                                                    .ok()
                                                    .map(|s| s.contains(&pubkey))
                                                    .unwrap_or(false)
                                            {
                                                state().db.mark_claimed_verified(&pubkey)
                                            } else {
                                                state().db.claim_infra_verified(
                                                    &pubkey,
                                                    kind.as_deref(),
                                                    &label,
                                                    "",
                                                )
                                            };
                                            match write {
                                                Ok(()) => {
                                                    claim_target.set(None);
                                                    claim_password.set(String::new());
                                                    claim_error.set(String::new());
                                                    need_relogin.set(false);
                                                    note.set(if reverify {
                                                        format!("Verified {label}.")
                                                    } else {
                                                        format!("Claimed and verified {label}.")
                                                    });
                                                    kind_draft.set(
                                                        kind.unwrap_or_else(|| "repeater".into()),
                                                    );
                                                    detail_tab.set("status".into());
                                                    selected.set(Some(pubkey));
                                                    infra_tab.set("claimed".into());
                                                    tick.with_mut(|n| *n += 1);
                                                }
                                                Err(err) => claim_error.set(err.to_string()),
                                            }
                                        }
                                        Err(err) => claim_error.set(err),
                                    }
                                    claim_busy.set(false);
                                });
                            },
                            Icon { name: IconName::Key, size: 14 }
                            if claim_busy() {
                                "Verifying…"
                            } else if claim_target().as_ref().map(|t| t.3).unwrap_or(false) {
                                "Verify"
                            } else {
                                "Verify and claim"
                            }
                        }
                    }
                }
            }

            Dialog {
                open: settings_dialog().as_deref() == Some("admin_pw"),
                title: "Admin password".to_string(),
                on_close: move |_| {
                    settings_dialog.set(None);
                    admin_pw.set(String::new());
                },
                div { class: "flex flex-col gap-3",
                    p { class: "text-sm text-muted-foreground",
                        "Sets the remote admin password via LoRa. HQ never stores it. The node may echo the new password — the transcript masks that reply."
                    }
                    TextInput {
                        value: admin_pw(),
                        placeholder: "New admin password (max 15)".to_string(),
                        kind: "password".to_string(),
                        maxlength: Some(15),
                        oninput: move |e: FormEvent| admin_pw.set(e.value()),
                    }
                    div { class: "flex justify-end gap-2",
                        Button {
                            kind: ButtonKind::Ghost,
                            onclick: move |_| {
                                settings_dialog.set(None);
                                admin_pw.set(String::new());
                            },
                            "Cancel"
                        }
                        Button {
                            disabled: admin_pw().trim().is_empty() || selected().is_none(),
                            onclick: move |_| {
                                if let Some(key) = selected() {
                                    let pw = admin_pw().trim().to_string();
                                    if !pw.is_empty() && pw.as_bytes().len() <= 15 {
                                        run_infra_cli_cmd(
                                            linked,
                                            transcript,
                                            pending_cli_capture,
                                            need_relogin,
                                            key,
                                            format!("password {pw}"),
                                        );
                                    }
                                }
                                settings_dialog.set(None);
                                admin_pw.set(String::new());
                            },
                            "Send"
                        }
                    }
                }
            }

            Dialog {
                open: settings_dialog().as_deref() == Some("guest_pw"),
                title: "Guest password".to_string(),
                on_close: move |_| {
                    settings_dialog.set(None);
                    guest_pw.set(String::new());
                },
                div { class: "flex flex-col gap-3",
                    p { class: "text-sm text-muted-foreground",
                        "Sets guest.password on the node. Never stored in HQ."
                    }
                    TextInput {
                        value: guest_pw(),
                        placeholder: "New guest password".to_string(),
                        kind: "password".to_string(),
                        maxlength: Some(15),
                        oninput: move |e: FormEvent| guest_pw.set(e.value()),
                    }
                    div { class: "flex justify-end gap-2",
                        Button {
                            kind: ButtonKind::Ghost,
                            onclick: move |_| {
                                settings_dialog.set(None);
                                guest_pw.set(String::new());
                            },
                            "Cancel"
                        }
                        Button {
                            disabled: guest_pw().trim().is_empty() || selected().is_none(),
                            onclick: move |_| {
                                if let Some(key) = selected() {
                                    let pw = guest_pw().trim().to_string();
                                    if !pw.is_empty() {
                                        run_infra_cli_cmd(
                                            linked,
                                            transcript,
                                            pending_cli_capture,
                                            need_relogin,
                                            key,
                                            format!("set guest.password {pw}"),
                                        );
                                    }
                                }
                                settings_dialog.set(None);
                                guest_pw.set(String::new());
                            },
                            "Send"
                        }
                    }
                }
            }
        }
    }
}

fn start_status_refresh(
    linked: bool,
    pubkey: String,
    mut last_probe: Signal<HashMap<String, i64>>,
    mut probe_busy: Signal<bool>,
    mut status_note: Signal<String>,
    transcript: Signal<Vec<String>>,
    mut pending_cli_capture: Signal<Option<(String, String)>>,
    mut need_relogin: Signal<bool>,
    mut tick: Signal<u64>,
) {
    if !linked {
        status_note.set("Connect a primary companion before refreshing status.".into());
        return;
    }
    let now = unix_now();
    let last = last_probe().get(&pubkey).copied().unwrap_or(0);
    if now - last < PROBE_COOLDOWN_SECS {
        status_note.set(format!(
            "Rate-limited — wait {}s.",
            PROBE_COOLDOWN_SECS - (now - last)
        ));
        return;
    }
    last_probe.with_mut(|m| {
        m.insert(pubkey.clone(), now);
    });
    probe_busy.set(true);
    status_note.set("Refreshing status…".into());
    let prior_tx = state()
        .db
        .get_infra_status_latest(&pubkey)
        .ok()
        .flatten()
        .and_then(|r| r.tx_power_dbm);
    let rx = state().radio.probe_infra_status(pubkey.clone());
    spawn(async move {
        let result = rx
            .await
            .unwrap_or_else(|_| Err("Status refresh cancelled.".into()));
        match result {
            Ok(info) => {
                persist_status_data(&pubkey, &info, prior_tx);
                status_note.set("Status updated.".into());
                for (i, cmd) in STATUS_CLI_FOLLOWUP.iter().enumerate() {
                    if i > 0 {
                        tokio::time::sleep(std::time::Duration::from_millis(900)).await;
                    }
                    pending_cli_capture.set(Some((pubkey.clone(), (*cmd).to_string())));
                    append_transcript(transcript, &format!("> {cmd}"));
                    state()
                        .radio
                        .run_infra_cli(pubkey.clone(), (*cmd).to_string());
                    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                }
                tick.with_mut(|n| *n += 1);
            }
            Err(err) => {
                status_note.set(err.clone());
                if err.to_ascii_lowercase().contains("acl")
                    || err.to_ascii_lowercase().contains("timeout")
                {
                    need_relogin.set(true);
                }
            }
        }
        probe_busy.set(false);
    });
}

fn run_infra_cli_cmd(
    linked: bool,
    transcript: Signal<Vec<String>>,
    mut pending_cli_capture: Signal<Option<(String, String)>>,
    mut need_relogin: Signal<bool>,
    pubkey: String,
    text: String,
) {
    if !linked {
        append_transcript(
            transcript,
            "Not connected — link a primary companion before probing infrastructure.",
        );
        return;
    }
    if pubkey.trim().is_empty() {
        append_transcript(transcript, "Select a claimed node first.");
        return;
    }
    append_transcript(transcript, &format!("> {text}"));
    append_transcript(transcript, "… waiting for reply");
    let snap = state().radio.snapshot();
    let expected_seq = snap.cli_seq;
    let cmd_key = text.trim().to_string();
    if STATUS_CLI_FOLLOWUP.contains(&cmd_key.as_str())
        || cmd_key == "get guest.password"
        || cmd_key.starts_with("region")
    {
        pending_cli_capture.set(Some((pubkey.clone(), cmd_key)));
    }
    state().radio.run_infra_cli(pubkey, text);
    spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(18)).await;
        let snap = state().radio.snapshot();
        if snap.cli_seq == expected_seq {
            let hint = if snap.sync_phase != crate::radio::SyncPhase::Ready
                && snap.sync_phase != crate::radio::SyncPhase::Idle
            {
                format!(
                    "error: no CLI reply (timeout). Radio is busy ({}) — try again when Ready.",
                    snap.sync_label
                )
            } else {
                "error: no CLI reply (timeout). ACL may have expired — use Verify admin again."
                    .into()
            };
            append_transcript(transcript, &hint);
            need_relogin.set(true);
            pending_cli_capture.set(None);
        }
    });
}

fn status_metric(label: &str, value: String) -> Element {
    rsx! {
        div { class: "infra-status-metric",
            p { class: "infra-status-metric__label", "{label}" }
            p { class: "infra-status-metric__value", "{value}" }
        }
    }
}

fn status_panel(latest: Option<&InfraStatusLatest>) -> Element {
    let Some(s) = latest else {
        return rsx! {
            p { class: "text-sm text-muted-foreground",
                "No status sample yet. Verify admin, then tap Refresh status."
            }
        };
    };
    let pct = battery_pct_from_mv(s.battery_mv);
    let batt = format!("{pct}% / {:.2} V", s.battery_mv as f64 / 1000.0);
    let clock = if s.clock.is_empty() {
        "—".into()
    } else {
        s.clock.clone()
    };
    let tx_power = s
        .tx_power_dbm
        .map(|v| format!("{v} dBm"))
        .unwrap_or_else(|| "—".into());
    let packets = format!(
        "{} total · {} flood · {} direct · {} recv",
        s.nb_sent, s.flood_sent, s.direct_sent, s.nb_recv
    );
    let ver = if s.ver.is_empty() {
        "—".into()
    } else {
        s.ver.clone()
    };
    let radio = if s.radio.is_empty() {
        "—".into()
    } else {
        s.radio.clone()
    };
    let neighbors = if s.neighbors.is_empty() {
        "—".into()
    } else {
        s.neighbors.clone()
    };
    rsx! {
        div { class: "infra-status-list",
            p { class: "mb-2 text-xs text-muted-foreground",
                "Probed {relative_ago(Some(s.probed_at))}"
            }
            { status_metric("Battery", batt) }
            { status_metric("Clock", clock) }
            { status_metric("Uptime", format_uptime(s.uptime_s)) }
            { status_metric("Airtime TX", format_airtime(s.airtime_ms)) }
            { status_metric("Airtime RX", format_airtime(s.rx_airtime_ms)) }
            { status_metric("Last RSSI", format!("{}", s.last_rssi)) }
            { status_metric("Last SNR", format!("{:.1}", s.snr)) }
            { status_metric("Noise floor", format!("{} dBm", s.noise_floor)) }
            { status_metric("Packets", packets) }
            { status_metric("TX power", tx_power) }
            { status_metric("Firmware", ver) }
            { status_metric("Radio", radio) }
            { status_metric("Neighbors", neighbors) }
        }
    }
}

fn settings_row(
    id: &'static str,
    icon: IconName,
    title: &'static str,
    sub: &'static str,
    open: &Option<String>,
    mut settings_panel: Signal<Option<String>>,
    enabled: bool,
) -> Element {
    let active = open.as_deref() == Some(id);
    let disabled = !enabled && id != "identity";
    let open_now = open.clone();
    rsx! {
        button {
            class: if active {
                "infra-settings-row infra-settings-row-active"
            } else {
                "infra-settings-row"
            },
            disabled: disabled,
            onclick: move |_| {
                if open_now.as_deref() == Some(id) {
                    settings_panel.set(None);
                } else {
                    settings_panel.set(Some(id.to_string()));
                }
            },
            span { class: "infra-settings-row__icon",
                Icon { name: icon, size: 16 }
            }
            span { class: "min-w-0 flex-1 text-left",
                span { class: "block text-sm font-medium text-foreground", "{title}" }
                span { class: "block text-xs text-muted-foreground", "{sub}" }
            }
            Icon { name: IconName::ChevronRight, size: 14 }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn settings_list(
    enabled: bool,
    linked: bool,
    pubkey: &str,
    node: Option<&crate::db::NodeRow>,
    latest: Option<&InfraStatusLatest>,
    settings_panel: Signal<Option<String>>,
    mut pos_lat: Signal<String>,
    mut pos_lon: Signal<String>,
    mut acl_key: Signal<String>,
    mut acl_perm: Signal<String>,
    mut region_name: Signal<String>,
    mut region_parent: Signal<String>,
    mut flood_advert_hrs: Signal<String>,
    mut advert_mins: Signal<String>,
    mut tx_dbm: Signal<String>,
    mut settings_dialog: Signal<Option<String>>,
    transcript: Signal<Vec<String>>,
    pending_cli_capture: Signal<Option<(String, String)>>,
    need_relogin: Signal<bool>,
) -> Element {
    let key = pubkey.to_string();
    let open = settings_panel();
    let lat_hint = node.and_then(|n| n.lat).map(|v| format!("{v:.6}")).unwrap_or_default();
    let lon_hint = node.and_then(|n| n.lon).map(|v| format!("{v:.6}")).unwrap_or_default();
    let tx_hint = latest
        .and_then(|s| s.tx_power_dbm)
        .map(|v| v.to_string())
        .unwrap_or_default();

    rsx! {
        div { class: "infra-settings-list",
            { settings_row("position", IconName::Locate, "Position", "Set repeater position on the map.", &open, settings_panel, enabled) }
            if open.as_deref() == Some("position") {
                div { class: "infra-settings-panel",
                    p { class: "mb-2 text-xs text-muted-foreground",
                        "Current GPS: {lat_hint}, {lon_hint}"
                    }
                    div { class: "mb-2 grid grid-cols-2 gap-2",
                        TextInput {
                            value: pos_lat(),
                            placeholder: "Latitude".to_string(),
                            kind: "text".to_string(),
                            oninput: move |e: FormEvent| pos_lat.set(e.value()),
                        }
                        TextInput {
                            value: pos_lon(),
                            placeholder: "Longitude".to_string(),
                            kind: "text".to_string(),
                            oninput: move |e: FormEvent| pos_lon.set(e.value()),
                        }
                    }
                    div { class: "flex flex-wrap gap-2",
                        Button {
                            kind: ButtonKind::Ghost,
                            onclick: move |_| {
                                if let Some((_, lat, lon)) = state().db.hq_position().ok().flatten() {
                                    pos_lat.set(format!("{lat:.6}"));
                                    pos_lon.set(format!("{lon:.6}"));
                                }
                            },
                            "Use HQ GPS"
                        }
                        Button {
                            disabled: pos_lat().trim().is_empty() || pos_lon().trim().is_empty(),
                            onclick: {
                                let key = key.clone();
                                move |_| {
                                    let lat = pos_lat().trim().to_string();
                                    let lon = pos_lon().trim().to_string();
                                    run_infra_cli_cmd(linked, transcript, pending_cli_capture, need_relogin, key.clone(), format!("set lat {lat}"));
                                    run_infra_cli_cmd(linked, transcript, pending_cli_capture, need_relogin, key.clone(), format!("set lon {lon}"));
                                }
                            },
                            "Apply"
                        }
                    }
                }
            }

            { settings_row("clock", IconName::Clock, "Sync Clock", "Updates the repeater clock.", &open, settings_panel, enabled) }
            if open.as_deref() == Some("clock") {
                div { class: "infra-settings-panel",
                    Button {
                        onclick: {
                            let key = key.clone();
                            move |_| run_infra_cli_cmd(linked, transcript, pending_cli_capture, need_relogin, key.clone(), "clock sync".into())
                        },
                        "Sync clock now"
                    }
                }
            }

            { settings_row("acl", IconName::Users, "Access Control", "Add or remove users.", &open, settings_panel, enabled) }
            if open.as_deref() == Some("acl") {
                div { class: "infra-settings-panel",
                    p { class: "mb-2 text-xs text-muted-foreground",
                        "Full ACL dump is serial-only. Use setperm over LoRa (0 guest … 3 admin; omit level to remove)."
                    }
                    TextInput {
                        value: acl_key(),
                        placeholder: "Pubkey hex".to_string(),
                        kind: "text".to_string(),
                        oninput: move |e: FormEvent| acl_key.set(e.value()),
                    }
                    div { class: "mt-2 flex flex-wrap items-end gap-2",
                        div { class: "min-w-[8rem]",
                            SelectInput {
                                id: "infra-acl-perm".to_string(),
                                value: acl_perm(),
                                onchange: move |e: FormEvent| acl_perm.set(e.value()),
                                option { value: "0", style: "background-color:#101418;color:#f4f4f5;", "0 · Guest" }
                                option { value: "1", style: "background-color:#101418;color:#f4f4f5;", "1 · Read-only" }
                                option { value: "2", style: "background-color:#101418;color:#f4f4f5;", "2 · Read-write" }
                                option { value: "3", style: "background-color:#101418;color:#f4f4f5;", "3 · Admin" }
                                option { value: "", style: "background-color:#101418;color:#f4f4f5;", "Remove" }
                            }
                        }
                        Button {
                            disabled: acl_key().trim().is_empty(),
                            onclick: {
                                let key = key.clone();
                                move |_| {
                                    let pk = acl_key().trim().to_string();
                                    let perm = acl_perm();
                                    let cmd = if perm.is_empty() {
                                        format!("setperm {pk}")
                                    } else {
                                        format!("setperm {pk} {perm}")
                                    };
                                    run_infra_cli_cmd(linked, transcript, pending_cli_capture, need_relogin, key.clone(), cmd);
                                }
                            },
                            "Apply setperm"
                        }
                    }
                }
            }

            { settings_row("admin_pw", IconName::Key, "Admin Password", "Set remote management password.", &open, settings_panel, enabled) }
            if open.as_deref() == Some("admin_pw") {
                div { class: "infra-settings-panel",
                    Button {
                        onclick: move |_| settings_dialog.set(Some("admin_pw".into())),
                        "Change admin password…"
                    }
                }
            }

            { settings_row("guest_pw", IconName::Key, "Guest Password", "Set repeater status password.", &open, settings_panel, enabled) }
            if open.as_deref() == Some("guest_pw") {
                div { class: "infra-settings-panel",
                    div { class: "flex flex-wrap gap-2",
                        Button {
                            kind: ButtonKind::Ghost,
                            onclick: {
                                let key = key.clone();
                                move |_| run_infra_cli_cmd(linked, transcript, pending_cli_capture, need_relogin, key.clone(), "get guest.password".into())
                            },
                            "Get guest password"
                        }
                        Button {
                            onclick: move |_| settings_dialog.set(Some("guest_pw".into())),
                            "Set guest password…"
                        }
                    }
                }
            }

            { settings_row("identity", IconName::Key, "Change Identity Key", "Requires USB serial / flash — not available over LoRa.", &open, settings_panel, enabled) }

            { settings_row("regions", IconName::Track, "Manage Regions", "Configure regions to repeat.", &open, settings_panel, enabled) }
            if open.as_deref() == Some("regions") {
                div { class: "infra-settings-panel",
                    div { class: "mb-2 flex flex-wrap gap-2",
                        Button {
                            kind: ButtonKind::Ghost,
                            onclick: {
                                let key = key.clone();
                                move |_| run_infra_cli_cmd(linked, transcript, pending_cli_capture, need_relogin, key.clone(), "region".into())
                            },
                            "Dump regions"
                        }
                        Button {
                            kind: ButtonKind::Ghost,
                            onclick: {
                                let key = key.clone();
                                move |_| run_infra_cli_cmd(linked, transcript, pending_cli_capture, need_relogin, key.clone(), "region list allowed".into())
                            },
                            "List allowed"
                        }
                        Button {
                            kind: ButtonKind::Ghost,
                            onclick: {
                                let key = key.clone();
                                move |_| run_infra_cli_cmd(linked, transcript, pending_cli_capture, need_relogin, key.clone(), "region save".into())
                            },
                            "Save"
                        }
                    }
                    TextInput {
                        value: region_name(),
                        placeholder: "Region name".to_string(),
                        kind: "text".to_string(),
                        oninput: move |e: FormEvent| region_name.set(e.value()),
                    }
                    TextInput {
                        value: region_parent(),
                        placeholder: "Parent (* for root)".to_string(),
                        kind: "text".to_string(),
                        oninput: move |e: FormEvent| region_parent.set(e.value()),
                    }
                    div { class: "mt-2 flex flex-wrap gap-2",
                        Button {
                            disabled: region_name().trim().is_empty(),
                            onclick: {
                                let key = key.clone();
                                move |_| {
                                    let name = region_name().trim().to_string();
                                    let parent = region_parent().trim().to_string();
                                    run_infra_cli_cmd(linked, transcript, pending_cli_capture, need_relogin, key.clone(), format!("region put {name} {parent}"));
                                }
                            },
                            "Put"
                        }
                        Button {
                            kind: ButtonKind::Ghost,
                            disabled: region_name().trim().is_empty(),
                            onclick: {
                                let key = key.clone();
                                move |_| {
                                    let name = region_name().trim().to_string();
                                    run_infra_cli_cmd(linked, transcript, pending_cli_capture, need_relogin, key.clone(), format!("region allowf {name}"));
                                }
                            },
                            "Allow flood"
                        }
                        Button {
                            kind: ButtonKind::Ghost,
                            disabled: region_name().trim().is_empty(),
                            onclick: {
                                let key = key.clone();
                                move |_| {
                                    let name = region_name().trim().to_string();
                                    run_infra_cli_cmd(linked, transcript, pending_cli_capture, need_relogin, key.clone(), format!("region denyf {name}"));
                                }
                            },
                            "Deny flood"
                        }
                    }
                }
            }

            { settings_row("advert", IconName::Signal, "Advert", "Send flood or zero-hop advert now.", &open, settings_panel, enabled) }
            if open.as_deref() == Some("advert") {
                div { class: "infra-settings-panel",
                    div { class: "flex flex-wrap gap-2",
                        Button {
                            onclick: {
                                let key = key.clone();
                                move |_| run_infra_cli_cmd(linked, transcript, pending_cli_capture, need_relogin, key.clone(), "advert".into())
                            },
                            "Advert flood"
                        }
                        Button {
                            kind: ButtonKind::Ghost,
                            onclick: {
                                let key = key.clone();
                                move |_| run_infra_cli_cmd(linked, transcript, pending_cli_capture, need_relogin, key.clone(), "advert.zerohop".into())
                            },
                            "Advert zero-hop"
                        }
                    }
                }
            }

            { settings_row("intervals", IconName::Clock, "Advert intervals", "Flood hours / zero-hop minutes.", &open, settings_panel, enabled) }
            if open.as_deref() == Some("intervals") {
                div { class: "infra-settings-panel",
                    div { class: "mb-2 flex flex-wrap gap-2",
                        Button {
                            kind: ButtonKind::Ghost,
                            onclick: {
                                let key = key.clone();
                                move |_| run_infra_cli_cmd(linked, transcript, pending_cli_capture, need_relogin, key.clone(), "get flood.advert.interval".into())
                            },
                            "Get flood interval"
                        }
                        Button {
                            kind: ButtonKind::Ghost,
                            onclick: {
                                let key = key.clone();
                                move |_| run_infra_cli_cmd(linked, transcript, pending_cli_capture, need_relogin, key.clone(), "get advert.interval".into())
                            },
                            "Get zero-hop interval"
                        }
                    }
                    div { class: "grid grid-cols-2 gap-2",
                        TextInput {
                            value: flood_advert_hrs(),
                            placeholder: "Flood hours".to_string(),
                            kind: "text".to_string(),
                            oninput: move |e: FormEvent| flood_advert_hrs.set(e.value()),
                        }
                        TextInput {
                            value: advert_mins(),
                            placeholder: "Zero-hop minutes".to_string(),
                            kind: "text".to_string(),
                            oninput: move |e: FormEvent| advert_mins.set(e.value()),
                        }
                    }
                    div { class: "mt-2 flex flex-wrap gap-2",
                        Button {
                            disabled: flood_advert_hrs().trim().is_empty(),
                            onclick: {
                                let key = key.clone();
                                move |_| {
                                    let v = flood_advert_hrs().trim().to_string();
                                    run_infra_cli_cmd(linked, transcript, pending_cli_capture, need_relogin, key.clone(), format!("set flood.advert.interval {v}"));
                                }
                            },
                            "Set flood interval"
                        }
                        Button {
                            disabled: advert_mins().trim().is_empty(),
                            onclick: {
                                let key = key.clone();
                                move |_| {
                                    let v = advert_mins().trim().to_string();
                                    run_infra_cli_cmd(linked, transcript, pending_cli_capture, need_relogin, key.clone(), format!("set advert.interval {v}"));
                                }
                            },
                            "Set zero-hop interval"
                        }
                    }
                }
            }

            { settings_row("tx", IconName::Radio, "Radio / TX power", "Antenna TX power (reboot may be required).", &open, settings_panel, enabled) }
            if open.as_deref() == Some("tx") {
                div { class: "infra-settings-panel",
                    p { class: "mb-2 text-xs text-muted-foreground",
                        "Last known: {tx_hint} dBm"
                    }
                    div { class: "flex flex-wrap gap-2",
                        Button {
                            kind: ButtonKind::Ghost,
                            onclick: {
                                let key = key.clone();
                                move |_| run_infra_cli_cmd(linked, transcript, pending_cli_capture, need_relogin, key.clone(), "get tx".into())
                            },
                            "Get TX"
                        }
                        TextInput {
                            value: tx_dbm(),
                            placeholder: "dBm".to_string(),
                            kind: "text".to_string(),
                            oninput: move |e: FormEvent| tx_dbm.set(e.value()),
                        }
                        Button {
                            disabled: tx_dbm().trim().is_empty(),
                            onclick: {
                                let key = key.clone();
                                move |_| {
                                    let v = tx_dbm().trim().to_string();
                                    run_infra_cli_cmd(linked, transcript, pending_cli_capture, need_relogin, key.clone(), format!("set tx {v}"));
                                }
                            },
                            "Set TX"
                        }
                        Button {
                            kind: ButtonKind::Ghost,
                            onclick: {
                                let key = key.clone();
                                move |_| run_infra_cli_cmd(linked, transcript, pending_cli_capture, need_relogin, key.clone(), "get radio".into())
                            },
                            "Get radio"
                        }
                    }
                }
            }

            { settings_row("repeat", IconName::Signal, "Repeat / path", "Repeat flag and flood limits.", &open, settings_panel, enabled) }
            if open.as_deref() == Some("repeat") {
                div { class: "infra-settings-panel",
                    div { class: "flex flex-wrap gap-2",
                        Button {
                            kind: ButtonKind::Ghost,
                            onclick: {
                                let key = key.clone();
                                move |_| run_infra_cli_cmd(linked, transcript, pending_cli_capture, need_relogin, key.clone(), "get repeat".into())
                            },
                            "Get repeat"
                        }
                        Button {
                            kind: ButtonKind::Ghost,
                            onclick: {
                                let key = key.clone();
                                move |_| run_infra_cli_cmd(linked, transcript, pending_cli_capture, need_relogin, key.clone(), "get flood.max".into())
                            },
                            "Get flood.max"
                        }
                        Button {
                            kind: ButtonKind::Ghost,
                            onclick: {
                                let key = key.clone();
                                move |_| run_infra_cli_cmd(linked, transcript, pending_cli_capture, need_relogin, key.clone(), "get path.hash.mode".into())
                            },
                            "Get path hash"
                        }
                    }
                }
            }

            { settings_row("maint", IconName::Trash, "Maintenance", "Clear stats or reboot.", &open, settings_panel, enabled) }
            if open.as_deref() == Some("maint") {
                div { class: "infra-settings-panel",
                    div { class: "flex flex-wrap gap-2",
                        Button {
                            kind: ButtonKind::Ghost,
                            onclick: {
                                let key = key.clone();
                                move |_| run_infra_cli_cmd(linked, transcript, pending_cli_capture, need_relogin, key.clone(), "clear stats".into())
                            },
                            "Clear stats"
                        }
                        Button {
                            kind: ButtonKind::Danger,
                            onclick: {
                                let key = key.clone();
                                move |_| run_infra_cli_cmd(linked, transcript, pending_cli_capture, need_relogin, key.clone(), "reboot".into())
                            },
                            "Reboot"
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{is_cli_capture_noise, parse_tx_power};

    #[test]
    fn parse_tx_power_reads_plain_and_labeled() {
        assert_eq!(parse_tx_power("22"), Some(22));
        assert_eq!(parse_tx_power("TX power: 14 dBm"), Some(14));
        assert_eq!(parse_tx_power("-5"), Some(-5));
        assert_eq!(parse_tx_power("99"), None);
    }

    #[test]
    fn capture_ignores_send_ack_noise() {
        assert!(is_cli_capture_noise(
            "CLI command sent to contact (replies arrive as mesh messages)."
        ));
        assert!(is_cli_capture_noise("… waiting for reply"));
        assert!(!is_cli_capture_noise("22"));
        assert!(!is_cli_capture_noise("TX power: 14 dBm"));
    }
}
