//! Broadcast page — contact/channel groups, canned presets, queued fan-out.

use dioxus::prelude::*;

use super::fuzzy_score;
use super::icons::{Icon, IconName};
use super::state;
use super::widgets::{
    clamp_mesh_text, Badge, Button, ButtonKind, Card, ConversationAvatar, MeshTextCount,
    PageHeading, SelectInput, SlideDownPanel, TextInput,
};
use crate::db::{is_broadcast_pubkey, BroadcastGroupKind};
use crate::radio::{prefix_channel_sender, RadioStatus};
use crate::toast::{self, ToastKind};

#[derive(Clone)]
struct BroadcastRosterEntry {
    key: String,
    label: String,
    /// Display name for avatar letter/emoji (contacts only).
    avatar_label: Option<String>,
    /// Stable seed for avatar color (pubkey for contacts).
    avatar_seed: Option<String>,
    search_fields: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BroadcastTab {
    Send,
    ContactGroups,
    ChannelGroups,
    Canned,
}

const TEXTAREA_CLASS: &str =
    "min-h-[5.5rem] w-full rounded-md border border-border bg-card px-3 py-2 text-sm text-foreground outline-none";

fn broadcast_note(message: impl Into<String>) {
    let message = message.into();
    let kind = toast::infer_kind(&message);
    toast::push(kind, message);
}

#[component]
pub fn BroadcastPage(radio: RadioStatus) -> Element {
    let mut tab = use_signal(|| BroadcastTab::Send);
    let tick = use_signal(|| 0u32);
    {
        let mut tick = tick;
        use_future(move || async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                tick.set(tick() + 1);
            }
        });
    }
    let _ = tick();
    let linked = radio.state == "connected";

    rsx! {
        div { class: "mx-auto flex max-w-4xl flex-col gap-4",
            PageHeading {
                icon: IconName::Broadcast,
                title: "Broadcast".to_string(),
                "Send one message to a saved contact or channel group. HQ queues each recipient one-by-one."
            }

            div { class: "settings-tabs", role: "tablist", aria_label: "Broadcast sections",
                button {
                    r#type: "button",
                    role: "tab",
                    class: if tab() == BroadcastTab::Send {
                        "settings-tab settings-tab-active"
                    } else {
                        "settings-tab"
                    },
                    aria_selected: if tab() == BroadcastTab::Send { "true" } else { "false" },
                    onclick: move |_| tab.set(BroadcastTab::Send),
                    Icon { name: IconName::Broadcast, size: 14 }
                    "Send"
                }
                button {
                    r#type: "button",
                    role: "tab",
                    class: if tab() == BroadcastTab::ContactGroups {
                        "settings-tab settings-tab-active"
                    } else {
                        "settings-tab"
                    },
                    aria_selected: if tab() == BroadcastTab::ContactGroups { "true" } else { "false" },
                    onclick: move |_| tab.set(BroadcastTab::ContactGroups),
                    Icon { name: IconName::Users, size: 14 }
                    "Contact groups"
                }
                button {
                    r#type: "button",
                    role: "tab",
                    class: if tab() == BroadcastTab::ChannelGroups {
                        "settings-tab settings-tab-active"
                    } else {
                        "settings-tab"
                    },
                    aria_selected: if tab() == BroadcastTab::ChannelGroups { "true" } else { "false" },
                    onclick: move |_| tab.set(BroadcastTab::ChannelGroups),
                    Icon { name: IconName::Messages, size: 14 }
                    "Channel groups"
                }
                button {
                    r#type: "button",
                    role: "tab",
                    class: if tab() == BroadcastTab::Canned {
                        "settings-tab settings-tab-active"
                    } else {
                        "settings-tab"
                    },
                    aria_selected: if tab() == BroadcastTab::Canned { "true" } else { "false" },
                    onclick: move |_| tab.set(BroadcastTab::Canned),
                    Icon { name: IconName::Quote, size: 14 }
                    "Canned messages"
                }
            }

            match tab() {
                BroadcastTab::Send => rsx! {
                    BroadcastSendPanel { linked, tick: tick() }
                },
                BroadcastTab::ContactGroups => rsx! {
                    BroadcastGroupsPanel { kind: BroadcastGroupKind::Contact }
                },
                BroadcastTab::ChannelGroups => rsx! {
                    BroadcastGroupsPanel { kind: BroadcastGroupKind::Channel }
                },
                BroadcastTab::Canned => rsx! {
                    BroadcastCannedPanel {}
                },
            }
        }
    }
}

const HISTORY_PAGE_SIZE: i64 = 8;

