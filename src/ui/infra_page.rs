//! Operator-claimed mesh infrastructure (repeaters / rooms / sensors).
//! Traffic goes through the primary companion over LoRa — never treat these as pool radios.

use std::collections::HashMap;

use dioxus::prelude::*;

use crate::db::{unix_now, ClaimedInfraRow};
use crate::signup::Enrollment;

use super::cli_page::{
    commands_for, is_destructive_cli, CliMode, STATUS_PROBE_COMMANDS,
};
use super::icons::{Icon, IconName};
use super::state;
use super::widgets::{
    Badge, Button, ButtonKind, Dialog, PageHeading, SelectInput, TextInput,
};
use crate::radio::RadioStatus;

const PROBE_COOLDOWN_SECS: i64 = 60;
const STALE_HEARD_SECS: i64 = 30 * 60;

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
            lines.push(part.to_string());
        }
        if lines.len() > 400 {
            let skip = lines.len() - 400;
            lines.drain(0..skip);
        }
    });
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

#[component]
pub fn InfraPage(radio: RadioStatus) -> Element {
    let mut tick = use_signal(|| 0u64);
    let mut query = use_signal(String::new);
    let mut selected = use_signal(|| None::<String>);
    let mut notes_draft = use_signal(String::new);
    let mut editing_notes = use_signal(|| false);
    let mut kind_draft = use_signal(|| "repeater".to_string());
    let mut note = use_signal(String::new);
    let mut claim_kind = use_signal(|| String::new());
    // Pending claim/re-verify: (pubkey, label, kind_override, reverify_existing).
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
    let mut last_probe = use_signal(HashMap::<String, i64>::new);
    let mut probe_busy = use_signal(|| false);
    let mut discover_note = use_signal(String::new);
    let mut need_relogin = use_signal(|| false);

    if !poll_started() {
        poll_started.set(true);
        spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                poll.with_mut(|n| *n = n.wrapping_add(1));
            }
        });
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

    let mut candidates: Vec<_> = nodes
        .iter()
        .filter(|n| n.enrollment != Enrollment::Hq && !claimed_keys.contains(&n.pubkey))
        .filter(|n| {
            // Prefer infra types; still allow override via empty kind filter + any node.
            matches!(n.contact_type, 2 | 3 | 4) || !claim_kind().is_empty()
        })
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
        append_transcript(transcript, &snap.cli_line);
    });

    let send_command = move |pubkey: String, text: String| {
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
                    "error: no CLI reply (timeout). ACL may have expired — use Verify admin again.".into()
                };
                append_transcript(transcript, &hint);
                need_relogin.set(true);
            }
        });
    };

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
                                            "Verify admin over LoRa before status probe or remote CLI."
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

                            div { class: "mb-4 flex flex-wrap items-center gap-2",
                                Button {
                                    disabled: !can_probe,
                                    onclick: {
                                        let key = pubkey.clone();
                                        move |_| {
                                            if !linked {
                                                append_transcript(
                                                    transcript,
                                                    "Connect a primary companion before status probe.",
                                                );
                                                return;
                                            }
                                            let now = unix_now();
                                            let last = last_probe().get(&key).copied().unwrap_or(0);
                                            if now - last < PROBE_COOLDOWN_SECS {
                                                append_transcript(
                                                    transcript,
                                                    &format!(
                                                        "Probe rate-limited — wait {}s.",
                                                        PROBE_COOLDOWN_SECS - (now - last)
                                                    ),
                                                );
                                                return;
                                            }
                                            last_probe.with_mut(|map| {
                                                map.insert(key.clone(), now);
                                            });
                                            probe_busy.set(true);
                                            append_transcript(
                                                transcript,
                                                &format!(
                                                    "— status probe ({})",
                                                    STATUS_PROBE_COMMANDS.join(", ")
                                                ),
                                            );
                                            let cmds: Vec<String> = STATUS_PROBE_COMMANDS
                                                .iter()
                                                .map(|s| (*s).to_string())
                                                .collect();
                                            let key2 = key.clone();
                                            spawn(async move {
                                                for (i, cmd) in cmds.into_iter().enumerate() {
                                                    if i > 0 {
                                                        tokio::time::sleep(
                                                            std::time::Duration::from_millis(900),
                                                        )
                                                        .await;
                                                    }
                                                    let snap = state().radio.snapshot();
                                                    let expected = snap.cli_seq;
                                                    append_transcript(transcript, &format!("> {cmd}"));
                                                    state().radio.run_infra_cli(key2.clone(), cmd);
                                                    tokio::time::sleep(
                                                        std::time::Duration::from_secs(12),
                                                    )
                                                    .await;
                                                    let after = state().radio.snapshot();
                                                    if after.cli_seq == expected {
                                                        append_transcript(
                                                            transcript,
                                                            "error: probe step timed out",
                                                        );
                                                    }
                                                }
                                                probe_busy.set(false);
                                            });
                                        }
                                    },
                                    Icon { name: IconName::Scan, size: 14 }
                                    {
                                        if probe_busy() {
                                            "Probing…".to_string()
                                        } else if probe_wait > 0 {
                                            format!("Probe ({probe_wait}s)")
                                        } else {
                                            "Status probe".to_string()
                                        }
                                    }
                                }
                                Button {
                                    kind: ButtonKind::Ghost,
                                    disabled: !linked,
                                    onclick: move |_| {
                                        discover_note.set(String::new());
                                        let rx = state().radio.discover_nodes(2); // filter: repeaters
                                        spawn(async move {
                                            match rx.await {
                                                Ok(Ok(summary)) => {
                                                    discover_note.set(format!(
                                                        "Discover: {} returned, {} added to roster",
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
                                if !discover_note().is_empty() {
                                    span { class: "text-xs text-muted-foreground", "{discover_note}" }
                                }
                            }

                            if !verified {
                                p { class: "mb-3 rounded-md border border-border bg-card/40 px-3 py-2 text-sm text-muted-foreground",
                                    "Status probe and remote CLI unlock after admin verification."
                                }
                            }
                            section { class: "rounded-md border border-border bg-background/40 p-3",
                                div { class: "mb-2 flex flex-wrap items-center justify-between gap-2",
                                    h4 { class: "text-sm font-semibold", "Remote CLI · {mode.label()}" }
                                    Button {
                                        kind: ButtonKind::Ghost,
                                        onclick: move |_| transcript.set(Vec::new()),
                                        "Clear"
                                    }
                                }
                                if !verified {
                                    p { class: "mb-2 text-xs text-amber-200",
                                        "Verify admin first. Password is typed here only — never stored in HQ."
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
                                pre { class: "cli-transcript mb-2 max-h-56",
                                    for (idx, line) in transcript().iter().enumerate() {
                                        span { key: "{idx}", "{line}\n" }
                                    }
                                }
                                div { class: "flex gap-2",
                                    TextInput {
                                        value: draft(),
                                        placeholder: "{placeholder}",
                                        oninput: move |event: FormEvent| {
                                            let v = event.value();
                                            draft.set(v.clone());
                                            if catalog.iter().any(|(cmd, _)| *cmd == v) {
                                                preset.set(v);
                                            } else if !preset().is_empty() && preset() != v {
                                                preset.set(String::new());
                                            }
                                        },
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
                                                } else {
                                                    draft.set(String::new());
                                                    preset.set(String::new());
                                                    send_command(key.clone(), text);
                                                }
                                            }
                                        },
                                        Icon { name: IconName::Check, size: 14 }
                                        "Send"
                                    }
                                }
                            }
                            Dialog {
                                open: confirm_open(),
                                title: "Confirm destructive CLI".to_string(),
                                on_close: move |_| {
                                    confirm_open.set(false);
                                    pending.set(String::new());
                                },
                                p { class: "mb-3 text-sm text-muted-foreground",
                                    "Send `{pending()}` to this claimed node? This may reboot or erase it."
                                }
                                div { class: "flex flex-wrap items-center gap-2",
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
                                                send_command(key.clone(), text);
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
                    None => rsx! {
                    section { class: "rounded-lg border border-border bg-card/40 p-4",
                        h3 { class: "mb-1 text-sm font-semibold", "Claimed ({claimed.len()})" }
                        p { class: "mb-3 text-xs text-muted-foreground",
                            "Only admin-verified nodes stay claimed. Password is typed at claim time and never stored — later stats use the node ACL."
                        }
                        if claimed.is_empty() {
                            p { class: "text-sm text-muted-foreground",
                                "Nothing claimed yet. Add a repeater, room, or sensor from the roster below."
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
        }
    }
}
