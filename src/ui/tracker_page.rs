//! Operator whitelist of devices expected to share live GPS (car / hike / horseback).

use dioxus::prelude::*;

use crate::db::unix_now;
use crate::signup::Enrollment;

use super::icons::{Icon, IconName};
use super::state;
use super::widgets::{
    heard_icon_class, heard_within_week, Button, ButtonKind, PageHeading, TextInput,
};

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

#[component]
pub fn TrackerPage() -> Element {
    let mut tick = use_signal(|| 0u64);
    let mut query = use_signal(String::new);
    let mut notes_draft = use_signal(String::new);
    let mut editing = use_signal(|| None::<String>);
    let mut note = use_signal(String::new);

    let _ = tick();
    let tracked = state().db.list_tracked_nodes().unwrap_or_default();
    let tracked_keys: std::collections::HashSet<_> =
        tracked.iter().map(|t| t.pubkey.clone()).collect();
    let nodes = state().db.list_nodes().unwrap_or_default();
    let needle = query();

    let mut candidates: Vec<_> = nodes
        .iter()
        .filter(|n| n.enrollment != Enrollment::Hq && !tracked_keys.contains(&n.pubkey))
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

    rsx! {
        div { class: "flex flex-col gap-4",
            PageHeading {
                icon: IconName::Track,
                title: "Tracker".to_string(),
                "Whitelist devices you expect to move (vehicle, hike, horseback). Pins and trails update only when that radio shares location in adverts — adding them here does not turn GPS on for them."
            }
            if !note().is_empty() {
                p { class: "text-xs text-amber-200", "{note}" }
            }

            section { class: "rounded-lg border border-border bg-card/40 p-4",
                h3 { class: "mb-1 text-sm font-semibold", "On Tracker ({tracked.len()})" }
                p { class: "mb-3 text-xs text-muted-foreground",
                    "Map → Tracked shows these nodes plus HQ. Use Map → Tracker to select devices and plot trail lines with bend waypoints."
                }
                if tracked.is_empty() {
                    p { class: "text-sm text-muted-foreground",
                        "Nothing tracked yet. Search the roster below and add a device."
                    }
                } else {
                    div { class: "overflow-hidden rounded-md border border-border",
                        table { class: "w-full text-left text-sm",
                            thead { class: "bg-card text-xs uppercase tracking-wide text-muted-foreground",
                                tr {
                                    th { class: "px-3 py-2 font-medium", "Device" }
                                    th { class: "px-3 py-2 font-medium", "Last position" }
                                    th { class: "px-3 py-2 font-medium", "Notes" }
                                    th { class: "px-3 py-2 font-medium", "" }
                                }
                            }
                            tbody {
                                for (row_i, row) in tracked.into_iter().enumerate() {
                                    {
                                        let node = nodes.iter().find(|n| n.pubkey == row.pubkey);
                                        let title = node
                                            .map(|n| node_title(&n.name, &n.requested_name))
                                            .unwrap_or_else(|| {
                                                if row.label.is_empty() {
                                                    row.pubkey.chars().take(12).collect()
                                                } else {
                                                    row.label.clone()
                                                }
                                            });
                                        let prefix = node
                                            .map(|n| n.prefix.clone())
                                            .unwrap_or_else(|| {
                                                row.pubkey.chars().take(12).collect()
                                            });
                                        let has_gps = node.map(|n| n.has_gps).unwrap_or(false);
                                        let last = node.and_then(|n| n.last_advert.or(n.last_heard));
                                        let last_heard = node.and_then(|n| n.last_heard);
                                        let last_label = relative_ago(last);
                                        let added_label = relative_ago(Some(row.added_at));
                                        let notes_display = if row.notes.is_empty() {
                                            "—".to_string()
                                        } else {
                                            row.notes.clone()
                                        };
                                        let is_editing = editing().as_deref() == Some(row.pubkey.as_str());
                                        let pubkey = row.pubkey.clone();
                                        let notes_for_edit = row.notes.clone();
                                        let row_class = if row_i % 2 == 1 {
                                            "row-zebra border-t border-border"
                                        } else {
                                            "border-t border-border"
                                        };
                                        let locate_class = heard_icon_class(last_heard);
                                        rsx! {
                                            tr { class: "{row_class}",
                                                td { class: "px-3 py-2",
                                                    div { class: "font-medium", "{title}" }
                                                    div { class: "font-mono text-xs text-muted-foreground", "{prefix}" }
                                                    div { class: "text-xs text-muted-foreground", "Added {added_label}" }
                                                }
                                                td { class: "px-3 py-2 text-muted-foreground",
                                                    span {
                                                        class: "inline-flex items-center gap-1.5 {locate_class}",
                                                        title: if heard_within_week(last_heard) {
                                                            "Heard within the last week"
                                                        } else {
                                                            "Not heard in over a week"
                                                        },
                                                        Icon { name: IconName::Locate, size: 14 }
                                                        if has_gps {
                                                            "{last_label}"
                                                        } else {
                                                            "no fix yet"
                                                        }
                                                    }
                                                }
                                                td { class: "px-3 py-2",
                                                    if is_editing {
                                                        div { class: "flex flex-col gap-2",
                                                            TextInput {
                                                                value: notes_draft(),
                                                                placeholder: "Optional note…".to_string(),
                                                                kind: "text".to_string(),
                                                                oninput: move |event: FormEvent| notes_draft.set(event.value()),
                                                            }
                                                            div { class: "flex gap-2",
                                                                Button {
                                                                    kind: ButtonKind::Primary,
                                                                    onclick: {
                                                                        let key = pubkey.clone();
                                                                        move |_| {
                                                                            match state().db.update_tracked_notes(&key, &notes_draft()) {
                                                                                Ok(()) => {
                                                                                    note.set(String::new());
                                                                                    editing.set(None);
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
                                                                    onclick: move |_| editing.set(None),
                                                                    "Cancel"
                                                                }
                                                            }
                                                        }
                                                    } else {
                                                        span { class: "text-muted-foreground", "{notes_display}" }
                                                    }
                                                }
                                                td { class: "px-3 py-2",
                                                    div { class: "flex flex-wrap justify-end gap-2",
                                                        if !is_editing {
                                                            Button {
                                                                kind: ButtonKind::Ghost,
                                                                onclick: {
                                                                    let key = pubkey.clone();
                                                                    move |_| {
                                                                        notes_draft.set(notes_for_edit.clone());
                                                                        editing.set(Some(key.clone()));
                                                                    }
                                                                },
                                                                "Notes"
                                                            }
                                                        }
                                                        Button {
                                                            kind: ButtonKind::Ghost,
                                                            onclick: {
                                                                let key = pubkey.clone();
                                                                move |_| {
                                                                    let _ = state().db.remove_tracked_node(&key);
                                                                    if editing().as_deref() == Some(key.as_str()) {
                                                                        editing.set(None);
                                                                    }
                                                                    note.set(String::new());
                                                                    tick.with_mut(|n| *n += 1);
                                                                }
                                                            },
                                                            Icon { name: IconName::Close, size: 14 }
                                                            "Remove"
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

            section { class: "rounded-lg border border-border bg-card/40 p-4",
                h3 { class: "mb-1 text-sm font-semibold", "Add from roster" }
                p { class: "mb-3 text-xs text-muted-foreground",
                    "Only add radios that already share location. HQ never forces GPS share."
                }
                div { class: "mb-3 max-w-md",
                    TextInput {
                        value: query(),
                        placeholder: "Search name, prefix, pubkey…".to_string(),
                        kind: "text".to_string(),
                        oninput: move |event: FormEvent| query.set(event.value()),
                    }
                }
                if candidates.is_empty() {
                    p { class: "text-sm text-muted-foreground",
                        if needle.trim().is_empty() {
                            "Every roster device is already tracked, or none heard yet."
                        } else {
                            "No matches."
                        }
                    }
                } else {
                    div { class: "flex flex-col gap-2",
                        for (row_i, (_score, node, title)) in candidates.into_iter().enumerate() {
                            {
                                let gps_line = if node.has_gps {
                                    format!(
                                        "Has coordinates · last advert {}",
                                        relative_ago(node.last_advert)
                                    )
                                } else {
                                    "No coordinates yet (will trail once they share location)".into()
                                };
                                let key = node.pubkey.clone();
                                let label = title.clone();
                                let prefix = node.prefix.clone();
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
                                            p { class: "text-xs text-muted-foreground", "{gps_line}" }
                                        }
                                        Button {
                                            kind: ButtonKind::Primary,
                                            onclick: move |_| {
                                                match state().db.add_tracked_node(&key, &label, "") {
                                                    Ok(()) => {
                                                        note.set(format!("Tracking {label}."));
                                                        tick.with_mut(|n| *n += 1);
                                                    }
                                                    Err(err) => note.set(err.to_string()),
                                                }
                                            },
                                            Icon { name: IconName::Track, size: 14 }
                                            "Track"
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