fn broadcast_status_meta(status: &str) -> (&'static str, &'static str, &'static str) {
    match status {
        "running" => ("bg-sky-500/15 text-sky-200", "Sending", "bg-sky-400"),
        "queued" => ("bg-sky-500/15 text-sky-200", "Queued", "bg-sky-400"),
        "done" => ("bg-emerald-500/15 text-emerald-200", "Done", "bg-emerald-400"),
        "failed" => ("bg-red-500/15 text-red-200", "Errors", "bg-red-400"),
        "cancelled" => ("bg-amber-500/15 text-amber-200", "Cancelled", "bg-amber-400"),
        _ => ("bg-muted text-muted-foreground", "Unknown", "bg-muted-foreground"),
    }
}

fn broadcast_group_label(kind_label: &str, name: &str, group_id: i64) -> String {
    if name.trim().is_empty() {
        format!("{kind_label} · Group {group_id}")
    } else {
        format!("{kind_label} · {name}")
    }
}

#[component]
fn BroadcastSendPanel(linked: bool, tick: u32) -> Element {
    let _ = tick;
    let mut group_id = use_signal(|| 0i64);
    let mut canned_id = use_signal(|| 0i64);
    let mut draft = use_signal(String::new);
    let mut history_page = use_signal(|| 0i64);
    let groups = state()
        .db
        .list_broadcast_groups(None)
        .unwrap_or_default();
    let canned = state().db.list_broadcast_canned().unwrap_or_default();
    let progress = state().db.active_broadcast_job().unwrap_or(None);
    let active = progress.is_some();
    let requested_page = history_page().max(0);
    let (mut history, history_total) = state()
        .db
        .list_broadcast_jobs(HISTORY_PAGE_SIZE, requested_page * HISTORY_PAGE_SIZE)
        .unwrap_or_default();
    let page_count = ((history_total + HISTORY_PAGE_SIZE - 1) / HISTORY_PAGE_SIZE).max(1);
    let page = requested_page.min(page_count - 1).max(0);
    if page != requested_page && history_total > 0 {
        history = state()
            .db
            .list_broadcast_jobs(HISTORY_PAGE_SIZE, page * HISTORY_PAGE_SIZE)
            .map(|(rows, _)| rows)
            .unwrap_or_default();
    }
    let advert = state()
        .db
        .hq_advert_name()
        .unwrap_or_else(|_| "hq".into());
    let selected_kind = groups
        .iter()
        .find(|g| g.id == group_id())
        .map(|g| g.kind);
    let wire_len = match selected_kind {
        Some(BroadcastGroupKind::Channel) => prefix_channel_sender(&advert, &draft()).len(),
        _ => draft().len(),
    };

    rsx! {
        div { class: "flex flex-col gap-4",
            Card { title: "Send broadcast".to_string(), icon: Some(IconName::Broadcast),
                if !linked {
                    p { class: "mb-3 flex items-center gap-2 text-sm text-amber-200/90",
                        Icon { name: IconName::Radio, size: 14 }
                        "Connect a primary companion before sending a broadcast."
                    }
                }
                div { class: "flex flex-col gap-3",
                    div {
                        label { class: "mb-1 flex items-center gap-1.5 text-xs text-muted-foreground",
                            Icon { name: IconName::Users, size: 12 }
                            "Group"
                        }
                        SelectInput {
                            id: "broadcast-group".to_string(),
                            value: group_id().to_string(),
                            onchange: move |event: FormEvent| {
                                group_id.set(event.value().parse::<i64>().unwrap_or(0));
                            },
                            option {
                                value: "0",
                                style: "background-color:#101418;color:#f4f4f5;",
                                "Choose a group…"
                            }
                            for g in groups.iter() {
                                option {
                                    value: "{g.id}",
                                    style: "background-color:#101418;color:#f4f4f5;",
                                    "{g.kind.label()} · {g.name} ({g.member_count})"
                                }
                            }
                        }
                    }
                    div {
                        label { class: "mb-1 flex items-center gap-1.5 text-xs text-muted-foreground",
                            Icon { name: IconName::Quote, size: 12 }
                            "Canned message"
                        }
                        SelectInput {
                            id: "broadcast-canned".to_string(),
                            value: canned_id().to_string(),
                            onchange: move |event: FormEvent| {
                                let v = event.value().parse::<i64>().unwrap_or(0);
                                canned_id.set(v);
                                if v > 0 {
                                    if let Ok(rows) = state().db.list_broadcast_canned() {
                                        if let Some(row) = rows.into_iter().find(|c| c.id == v) {
                                            draft.set(row.body);
                                        }
                                    }
                                }
                            },
                            option {
                                value: "0",
                                style: "background-color:#101418;color:#f4f4f5;",
                                "None — type below"
                            }
                            for row in canned.iter() {
                                option {
                                    value: "{row.id}",
                                    style: "background-color:#101418;color:#f4f4f5;",
                                    "{row.title}"
                                }
                            }
                        }
                    }
                    div {
                        label { class: "mb-1 flex items-center gap-1.5 text-xs text-muted-foreground",
                            Icon { name: IconName::Messages, size: 12 }
                            "Message"
                        }
                        textarea {
                            class: "{TEXTAREA_CLASS}",
                            rows: "4",
                            value: "{draft()}",
                            placeholder: "Message to send to every member…",
                            oninput: move |event: FormEvent| {
                                draft.set(event.value());
                                canned_id.set(0);
                            },
                        }
                        div { class: "mt-1 flex flex-wrap items-center justify-between gap-2",
                            MeshTextCount {
                                text: if selected_kind == Some(BroadcastGroupKind::Channel) {
                                    prefix_channel_sender(&advert, &draft())
                                } else {
                                    draft()
                                },
                            }
                            span { class: "text-xs text-muted-foreground",
                                if selected_kind == Some(BroadcastGroupKind::Channel) {
                                    "Channel sends include the HQ name prefix ({wire_len}/160)."
                                } else {
                                    "Direct messages use the body as typed ({wire_len}/160)."
                                }
                            }
                        }
                    }
                    div { class: "flex flex-wrap items-center gap-2",
                        Button {
                            kind: ButtonKind::Primary,
                            disabled: !linked || active || group_id() <= 0 || draft().trim().is_empty(),
                            onclick: move |_| {
                                let body = clamp_mesh_text(draft().trim());
                                match state().db.enqueue_broadcast(group_id(), &body) {
                                    Ok(job) => {
                                        history_page.set(0);
                                        broadcast_note(format!("Broadcast job {job} queued."));
                                    }
                                    Err(err) => broadcast_note(err.to_string()),
                                }
                            },
                            Icon { name: IconName::Broadcast, size: 14 }
                            "Send broadcast"
                        }
                        if let Some(p) = progress.clone() {
                            Button {
                                kind: ButtonKind::Ghost,
                                onclick: move |_| {
                                    match state().db.cancel_broadcast_job(p.id) {
                                        Ok(()) => broadcast_note("Broadcast cancelled."),
                                        Err(err) => broadcast_note(err.to_string()),
                                    }
                                },
                                "Cancel"
                            }
                        }
                    }
                    if let Some(p) = progress {
                        {
                            let done = (p.sent + p.failed + p.skipped).max(0);
                            let total = p.total.max(1);
                            let pct = ((done as f64 / total as f64) * 100.0).clamp(0.0, 100.0);
                            let (status_tone, status_label, bar_class) =
                                broadcast_status_meta(&p.status);
                            let group_label = broadcast_group_label(
                                p.group_kind.label(),
                                &p.group_name,
                                p.group_id,
                            );
                            rsx! {
                                div { class: "rounded-md bg-background/60 px-3 py-3",
                                    div { class: "mb-2 flex flex-wrap items-center justify-between gap-2",
                                        div { class: "flex min-w-0 flex-wrap items-center gap-2",
                                            span { class: "inline-flex items-center gap-1.5 text-sky-300",
                                                Icon { name: IconName::Broadcast, size: 14 }
                                                span { class: "truncate text-sm font-medium text-foreground",
                                                    "Now sending · {group_label}"
                                                }
                                            }
                                            span { class: "inline-flex items-center rounded-md px-2 py-0.5 text-xs font-medium {status_tone}",
                                                "{status_label}"
                                            }
                                        }
                                        span { class: "text-xs tabular-nums text-muted-foreground",
                                            "{done}/{total}"
                                        }
                                    }
                                    div { class: "mb-2 h-1.5 w-full overflow-hidden rounded-full bg-muted/60",
                                        div {
                                            class: "h-full rounded-full transition-all {bar_class}",
                                            style: "width: {pct:.0}%;",
                                        }
                                    }
                                    div { class: "mb-2 flex flex-wrap gap-1.5 text-xs",
                                        span { class: "rounded-md bg-emerald-500/10 px-2 py-0.5 text-emerald-200",
                                            "{p.sent} sent"
                                        }
                                        if p.failed > 0 {
                                            span { class: "rounded-md bg-red-500/10 px-2 py-0.5 text-red-200",
                                                "{p.failed} failed"
                                            }
                                        }
                                        if p.skipped > 0 {
                                            span { class: "rounded-md bg-amber-500/10 px-2 py-0.5 text-amber-200",
                                                "{p.skipped} skipped"
                                            }
                                        }
                                        if p.queued > 0 {
                                            span { class: "rounded-md bg-sky-500/10 px-2 py-0.5 text-sky-200",
                                                "{p.queued} left"
                                            }
                                        }
                                    }
                                    p { class: "truncate text-xs text-muted-foreground", "{p.body}" }
                                }
                            }
                        }
                    }
                }
            }

            Card { title: "Send history".to_string(), icon: Some(IconName::Clock),
                if history.is_empty() {
                    div { class: "rounded-lg border border-dashed border-border px-4 py-8 text-center",
                        span { class: "mx-auto mb-2 inline-flex text-muted-foreground",
                            Icon { name: IconName::Clock, size: 28 }
                        }
                        p { class: "text-sm text-muted-foreground",
                            "No broadcasts yet. Sent jobs will appear here."
                        }
                    }
                } else {
                    div { class: "flex flex-col overflow-hidden rounded-md",
                        for (idx, job) in history.iter().enumerate() {
                            {
                                let (status_tone, status_label, _) =
                                    broadcast_status_meta(&job.status);
                                let group_label = broadcast_group_label(
                                    job.group_kind.label(),
                                    &job.group_name,
                                    job.group_id,
                                );
                                let when = crate::timefmt::format_stamp(
                                    &state().db,
                                    job.created_at,
                                );
                                let done = job.sent + job.failed + job.skipped;
                                let zebra = if idx % 2 == 1 { " row-zebra" } else { "" };
                                rsx! {
                                    div { class: "flex flex-col gap-1.5 px-3 py-2.5{zebra}",
                                        div { class: "flex flex-wrap items-center justify-between gap-2",
                                            div { class: "flex min-w-0 flex-wrap items-center gap-2",
                                                span { class: "inline-flex items-center rounded-md px-2 py-0.5 text-xs font-medium {status_tone}",
                                                    "{status_label}"
                                                }
                                                span { class: "truncate text-sm font-medium",
                                                    "{group_label}"
                                                }
                                            }
                                            span { class: "shrink-0 text-xs tabular-nums text-muted-foreground",
                                                "{when}"
                                            }
                                        }
                                        p { class: "truncate text-sm text-muted-foreground", "{job.body}" }
                                        div { class: "flex flex-wrap gap-1.5 text-xs text-muted-foreground",
                                            span { class: "tabular-nums", "{done}/{job.total}" }
                                            span { "·" }
                                            span { "{job.sent} sent" }
                                            if job.failed > 0 {
                                                span { "· {job.failed} failed" }
                                            }
                                            if job.skipped > 0 {
                                                span { "· {job.skipped} skipped" }
                                            }
                                            if job.queued > 0 {
                                                span { "· {job.queued} left" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if history_total > HISTORY_PAGE_SIZE {
                        div { class: "mt-3 flex flex-wrap items-center justify-between gap-2",
                            span { class: "text-xs text-muted-foreground",
                                "Page {page + 1} of {page_count} · {history_total} jobs"
                            }
                            div { class: "flex items-center gap-2",
                                Button {
                                    kind: ButtonKind::Ghost,
                                    disabled: page <= 0,
                                    onclick: move |_| {
                                        history_page.set((history_page() - 1).max(0));
                                    },
                                    "Previous"
                                }
                                Button {
                                    kind: ButtonKind::Ghost,
                                    disabled: page + 1 >= page_count,
                                    onclick: move |_| {
                                        history_page.set(history_page() + 1);
                                    },
                                    "Next"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn clear_group_editor(
    mut editor_open: Signal<bool>,
    mut editing_id: Signal<Option<i64>>,
    mut name_draft: Signal<String>,
    mut member_keys: Signal<Vec<String>>,
    mut member_query: Signal<String>,
) {
    editor_open.set(false);
    editing_id.set(None);
    name_draft.set(String::new());
    member_keys.set(Vec::new());
    member_query.set(String::new());
}

#[component]
fn BroadcastGroupsPanel(kind: BroadcastGroupKind) -> Element {
    let mut editor_open = use_signal(|| false);
    let mut editing_id = use_signal(|| Option::<i64>::None);
    let mut name_draft = use_signal(String::new);
    let mut member_keys = use_signal(Vec::<String>::new);
    let mut member_query = use_signal(String::new);
    let groups = state()
        .db
        .list_broadcast_groups(Some(kind))
        .unwrap_or_default();
    let card_title = match kind {
        BroadcastGroupKind::Contact => "Contact groups",
        BroadcastGroupKind::Channel => "Channel groups",
    };
    let card_icon = match kind {
        BroadcastGroupKind::Contact => IconName::Users,
        BroadcastGroupKind::Channel => IconName::Messages,
    };
    let icon_tone = match kind {
        BroadcastGroupKind::Contact => "text-sky-300",
        BroadcastGroupKind::Channel => "text-emerald-300",
    };
    let sheet_title = if editing_id().is_some() {
        format!("Edit {} group", kind.label().to_ascii_lowercase())
    } else {
        format!("New {} group", kind.label().to_ascii_lowercase())
    };
    let sheet_subtitle = match kind {
        BroadcastGroupKind::Contact => {
            "Name the group, then check Contacts you added from a contact card (full public key)."
        }
        BroadcastGroupKind::Channel => {
            "Name the group, then check the active channels that should receive the blast."
        }
    };
    let search_placeholder = match kind {
        BroadcastGroupKind::Contact => "Fuzzy search name or pubkey…",
        BroadcastGroupKind::Channel => "Fuzzy search channel name…",
    };

    let roster: Vec<BroadcastRosterEntry> = match kind {
        // DM fan-out uses manually added Contacts (contact cards), not the heard roster.
        BroadcastGroupKind::Contact => state()
            .db
            .list_pinned_contacts()
            .unwrap_or_default()
            .into_iter()
            .filter(|c| is_broadcast_pubkey(&c.pubkey))
            .map(|c| {
                let key = c.pubkey.to_ascii_lowercase();
                let short: String = c.pubkey.chars().take(8).collect();
                let avatar_label = if c.label.trim().is_empty() {
                    short.clone()
                } else {
                    c.label.clone()
                };
                let label = if c.label.trim().is_empty() {
                    c.pubkey.chars().take(12).collect::<String>()
                } else {
                    format!("{} ({short})", c.label)
                };
                BroadcastRosterEntry {
                    key: key.clone(),
                    label,
                    avatar_label: Some(avatar_label),
                    avatar_seed: Some(key.clone()),
                    search_fields: vec![c.label, key, short],
                }
            })
            .collect(),
        BroadcastGroupKind::Channel => state()
            .db
            .list_channels()
            .unwrap_or_default()
            .into_iter()
            .filter(|c| c.active)
            .map(|c| {
                let slot = c.slot.to_string();
                BroadcastRosterEntry {
                    key: c.name.clone(),
                    label: format!("{} · slot {}", c.name, c.slot),
                    avatar_label: None,
                    avatar_seed: None,
                    search_fields: vec![c.name, slot],
                }
            })
            .collect(),
    };

    let query = member_query();
    let filtered: Vec<BroadcastRosterEntry> = {
        let q = query.trim();
        if q.is_empty() {
            roster
        } else {
            let mut ranked: Vec<(i32, BroadcastRosterEntry)> = roster
                .into_iter()
                .filter_map(|entry| {
                    let fields: Vec<&str> =
                        entry.search_fields.iter().map(String::as_str).collect();
                    fuzzy_score(q, &fields).map(|score| (score, entry))
                })
                .collect();
            ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.label.cmp(&b.1.label)));
            ranked.into_iter().map(|(_, entry)| entry).collect()
        }
    };

    rsx! {
        Card { title: card_title.to_string(), icon: Some(card_icon),
            div { class: "mb-3 flex flex-wrap items-center justify-between gap-2",
                p { class: "text-sm text-muted-foreground",
                    if kind == BroadcastGroupKind::Contact {
                        "DM fan-out lists built from Contacts you added (contact cards)."
                    } else {
                        "Saved channel lists for multi-channel fan-out."
                    }
                }
                Button {
                    kind: ButtonKind::Primary,
                    onclick: move |_| {
                        editing_id.set(None);
                        name_draft.set(String::new());
                        member_keys.set(Vec::new());
                        member_query.set(String::new());
                        editor_open.set(true);
                    },
                    Icon { name: IconName::Plus, size: 14 }
                    "New group"
                }
            }
            if groups.is_empty() {
                div { class: "rounded-lg border border-dashed border-border px-4 py-8 text-center",
                    span { class: "mx-auto mb-2 inline-flex {icon_tone}",
                        Icon { name: card_icon, size: 28 }
                    }
                    p { class: "text-sm text-muted-foreground",
                        "No groups yet. Create one to start broadcasting."
                    }
                }
            } else {
                div { class: "flex flex-col overflow-hidden rounded-md",
                    for (idx, g) in groups.iter().enumerate() {
                        {
                            let id = g.id;
                            let g_name = g.name.clone();
                            let count = g.member_count;
                            let members_label = if count == 1 {
                                format!("{count} member")
                            } else {
                                format!("{count} members")
                            };
                            let zebra = if idx % 2 == 1 { " row-zebra" } else { "" };
                            rsx! {
                                div {
                                    class: "flex items-center justify-between gap-3 px-3 py-2.5{zebra}",
                                    div { class: "flex min-w-0 items-center gap-3",
                                        span {
                                            class: "inline-flex h-8 w-8 shrink-0 items-center justify-center rounded-md {icon_tone}",
                                            Icon { name: card_icon, size: 16 }
                                        }
                                        div { class: "min-w-0",
                                            p { class: "truncate font-medium", "{g_name}" }
                                            p { class: "text-xs text-muted-foreground", "{members_label}" }
                                        }
                                    }
                                    div { class: "flex shrink-0 flex-wrap items-center gap-1",
                                        Button {
                                            kind: ButtonKind::Ghost,
                                            onclick: move |_| {
                                                editing_id.set(Some(id));
                                                if let Some(row) = state()
                                                    .db
                                                    .list_broadcast_groups(Some(kind))
                                                    .ok()
                                                    .into_iter()
                                                    .flatten()
                                                    .find(|r| r.id == id)
                                                {
                                                    name_draft.set(row.name);
                                                }
                                                member_keys.set(
                                                    state()
                                                        .db
                                                        .list_broadcast_group_members(id)
                                                        .unwrap_or_default(),
                                                );
                                                member_query.set(String::new());
                                                editor_open.set(true);
                                            },
                                            Icon { name: IconName::Pencil, size: 14 }
                                            "Edit"
                                        }
                                        Button {
                                            kind: ButtonKind::Ghost,
                                            onclick: move |_| {
                                                match state().db.delete_broadcast_group(id) {
                                                    Ok(()) => {
                                                        if editing_id() == Some(id) {
                                                            clear_group_editor(
                                                                editor_open,
                                                                editing_id,
                                                                name_draft,
                                                                member_keys,
                                                                member_query,
                                                            );
                                                        }
                                                        toast::push(
                                                            ToastKind::Success,
                                                            "Group deleted.",
                                                        );
                                                    }
                                                    Err(err) => broadcast_note(err.to_string()),
                                                }
                                            },
                                            Icon { name: IconName::Trash, size: 14 }
                                            "Delete"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        SlideDownPanel {
            open: editor_open(),
            title: sheet_title,
            subtitle: sheet_subtitle.to_string(),
            icon: Some(card_icon),
            on_close: move |_| {
                clear_group_editor(
                    editor_open,
                    editing_id,
                    name_draft,
                    member_keys,
                    member_query,
                );
            },
            footer: rsx! {
                if editing_id().is_some() {
                    Button {
                        kind: ButtonKind::Ghost,
                        onclick: move |_| {
                            if let Some(id) = editing_id() {
                                match state().db.delete_broadcast_group(id) {
                                    Ok(()) => {
                                        clear_group_editor(
                                            editor_open,
                                            editing_id,
                                            name_draft,
                                            member_keys,
                                            member_query,
                                        );
                                        toast::push(ToastKind::Success, "Group deleted.");
                                    }
                                    Err(err) => broadcast_note(err.to_string()),
                                }
                            }
                        },
                        Icon { name: IconName::Trash, size: 14 }
                        "Delete"
                    }
                }
                Button {
                    kind: ButtonKind::Ghost,
                    onclick: move |_| {
                        clear_group_editor(
                            editor_open,
                            editing_id,
                            name_draft,
                            member_keys,
                            member_query,
                        );
                    },
                    "Cancel"
                }
                Button {
                    kind: ButtonKind::Primary,
                    onclick: move |_| {
                        let name = name_draft().trim().to_string();
                        if name.is_empty() {
                            toast::push(ToastKind::Warn, "Group name is required.");
                            return;
                        }
                        let result = if let Some(id) = editing_id() {
                            state()
                                .db
                                .rename_broadcast_group(id, &name)
                                .and_then(|_| {
                                    state().db.set_broadcast_group_members(
                                        id,
                                        kind,
                                        &member_keys(),
                                    )
                                })
                                .map(|_| id)
                        } else {
                            state().db.create_broadcast_group(kind, &name).and_then(|id| {
                                state().db.set_broadcast_group_members(
                                    id,
                                    kind,
                                    &member_keys(),
                                )?;
                                Ok(id)
                            })
                        };
                        match result {
                            Ok(_) => {
                                clear_group_editor(
                                    editor_open,
                                    editing_id,
                                    name_draft,
                                    member_keys,
                                    member_query,
                                );
                                toast::push(ToastKind::Success, "Group saved.");
                            }
                            Err(err) => broadcast_note(err.to_string()),
                        }
                    },
                    Icon { name: IconName::Check, size: 14 }
                    "Save group"
                }
            },
            div { class: "flex flex-col gap-3",
                div {
                    label { class: "mb-1 flex items-center gap-1.5 text-xs text-muted-foreground",
                        Icon { name: IconName::Pencil, size: 12 }
                        "Name"
                    }
                    TextInput {
                        value: name_draft(),
                        placeholder: "Group name",
                        oninput: move |event: FormEvent| name_draft.set(event.value()),
                    }
                }
                div {
                    label { class: "mb-1 flex items-center gap-1.5 text-xs text-muted-foreground",
                        Icon { name: card_icon, size: 12 }
                        if kind == BroadcastGroupKind::Contact {
                            "Members (added Contacts)"
                        } else {
                            "Members (active channels)"
                        }
                    }
                    TextInput {
                        value: member_query(),
                        placeholder: "{search_placeholder}",
                        oninput: move |event: FormEvent| member_query.set(event.value()),
                    }
                    if filtered.is_empty() {
                        p { class: "mt-2 text-sm text-muted-foreground",
                            if query.trim().is_empty() {
                                if kind == BroadcastGroupKind::Contact {
                                    "No Contacts with a public key yet. Add one from a contact card in Messages."
                                } else {
                                    "No active channels."
                                }
                            } else {
                                "No matches for that search."
                            }
                        }
                    } else {
                        div { class: "mt-2 flex max-h-72 flex-col gap-2 overflow-y-auto pr-1",
                            for entry in filtered.iter() {
                                {
                                    let key = entry.key.clone();
                                    let label = entry.label.clone();
                                    let avatar_label = entry.avatar_label.clone();
                                    let avatar_seed = entry.avatar_seed.clone();
                                    let checked = member_keys()
                                        .iter()
                                        .any(|k| k.eq_ignore_ascii_case(&key));
                                    rsx! {
                                        label {
                                            class: if checked {
                                                "flex cursor-pointer items-center gap-2 rounded-lg border border-emerald-500/40 bg-emerald-500/10 px-3 py-2 text-sm"
                                            } else {
                                                "flex cursor-pointer items-center gap-2 rounded-lg border border-border bg-background px-3 py-2 text-sm"
                                            },
                                            input {
                                                r#type: "checkbox",
                                                checked: checked,
                                                onchange: move |_| {
                                                    let mut next = member_keys();
                                                    if let Some(pos) = next
                                                        .iter()
                                                        .position(|k| k.eq_ignore_ascii_case(&key))
                                                    {
                                                        next.remove(pos);
                                                    } else {
                                                        next.push(key.clone());
                                                    }
                                                    member_keys.set(next);
                                                },
                                            }
                                            if let (Some(av_label), Some(av_seed)) =
                                                (avatar_label, avatar_seed)
                                            {
                                                ConversationAvatar {
                                                    label: av_label,
                                                    seed: av_seed,
                                                    compact: true,
                                                }
                                            }
                                            span { class: "min-w-0 flex-1 truncate", "{label}" }
                                            if checked {
                                                span { class: "ml-auto shrink-0 text-emerald-300",
                                                    Icon { name: IconName::Check, size: 14 }
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
}

#[component]
fn BroadcastCannedPanel() -> Element {
    let mut editor_open = use_signal(|| false);
    let mut edit_id = use_signal(|| Option::<i64>::None);
    let mut title = use_signal(String::new);
    let mut body = use_signal(String::new);
    let rows = state().db.list_broadcast_canned().unwrap_or_default();
    let sheet_title = if edit_id().is_some() {
        "Edit preset".to_string()
    } else {
        "New preset".to_string()
    };

    rsx! {
        Card { title: "Canned messages".to_string(), icon: Some(IconName::Quote),
            div { class: "mb-3 flex flex-wrap items-center justify-between gap-2",
                p { class: "text-sm text-muted-foreground",
                    "Reusable message bodies for the Send tab."
                }
                Button {
                    kind: ButtonKind::Primary,
                    onclick: move |_| {
                        edit_id.set(None);
                        title.set(String::new());
                        body.set(String::new());
                        editor_open.set(true);
                    },
                    Icon { name: IconName::Plus, size: 14 }
                    "New preset"
                }
            }
            if rows.is_empty() {
                div { class: "rounded-lg border border-dashed border-border px-4 py-8 text-center",
                    span { class: "mx-auto mb-2 inline-flex text-violet-300",
                        Icon { name: IconName::Quote, size: 28 }
                    }
                    p { class: "text-sm text-muted-foreground",
                        "No presets yet. Create one to fill the Send draft quickly."
                    }
                }
            } else {
                div { class: "flex flex-col overflow-hidden rounded-md",
                    for (idx, row) in rows.iter().enumerate() {
                        {
                            let id = row.id;
                            let row_title = row.title.clone();
                            let row_body = row.body.clone();
                            let zebra = if idx % 2 == 1 { " row-zebra" } else { "" };
                            rsx! {
                                div { class: "flex items-center justify-between gap-3 px-3 py-2.5{zebra}",
                                    div { class: "flex min-w-0 flex-1 items-start gap-3",
                                        span { class: "mt-0.5 inline-flex h-8 w-8 shrink-0 items-center justify-center rounded-md text-violet-300",
                                            Icon { name: IconName::Quote, size: 14 }
                                        }
                                        div { class: "min-w-0 flex-1",
                                            div { class: "mb-0.5 flex flex-wrap items-center gap-2",
                                                p { class: "truncate font-medium", "{row.title}" }
                                                if row.is_default {
                                                    Badge { tone: "ok".to_string(), "default" }
                                                }
                                            }
                                            p { class: "truncate text-sm text-muted-foreground", "{row.body}" }
                                        }
                                    }
                                    Button {
                                        kind: ButtonKind::Ghost,
                                        onclick: move |_| {
                                            edit_id.set(Some(id));
                                            title.set(row_title.clone());
                                            body.set(row_body.clone());
                                            editor_open.set(true);
                                        },
                                        Icon { name: IconName::Pencil, size: 14 }
                                        "Edit"
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        SlideDownPanel {
            open: editor_open(),
            title: sheet_title,
            subtitle: "Title appears in the Send canned picker; body is what goes on the mesh.".to_string(),
            icon: Some(IconName::Quote),
            on_close: move |_| {
                editor_open.set(false);
                edit_id.set(None);
                title.set(String::new());
                body.set(String::new());
            },
            footer: rsx! {
                if edit_id().is_some() {
                    Button {
                        kind: ButtonKind::Ghost,
                        onclick: move |_| {
                            if let Some(id) = edit_id() {
                                match state().db.delete_broadcast_canned(id) {
                                    Ok(()) => {
                                        editor_open.set(false);
                                        edit_id.set(None);
                                        title.set(String::new());
                                        body.set(String::new());
                                        toast::push(ToastKind::Success, "Canned message deleted.");
                                    }
                                    Err(err) => broadcast_note(err.to_string()),
                                }
                            }
                        },
                        Icon { name: IconName::Trash, size: 14 }
                        "Delete"
                    }
                }
                Button {
                    kind: ButtonKind::Ghost,
                    onclick: move |_| {
                        editor_open.set(false);
                        edit_id.set(None);
                        title.set(String::new());
                        body.set(String::new());
                    },
                    "Cancel"
                }
                Button {
                    kind: ButtonKind::Primary,
                    onclick: move |_| {
                        let t = title().trim().to_string();
                        let b = body().trim().to_string();
                        if t.is_empty() {
                            toast::push(ToastKind::Warn, "Preset title is required.");
                            return;
                        }
                        if b.is_empty() {
                            toast::push(ToastKind::Warn, "Preset body is required.");
                            return;
                        }
                        match state().db.upsert_broadcast_canned(edit_id(), &t, &b) {
                            Ok(_) => {
                                editor_open.set(false);
                                edit_id.set(None);
                                title.set(String::new());
                                body.set(String::new());
                                toast::push(ToastKind::Success, "Canned message saved.");
                            }
                            Err(err) => broadcast_note(err.to_string()),
                        }
                    },
                    Icon { name: IconName::Check, size: 14 }
                    "Save preset"
                }
            },
            div { class: "flex flex-col gap-3",
                div {
                    label { class: "mb-1 flex items-center gap-1.5 text-xs text-muted-foreground",
                        Icon { name: IconName::Pencil, size: 12 }
                        "Title"
                    }
                    TextInput {
                        value: title(),
                        placeholder: "Preset title",
                        oninput: move |event: FormEvent| title.set(event.value()),
                    }
                }
                div {
                    label { class: "mb-1 flex items-center gap-1.5 text-xs text-muted-foreground",
                        Icon { name: IconName::Messages, size: 12 }
                        "Body"
                    }
                    textarea {
                        class: "{TEXTAREA_CLASS}",
                        rows: "4",
                        value: "{body()}",
                        placeholder: "Preset message body…",
                        oninput: move |event: FormEvent| body.set(event.value()),
                    }
                    div { class: "mt-1",
                        MeshTextCount { text: body() }
                    }
                }
            }
        }
    }
}
