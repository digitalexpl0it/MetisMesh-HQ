//! Messages left rail: Channels / DMs / Contacts tabs.

use dioxus::prelude::*;

use crate::db::{unix_now, ChannelRow, DmThreadRow, MessagesRailSection, NodeRow};
use crate::timefmt;

use super::icons::{Icon, IconName};
use super::state;
use super::widgets::{
    Button, ButtonKind, ConversationAvatar, Dialog, EmojiPickerButton, SlideDownPanel, TextInput,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum RailTab {
    Channels,
    Dms,
    Contacts,
}

#[derive(Clone)]
enum RailDialog {
    NewSection {
        tab: String,
        /// After create, move this `(item_kind, item_key)` into the new section.
        move_item: Option<(String, String)>,
    },
    SetSectionIcon {
        id: i64,
        current: String,
    },
    AddContact,
}

/// Slide-down details for a rail row (channel name, or contact/DM pubkey / `name:` key).
#[derive(Clone, PartialEq, Eq)]
enum RailDetails {
    Channel(String),
    Contact(String),
}

#[derive(Clone, Copy, PartialEq)]
enum RailItemMenuAnchor {
    /// Fixed viewport position (right-click or settings gear).
    Context { x: f64, y: f64 },
}

fn section_icon(sections: &[MessagesRailSection], kind: &str) -> String {
    sections
        .iter()
        .find(|s| s.kind == kind)
        .map(|s| s.icon.clone())
        .unwrap_or_default()
}

fn rail_fuzzy_match(query: &str, fields: &[&str]) -> bool {
    super::fuzzy_score(query, fields).is_some()
}

/// DMs search matches thread titles (and short key prefixes only when the query
/// looks like hex). Never scans the full 64-char pubkey — that made typing `1`
/// match almost every contact/thread.
fn dm_thread_fuzzy_match(query: &str, thread: &DmThreadRow) -> bool {
    let q = query.trim();
    if q.is_empty() {
        return true;
    }
    if rail_fuzzy_match(q, &[&thread.name]) {
        return true;
    }
    let hexish = q.len() >= 4 && q.chars().all(|c| c.is_ascii_hexdigit());
    if !hexish {
        return false;
    }
    let prefix = thread.pubkey.get(..thread.pubkey.len().min(12)).unwrap_or("");
    rail_fuzzy_match(q, &[prefix, thread.pubkey.as_str()])
}

fn tab_search_placeholder(tab: RailTab) -> &'static str {
    match tab {
        RailTab::Channels => "Search channels…",
        RailTab::Dms => "Search DMs…",
        RailTab::Contacts => "Search contacts…",
    }
}

fn unread_badge_class(unread_mentions: i64) -> &'static str {
    if unread_mentions > 0 {
        "channel-unread-badge channel-unread-badge-mention"
    } else {
        "channel-unread-badge"
    }
}

fn node_label(node: &NodeRow) -> String {
    let name = node.name.trim();
    if !name.is_empty() {
        return name.to_string();
    }
    if let Some(req) = node.requested_name.as_ref() {
        let req = req.trim();
        if !req.is_empty() {
            return req.to_string();
        }
    }
    if !node.prefix.is_empty() {
        return node.prefix.clone();
    }
    node.pubkey.chars().take(12).collect()
}

fn is_chat_device(contact_type: u8) -> bool {
    !matches!(contact_type, 2 | 3 | 4)
}

fn section_collapsed(sections: &[MessagesRailSection], kind: &str) -> (Option<i64>, bool) {
    sections
        .iter()
        .find(|s| s.kind == kind)
        .map(|s| (Some(s.id), s.collapsed))
        .unwrap_or((None, false))
}

#[component]
pub fn MessagesRail(
    channels: Vec<ChannelRow>,
    dm_threads: Vec<DmThreadRow>,
    selected_channel: Option<String>,
    selected_dm: Option<String>,
    on_select_channel: EventHandler<String>,
    on_select_dm: EventHandler<String>,
    on_delete_channel: EventHandler<String>,
    on_clear_dm: EventHandler<(String, String)>,
    on_channel_appearance: EventHandler<String>,
    rail_epoch: Signal<u64>,
) -> Element {
    let mut tab = use_signal(|| RailTab::Channels);
    let mut dialog = use_signal(|| None::<RailDialog>);
    let mut details = use_signal(|| None::<RailDetails>);
    let mut details_tick = use_signal(|| 0u64);
    let mut section_name = use_signal(String::new);
    let mut section_emoji = use_signal(String::new);
    let mut contact_key = use_signal(String::new);
    let mut contact_label = use_signal(String::new);
    let mut channel_query = use_signal(String::new);
    let mut dm_query = use_signal(String::new);
    let mut contact_query = use_signal(String::new);
    let _ = rail_epoch();
    // Channel-name favorites (`name:…`) upgrade when that node shows up on the roster
    // (companion contact sync / advert) — not from channel text alone.
    let _ = state().db.promote_pending_name_pins();

    let chat_sections = state()
        .db
        .list_messages_rail_sections("chats")
        .unwrap_or_default();
    let dm_sections = state()
        .db
        .list_messages_rail_sections("dms")
        .unwrap_or_default();
    let contact_sections = state()
        .db
        .list_messages_rail_sections("contacts")
        .unwrap_or_default();
    let favorites = state()
        .db
        .list_messages_favorites()
        .unwrap_or_default();
    let fav_set: std::collections::HashSet<(String, String)> =
        favorites.iter().cloned().collect();
    let moved_chats = state()
        .db
        .messages_moved_item_keys(Some("chats"))
        .unwrap_or_default();
    let moved_dms = state()
        .db
        .messages_moved_item_keys(Some("dms"))
        .unwrap_or_default();
    let moved_contacts = state()
        .db
        .messages_moved_item_keys(Some("contacts"))
        .unwrap_or_default();
    let pinned = state().db.list_pinned_contacts().unwrap_or_default();
    let pinned_set: std::collections::HashSet<String> =
        pinned.iter().map(|p| p.pubkey.clone()).collect();
    let nodes = state().db.list_nodes().unwrap_or_default();
    let chat_nodes: Vec<_> = nodes
        .iter()
        .filter(|n| n.enrollment != crate::signup::Enrollment::Hq && is_chat_device(n.contact_type))
        .cloned()
        .collect();
    let infra_nodes: Vec<_> = nodes
        .iter()
        .filter(|n| n.enrollment != crate::signup::Enrollment::Hq && !is_chat_device(n.contact_type))
        .cloned()
        .collect();
    let chat_custom_sections: Vec<_> = chat_sections
        .iter()
        .filter(|s| s.kind == "custom")
        .cloned()
        .collect();
    let dm_custom_sections: Vec<_> = dm_sections
        .iter()
        .filter(|s| s.kind == "custom")
        .cloned()
        .collect();
    let contact_custom_sections: Vec<_> = contact_sections
        .iter()
        .filter(|s| s.kind == "custom")
        .cloned()
        .collect();

    let (fav_id, fav_collapsed) = section_collapsed(&chat_sections, "favorites");
    let (channels_id, channels_collapsed) = section_collapsed(&chat_sections, "channels");
    let (dm_fav_id, dm_fav_collapsed) = section_collapsed(&dm_sections, "favorites");
    let (dms_id, dms_collapsed) = section_collapsed(&dm_sections, "dms");
    let (pinned_id, pinned_collapsed) = section_collapsed(&contact_sections, "pinned");
    let (chat_dev_id, chat_dev_collapsed) = section_collapsed(&contact_sections, "chat_devices");
    let (infra_id, infra_collapsed) = section_collapsed(&contact_sections, "infra_nodes");
    let fav_icon = section_icon(&chat_sections, "favorites");
    let channels_icon = section_icon(&chat_sections, "channels");
    let dm_fav_icon = section_icon(&dm_sections, "favorites");
    let dms_icon = section_icon(&dm_sections, "dms");
    let pinned_icon = section_icon(&contact_sections, "pinned");
    let roster_icon = section_icon(&contact_sections, "chat_devices");
    let infra_icon = section_icon(&contact_sections, "infra_nodes");

    let mut bump = move || {
        rail_epoch.with_mut(|n| *n = n.wrapping_add(1));
    };

    let ch_q = channel_query();
    let dm_q = dm_query();
    let ct_q = contact_query();

    let fav_channels: Vec<_> = channels
        .iter()
        .filter(|c| fav_set.contains(&("channel".into(), c.name.clone())))
        .filter(|c| rail_fuzzy_match(&ch_q, &[&c.name]))
        .cloned()
        .collect();
    let fav_dms: Vec<_> = dm_threads
        .iter()
        .filter(|t| {
            fav_set.contains(&("dm".into(), t.pubkey.clone()))
                || fav_set.contains(&("dm".into(), t.pubkey.to_ascii_lowercase()))
        })
        .filter(|t| dm_thread_fuzzy_match(&dm_q, t))
        .cloned()
        .collect();
    let default_channels: Vec<_> = channels
        .iter()
        .filter(|c| !moved_chats.contains(&("channel".into(), c.name.clone())))
        .filter(|c| rail_fuzzy_match(&ch_q, &[&c.name]))
        .cloned()
        .collect();
    let default_dms: Vec<_> = dm_threads
        .iter()
        .filter(|t| {
            !moved_dms.contains(&("dm".into(), t.pubkey.clone()))
                && !moved_dms.contains(&("dm".into(), t.pubkey.to_ascii_lowercase()))
        })
        .filter(|t| dm_thread_fuzzy_match(&dm_q, t))
        .cloned()
        .collect();
    let roster_nodes: Vec<_> = chat_nodes
        .iter()
        .filter(|n| !moved_contacts.contains(&("contact".into(), n.pubkey.clone())))
        .filter(|n| {
            let label = node_label(n);
            rail_fuzzy_match(&ct_q, &[&label, &n.name, &n.pubkey, &n.prefix])
        })
        .cloned()
        .collect();
    let default_infra: Vec<_> = infra_nodes
        .iter()
        .filter(|n| !moved_contacts.contains(&("contact".into(), n.pubkey.clone())))
        .filter(|n| {
            let label = node_label(n);
            rail_fuzzy_match(&ct_q, &[&label, &n.name, &n.pubkey, &n.prefix])
        })
        .cloned()
        .collect();
    let pinned_filtered: Vec<_> = pinned
        .iter()
        .filter(|row| {
            let label = if row.label.trim().is_empty() {
                row.pubkey.chars().take(12).collect::<String>()
            } else {
                row.label.clone()
            };
            rail_fuzzy_match(&ct_q, &[&label, &row.pubkey])
        })
        .cloned()
        .collect();

    let active_query = match tab() {
        RailTab::Channels => channel_query(),
        RailTab::Dms => dm_query(),
        RailTab::Contacts => contact_query(),
    };
    let search_placeholder = tab_search_placeholder(tab());

    rsx! {
        nav { class: "messages-rail", aria_label: "Conversations",
            div { class: "messages-rail-tabs",
                button {
                    r#type: "button",
                    class: if tab() == RailTab::Channels {
                        "messages-rail-tab messages-rail-tab-active"
                    } else {
                        "messages-rail-tab"
                    },
                    title: "Channels",
                    onclick: move |_| tab.set(RailTab::Channels),
                    Icon { name: IconName::Radio, size: 13 }
                    span { "Channels" }
                }
                button {
                    r#type: "button",
                    class: if tab() == RailTab::Dms {
                        "messages-rail-tab messages-rail-tab-active"
                    } else {
                        "messages-rail-tab"
                    },
                    title: "Direct messages",
                    onclick: move |_| tab.set(RailTab::Dms),
                    Icon { name: IconName::Messages, size: 13 }
                    span { "DMs" }
                }
                button {
                    r#type: "button",
                    class: if tab() == RailTab::Contacts {
                        "messages-rail-tab messages-rail-tab-active"
                    } else {
                        "messages-rail-tab"
                    },
                    title: "Contacts",
                    onclick: move |_| tab.set(RailTab::Contacts),
                    Icon { name: IconName::Users, size: 13 }
                    span { "Contacts" }
                }
            }
            div { class: "messages-rail-search",
                Icon { name: IconName::Scan, size: 13 }
                input {
                    class: "messages-rail-search-input",
                    r#type: "search",
                    value: "{active_query}",
                    placeholder: "{search_placeholder}",
                    "aria-label": "{search_placeholder}",
                    oninput: move |e: FormEvent| {
                        let value = e.value();
                        match tab() {
                            RailTab::Channels => channel_query.set(value),
                            RailTab::Dms => dm_query.set(value),
                            RailTab::Contacts => contact_query.set(value),
                        }
                    },
                }
                if !active_query.is_empty() {
                    button {
                        r#type: "button",
                        class: "messages-rail-search-clear",
                        title: "Clear search",
                        onclick: move |_| {
                            match tab() {
                                RailTab::Channels => channel_query.set(String::new()),
                                RailTab::Dms => dm_query.set(String::new()),
                                RailTab::Contacts => contact_query.set(String::new()),
                            }
                        },
                        Icon { name: IconName::Close, size: 11 }
                    }
                }
            }

            if tab() == RailTab::Channels {
                div { class: "messages-rail-toolbar",
                    button {
                        r#type: "button",
                        class: "messages-rail-tool",
                        title: "New section",
                        onclick: move |_| {
                            section_name.set(String::new());
                            section_emoji.set(String::new());
                            dialog.set(Some(RailDialog::NewSection {
                                tab: "chats".into(),
                                move_item: None,
                            }));
                        },
                        Icon { name: IconName::Plus, size: 12 }
                        "Section"
                    }
                }

                // Favorites
                RailSectionHeader {
                    title: "Favorites".to_string(),
                    icon: fav_icon.clone(),
                    section_id: fav_id,
                    collapsed: fav_collapsed,
                    can_delete: false,
                    on_toggle: move |_| {
                        if let Some(id) = fav_id {
                            let _ = state().db.set_messages_section_collapsed(id, !fav_collapsed);
                            bump();
                        }
                    },
                    on_delete: move |_| {},
                    on_edit_icon: move |id| {
                        section_emoji.set(fav_icon.clone());
                        dialog.set(Some(RailDialog::SetSectionIcon {
                            id,
                            current: fav_icon.clone(),
                        }));
                    },
                }
                if !fav_collapsed {
                    div { class: "messages-rail-list",
                        if fav_channels.is_empty() {
                            p { class: "messages-rail-empty", "Star a channel to pin it here." }
                        }
                        for channel in fav_channels.iter() {
                            {
                                let channel = channel.clone();
                                let active = selected_channel.as_ref() == Some(&channel.name);
                                rsx! {
                                    ChannelRailItem {
                                        channel,
                                        active,
                                        favorite: true,
                                        sections: chat_custom_sections.clone(),
                                        details,
                                        on_add_section: move |(kind, key)| {
                                            section_name.set(String::new());
                                            section_emoji.set(String::new());
                                            dialog.set(Some(RailDialog::NewSection {
                                                tab: "chats".into(),
                                                move_item: Some((kind, key)),
                                            }));
                                        },
                                        on_select: move |n| on_select_channel.call(n),
                                        on_appearance: move |n| on_channel_appearance.call(n),
                                        on_delete: move |n| on_delete_channel.call(n),
                                        on_changed: move |_| bump(),
                                    }
                                }
                            }
                        }
                    }
                }

                // Custom sections
                for section in chat_custom_sections.iter() {
                    {
                        let section = section.clone();
                        let sid = section.id;
                        let collapsed = section.collapsed;
                        let title = section.name.clone();
                        let icon = section.icon.clone();
                        let members = state()
                            .db
                            .list_messages_section_members(sid)
                            .unwrap_or_default()
                            .into_iter()
                            .filter(|(ik, key)| {
                                if ik != "channel" {
                                    return false;
                                }
                                channels
                                    .iter()
                                    .find(|c| &c.name == key)
                                    .map(|c| rail_fuzzy_match(&ch_q, &[&c.name]))
                                    .unwrap_or(false)
                            })
                            .collect::<Vec<_>>();
                        if members.is_empty() && !ch_q.trim().is_empty() {
                            rsx! {}
                        } else {
                        rsx! {
                            RailSectionHeader {
                                title,
                                icon: icon.clone(),
                                section_id: Some(sid),
                                collapsed,
                                can_delete: true,
                                on_toggle: move |_| {
                                    let _ = state().db.set_messages_section_collapsed(sid, !collapsed);
                                    bump();
                                },
                                on_delete: move |_| {
                                    if let Err(err) = state().db.delete_messages_custom_section(sid) {
                                        crate::toast::push(
                                            crate::toast::ToastKind::Error,
                                            err.to_string(),
                                        );
                                    } else {
                                        bump();
                                    }
                                },
                                on_edit_icon: move |id| {
                                    section_emoji.set(icon.clone());
                                    dialog.set(Some(RailDialog::SetSectionIcon {
                                        id,
                                        current: icon.clone(),
                                    }));
                                },
                            }
                            if !collapsed {
                                div { class: "messages-rail-list",
                                    if members.is_empty() {
                                        p { class: "messages-rail-empty", "Move a channel here." }
                                    }
                                    for (ik, key) in members.iter() {
                                        if ik == "channel" {
                                            if let Some(channel) = channels.iter().find(|c| &c.name == key).cloned() {
                                                {
                                                    let fav = fav_set.contains(&("channel".into(), channel.name.clone()));
                                                    let active = selected_channel.as_ref() == Some(&channel.name);
                                                    rsx! {
                                                        ChannelRailItem {
                                                            channel,
                                                            active,
                                                            favorite: fav,
                                                            sections: chat_custom_sections.clone(),
                                                            details,
                                                            on_add_section: move |(kind, key)| {
                                                                section_name.set(String::new());
                                                                section_emoji.set(String::new());
                                                                dialog.set(Some(RailDialog::NewSection {
                                                                    tab: "chats".into(),
                                                                    move_item: Some((kind, key)),
                                                                }));
                                                            },
                                                            on_select: move |n| on_select_channel.call(n),
                                                            on_appearance: move |n| on_channel_appearance.call(n),
                                                            on_delete: move |n| on_delete_channel.call(n),
                                                            on_changed: move |_| bump(),
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

                // All channels
                RailSectionHeader {
                    title: "Channels".to_string(),
                    icon: channels_icon.clone(),
                    section_id: channels_id,
                    collapsed: channels_collapsed,
                    can_delete: false,
                    on_toggle: move |_| {
                        if let Some(id) = channels_id {
                            let _ = state().db.set_messages_section_collapsed(id, !channels_collapsed);
                            bump();
                        }
                    },
                    on_delete: move |_| {},
                    on_edit_icon: move |id| {
                        section_emoji.set(channels_icon.clone());
                        dialog.set(Some(RailDialog::SetSectionIcon {
                            id,
                            current: channels_icon.clone(),
                        }));
                    },
                }
                if !channels_collapsed {
                    div { class: "messages-rail-list",
                        if default_channels.is_empty() {
                            p { class: "messages-rail-empty", "No channels yet." }
                        }
                        for channel in default_channels.iter() {
                            {
                                let channel = channel.clone();
                                let fav = fav_set.contains(&("channel".into(), channel.name.clone()));
                                let active = selected_channel.as_ref() == Some(&channel.name);
                                rsx! {
                                    ChannelRailItem {
                                        channel,
                                        active,
                                        favorite: fav,
                                        sections: chat_custom_sections.clone(),
                                        details,
                                        on_add_section: move |(kind, key)| {
                                            section_name.set(String::new());
                                            section_emoji.set(String::new());
                                            dialog.set(Some(RailDialog::NewSection {
                                                tab: "chats".into(),
                                                move_item: Some((kind, key)),
                                            }));
                                        },
                                        on_select: move |n| on_select_channel.call(n),
                                        on_appearance: move |n| on_channel_appearance.call(n),
                                        on_delete: move |n| on_delete_channel.call(n),
                                        on_changed: move |_| bump(),
                                    }
                                }
                            }
                        }
                    }
                }
            } else if tab() == RailTab::Dms {
                div { class: "messages-rail-toolbar",
                    button {
                        r#type: "button",
                        class: "messages-rail-tool",
                        title: "New DM section",
                        onclick: move |_| {
                            section_name.set(String::new());
                            section_emoji.set(String::new());
                            dialog.set(Some(RailDialog::NewSection {
                                tab: "dms".into(),
                                move_item: None,
                            }));
                        },
                        Icon { name: IconName::Plus, size: 12 }
                        "Section"
                    }
                }
                // Favorites (DMs)
                RailSectionHeader {
                    title: "Favorites".to_string(),
                    icon: dm_fav_icon.clone(),
                    section_id: dm_fav_id,
                    collapsed: dm_fav_collapsed,
                    can_delete: false,
                    on_toggle: move |_| {
                        if let Some(id) = dm_fav_id {
                            let _ = state()
                                .db
                                .set_messages_section_collapsed(id, !dm_fav_collapsed);
                            bump();
                        }
                    },
                    on_delete: move |_| {},
                    on_edit_icon: move |id| {
                        section_emoji.set(dm_fav_icon.clone());
                        dialog.set(Some(RailDialog::SetSectionIcon {
                            id,
                            current: dm_fav_icon.clone(),
                        }));
                    },
                }
                if !dm_fav_collapsed {
                    div { class: "messages-rail-list",
                        if fav_dms.is_empty() {
                            p { class: "messages-rail-empty",
                                if dm_q.trim().is_empty() {
                                    "Star a DM to pin it here."
                                } else {
                                    "No matching favorites."
                                }
                            }
                        }
                        for thread in fav_dms.iter() {
                            {
                                let thread = thread.clone();
                                let active = selected_dm.as_ref() == Some(&thread.pubkey);
                                rsx! {
                                    DmRailItem {
                                        thread,
                                        active,
                                        favorite: true,
                                        sections: dm_custom_sections.clone(),
                                        details,
                                        on_add_section: move |(kind, key)| {
                                            section_name.set(String::new());
                                            section_emoji.set(String::new());
                                            dialog.set(Some(RailDialog::NewSection {
                                                tab: "dms".into(),
                                                move_item: Some((kind, key)),
                                            }));
                                        },
                                        on_select: move |k| on_select_dm.call(k),
                                        on_clear: move |pair| on_clear_dm.call(pair),
                                        on_changed: move |_| bump(),
                                    }
                                }
                            }
                        }
                    }
                }

                // Custom DM sections
                for section in dm_custom_sections.iter() {
                    {
                        let section = section.clone();
                        let sid = section.id;
                        let collapsed = section.collapsed;
                        let title = section.name.clone();
                        let icon = section.icon.clone();
                        let members = state()
                            .db
                            .list_messages_section_members(sid)
                            .unwrap_or_default()
                            .into_iter()
                            .filter(|(ik, key)| {
                                if ik != "dm" {
                                    return false;
                                }
                                dm_threads
                                    .iter()
                                    .find(|t| &t.pubkey == key)
                                    .map(|t| dm_thread_fuzzy_match(&dm_q, t))
                                    .unwrap_or(false)
                            })
                            .collect::<Vec<_>>();
                        if members.is_empty() && !dm_q.trim().is_empty() {
                            rsx! {}
                        } else {
                            rsx! {
                                RailSectionHeader {
                                    title,
                                    icon: icon.clone(),
                                    section_id: Some(sid),
                                    collapsed,
                                    can_delete: true,
                                    on_toggle: move |_| {
                                        let _ = state().db.set_messages_section_collapsed(sid, !collapsed);
                                        bump();
                                    },
                                    on_delete: move |_| {
                                        if let Err(err) = state().db.delete_messages_custom_section(sid) {
                                            crate::toast::push(
                                                crate::toast::ToastKind::Error,
                                                err.to_string(),
                                            );
                                        } else {
                                            bump();
                                        }
                                    },
                                    on_edit_icon: move |id| {
                                        section_emoji.set(icon.clone());
                                        dialog.set(Some(RailDialog::SetSectionIcon {
                                            id,
                                            current: icon.clone(),
                                        }));
                                    },
                                }
                                if !collapsed {
                                    div { class: "messages-rail-list",
                                        if members.is_empty() {
                                            p { class: "messages-rail-empty", "Move a DM here." }
                                        }
                                        for (_ik, key) in members.iter() {
                                            if let Some(thread) = dm_threads.iter().find(|t| &t.pubkey == key).cloned() {
                                                {
                                                    let fav = fav_set.contains(&("dm".into(), thread.pubkey.clone()));
                                                    let active = selected_dm.as_ref() == Some(&thread.pubkey);
                                                    rsx! {
                                                        DmRailItem {
                                                            thread,
                                                            active,
                                                            favorite: fav,
                                                            sections: dm_custom_sections.clone(),
                                                            details,
                                                            on_add_section: move |(kind, key)| {
                                                                section_name.set(String::new());
                                                                section_emoji.set(String::new());
                                                                dialog.set(Some(RailDialog::NewSection {
                                                                    tab: "dms".into(),
                                                                    move_item: Some((kind, key)),
                                                                }));
                                                            },
                                                            on_select: move |k| on_select_dm.call(k),
                                                            on_clear: move |pair| on_clear_dm.call(pair),
                                                            on_changed: move |_| bump(),
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

                RailSectionHeader {
                    title: "Direct messages".to_string(),
                    icon: dms_icon.clone(),
                    section_id: dms_id,
                    collapsed: dms_collapsed,
                    can_delete: false,
                    on_toggle: move |_| {
                        if let Some(id) = dms_id {
                            let _ = state().db.set_messages_section_collapsed(id, !dms_collapsed);
                            bump();
                        }
                    },
                    on_delete: move |_| {},
                    on_edit_icon: move |id| {
                        section_emoji.set(dms_icon.clone());
                        dialog.set(Some(RailDialog::SetSectionIcon {
                            id,
                            current: dms_icon.clone(),
                        }));
                    },
                }
                if !dms_collapsed {
                    div { class: "messages-rail-list",
                        if default_dms.is_empty() {
                            p { class: "messages-rail-empty",
                                if dm_q.trim().is_empty() {
                                    "No direct threads yet."
                                } else {
                                    "No matching DMs."
                                }
                            }
                        }
                        for thread in default_dms.iter() {
                            {
                                let thread = thread.clone();
                                let fav = fav_set.contains(&("dm".into(), thread.pubkey.clone()))
                                    || fav_set.contains(&(
                                        "dm".into(),
                                        thread.pubkey.to_ascii_lowercase(),
                                    ));
                                let active = selected_dm.as_ref() == Some(&thread.pubkey);
                                rsx! {
                                    DmRailItem {
                                        thread,
                                        active,
                                        favorite: fav,
                                        sections: dm_custom_sections.clone(),
                                        details,
                                        on_add_section: move |(kind, key)| {
                                            section_name.set(String::new());
                                            section_emoji.set(String::new());
                                            dialog.set(Some(RailDialog::NewSection {
                                                tab: "dms".into(),
                                                move_item: Some((kind, key)),
                                            }));
                                        },
                                        on_select: move |k| on_select_dm.call(k),
                                        on_clear: move |pair| on_clear_dm.call(pair),
                                        on_changed: move |_| bump(),
                                    }
                                }
                            }
                        }
                    }
                }
            } else {
                // Contacts: roster auto-fills; custom sections + manual/favorite add.
                div { class: "messages-rail-toolbar",
                    button {
                        r#type: "button",
                        class: "messages-rail-tool",
                        title: "New section",
                        onclick: move |_| {
                            section_name.set(String::new());
                            section_emoji.set(String::new());
                            dialog.set(Some(RailDialog::NewSection {
                                tab: "contacts".into(),
                                move_item: None,
                            }));
                        },
                        Icon { name: IconName::Plus, size: 12 }
                        "Section"
                    }
                    button {
                        r#type: "button",
                        class: "messages-rail-tool",
                        title: "Add a contact by public key",
                        onclick: move |_| {
                            contact_key.set(String::new());
                            contact_label.set(String::new());
                            dialog.set(Some(RailDialog::AddContact));
                        },
                        Icon { name: IconName::Users, size: 12 }
                        "Add"
                    }
                }
                RailSectionHeader {
                    title: "Pinned".to_string(),
                    icon: pinned_icon.clone(),
                    section_id: pinned_id,
                    collapsed: pinned_collapsed,
                    can_delete: false,
                    on_toggle: move |_| {
                        if let Some(id) = pinned_id {
                            let _ = state().db.set_messages_section_collapsed(id, !pinned_collapsed);
                            bump();
                        }
                    },
                    on_delete: move |_| {},
                    on_edit_icon: move |id| {
                        section_emoji.set(pinned_icon.clone());
                        dialog.set(Some(RailDialog::SetSectionIcon {
                            id,
                            current: pinned_icon.clone(),
                        }));
                    },
                }
                if !pinned_collapsed {
                    div { class: "messages-rail-list",
                        if pinned.is_empty() {
                            p { class: "messages-rail-empty", "Star a roster contact, Add manually, or use Add on a contact link." }
                        } else if pinned_filtered.is_empty() {
                            p { class: "messages-rail-empty", "No pinned contacts match." }
                        }
                        for row in pinned_filtered.iter() {
                            {
                                let pubkey = row.pubkey.clone();
                                let label = if row.label.trim().is_empty() {
                                    pubkey.chars().take(12).collect::<String>()
                                } else {
                                    row.label.clone()
                                };
                                let active = selected_dm.as_ref() == Some(&pubkey);
                                let name_only = pubkey.starts_with("name:");
                                rsx! {
                                    ContactNodeItem {
                                        pubkey: pubkey.clone(),
                                        label: if name_only {
                                            format!("{label} (awaiting advert)")
                                        } else {
                                            label
                                        },
                                        contact_type: 1,
                                        active,
                                        pinned: true,
                                        sections: contact_custom_sections.clone(),
                                        details,
                                        on_add_section: move |(kind, key)| {
                                            section_name.set(String::new());
                                            section_emoji.set(String::new());
                                            dialog.set(Some(RailDialog::NewSection {
                                                tab: "contacts".into(),
                                                move_item: Some((kind, key)),
                                            }));
                                        },
                                        on_select: move |k: String| {
                                            if k.starts_with("name:") {
                                                crate::toast::push(
                                                    crate::toast::ToastKind::Warn,
                                                    "Still awaiting their radio advert — channel chat has no public key. Once the companion hears them (or you Add their key), this pin upgrades automatically.",
                                                );
                                            } else {
                                                on_select_dm.call(k);
                                            }
                                        },
                                        on_changed: move |_| bump(),
                                    }
                                }
                            }
                        }
                    }
                }

                for section in contact_custom_sections.iter() {
                    {
                        let section = section.clone();
                        let sid = section.id;
                        let collapsed = section.collapsed;
                        let title = section.name.clone();
                        let icon = section.icon.clone();
                        let members = state()
                            .db
                            .list_messages_section_members(sid)
                            .unwrap_or_default()
                            .into_iter()
                            .filter(|(ik, key)| {
                                if ik != "contact" {
                                    return false;
                                }
                                if let Some(node) = nodes.iter().find(|n| &n.pubkey == key) {
                                    let label = node_label(node);
                                    return rail_fuzzy_match(
                                        &ct_q,
                                        &[&label, &node.name, &node.pubkey, &node.prefix],
                                    );
                                }
                                if let Some(row) = pinned.iter().find(|p| &p.pubkey == key) {
                                    let label = if row.label.trim().is_empty() {
                                        row.pubkey.chars().take(12).collect::<String>()
                                    } else {
                                        row.label.clone()
                                    };
                                    return rail_fuzzy_match(&ct_q, &[&label, &row.pubkey]);
                                }
                                false
                            })
                            .collect::<Vec<_>>();
                        if members.is_empty() && !ct_q.trim().is_empty() {
                            rsx! {}
                        } else {
                        rsx! {
                            RailSectionHeader {
                                title,
                                icon: icon.clone(),
                                section_id: Some(sid),
                                collapsed,
                                can_delete: true,
                                on_toggle: move |_| {
                                    let _ = state().db.set_messages_section_collapsed(sid, !collapsed);
                                    bump();
                                },
                                on_delete: move |_| {
                                    if let Err(err) = state().db.delete_messages_custom_section(sid) {
                                        crate::toast::push(
                                            crate::toast::ToastKind::Error,
                                            err.to_string(),
                                        );
                                    } else {
                                        bump();
                                    }
                                },
                                on_edit_icon: move |id| {
                                    section_emoji.set(icon.clone());
                                    dialog.set(Some(RailDialog::SetSectionIcon {
                                        id,
                                        current: icon.clone(),
                                    }));
                                },
                            }
                            if !collapsed {
                                div { class: "messages-rail-list",
                                    if members.is_empty() {
                                        p { class: "messages-rail-empty", "Move a contact here." }
                                    }
                                    for (_ik, key) in members.iter() {
                                        if let Some(node) = nodes.iter().find(|n| &n.pubkey == key).cloned() {
                                            {
                                                let pinned_on = pinned_set.contains(&node.pubkey);
                                                let active = selected_dm.as_ref() == Some(&node.pubkey);
                                                rsx! {
                                                    ContactNodeItem {
                                                        pubkey: node.pubkey.clone(),
                                                        label: node_label(&node),
                                                        contact_type: node.contact_type,
                                                        active,
                                                        pinned: pinned_on,
                                                        sections: contact_custom_sections.clone(),
                                                        details,
                                                        on_add_section: move |(kind, key)| {
                                                            section_name.set(String::new());
                                                            section_emoji.set(String::new());
                                                            dialog.set(Some(RailDialog::NewSection {
                                                                tab: "contacts".into(),
                                                                move_item: Some((kind, key)),
                                                            }));
                                                        },
                                                        on_select: move |k| on_select_dm.call(k),
                                                        on_changed: move |_| bump(),
                                                    }
                                                }
                                            }
                                        } else if let Some(row) = pinned.iter().find(|p| &p.pubkey == key).cloned() {
                                            {
                                                let label = if row.label.trim().is_empty() {
                                                    row.pubkey.chars().take(12).collect::<String>()
                                                } else {
                                                    row.label.clone()
                                                };
                                                let active = selected_dm.as_ref() == Some(&row.pubkey);
                                                rsx! {
                                                    ContactNodeItem {
                                                        pubkey: row.pubkey.clone(),
                                                        label,
                                                        contact_type: 1,
                                                        active,
                                                        pinned: true,
                                                        sections: contact_custom_sections.clone(),
                                                        details,
                                                        on_add_section: move |(kind, key)| {
                                                            section_name.set(String::new());
                                                            section_emoji.set(String::new());
                                                            dialog.set(Some(RailDialog::NewSection {
                                                                tab: "contacts".into(),
                                                                move_item: Some((kind, key)),
                                                            }));
                                                        },
                                                        on_select: move |k| on_select_dm.call(k),
                                                        on_changed: move |_| bump(),
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

                RailSectionHeader {
                    title: "Roster".to_string(),
                    icon: roster_icon.clone(),
                    section_id: chat_dev_id,
                    collapsed: chat_dev_collapsed,
                    can_delete: false,
                    on_toggle: move |_| {
                        if let Some(id) = chat_dev_id {
                            let _ = state().db.set_messages_section_collapsed(id, !chat_dev_collapsed);
                            bump();
                        }
                    },
                    on_delete: move |_| {},
                    on_edit_icon: move |id| {
                        section_emoji.set(roster_icon.clone());
                        dialog.set(Some(RailDialog::SetSectionIcon {
                            id,
                            current: roster_icon.clone(),
                        }));
                    },
                }
                if !chat_dev_collapsed {
                    div { class: "messages-rail-list",
                        if roster_nodes.is_empty() {
                            p { class: "messages-rail-empty", "Roster chat devices appear here automatically." }
                        }
                        for node in roster_nodes.iter() {
                            {
                                let active = selected_dm.as_ref() == Some(&node.pubkey);
                                let pinned_on = pinned_set.contains(&node.pubkey);
                                rsx! {
                                    ContactNodeItem {
                                        pubkey: node.pubkey.clone(),
                                        label: node_label(node),
                                        contact_type: node.contact_type,
                                        active,
                                        pinned: pinned_on,
                                        sections: contact_custom_sections.clone(),
                                        details,
                                        on_add_section: move |(kind, key)| {
                                            section_name.set(String::new());
                                            section_emoji.set(String::new());
                                            dialog.set(Some(RailDialog::NewSection {
                                                tab: "contacts".into(),
                                                move_item: Some((kind, key)),
                                            }));
                                        },
                                        on_select: move |k| on_select_dm.call(k),
                                        on_changed: move |_| bump(),
                                    }
                                }
                            }
                        }
                    }
                }
                RailSectionHeader {
                    title: "Repeaters & rooms".to_string(),
                    icon: infra_icon.clone(),
                    section_id: infra_id,
                    collapsed: infra_collapsed,
                    can_delete: false,
                    on_toggle: move |_| {
                        if let Some(id) = infra_id {
                            let _ = state().db.set_messages_section_collapsed(id, !infra_collapsed);
                            bump();
                        }
                    },
                    on_delete: move |_| {},
                    on_edit_icon: move |id| {
                        section_emoji.set(infra_icon.clone());
                        dialog.set(Some(RailDialog::SetSectionIcon {
                            id,
                            current: infra_icon.clone(),
                        }));
                    },
                }
                if !infra_collapsed {
                    div { class: "messages-rail-list",
                        if default_infra.is_empty() {
                            p { class: "messages-rail-empty", "No repeaters or rooms heard yet." }
                        }
                        for node in default_infra.iter() {
                            {
                                let pinned_on = pinned_set.contains(&node.pubkey);
                                let active = selected_dm.as_ref() == Some(&node.pubkey);
                                rsx! {
                                    ContactNodeItem {
                                        pubkey: node.pubkey.clone(),
                                        label: node_label(node),
                                        contact_type: node.contact_type,
                                        active,
                                        pinned: pinned_on,
                                        sections: contact_custom_sections.clone(),
                                        details,
                                        on_add_section: move |(kind, key)| {
                                            section_name.set(String::new());
                                            section_emoji.set(String::new());
                                            dialog.set(Some(RailDialog::NewSection {
                                                tab: "contacts".into(),
                                                move_item: Some((kind, key)),
                                            }));
                                        },
                                        on_select: move |k| on_select_dm.call(k),
                                        on_changed: move |_| bump(),
                                    }
                                }
                            }
                        }
                    }
                }
            }

            Dialog {
                open: dialog().is_some(),
                title: match dialog() {
                    Some(RailDialog::NewSection { move_item: Some(_), .. }) => {
                        "New section & move".into()
                    }
                    Some(RailDialog::NewSection { .. }) => "New section".into(),
                    Some(RailDialog::SetSectionIcon { .. }) => "Section icon".into(),
                    Some(RailDialog::AddContact) => "Add contact".into(),
                    None => String::new(),
                },
                on_close: move |_| dialog.set(None),
                div { class: "flex flex-col gap-3",
                    match dialog() {
                        Some(RailDialog::NewSection {
                            tab: section_tab,
                            move_item,
                        }) => {
                            let will_move = move_item.is_some();
                            let pending_move = move_item.clone();
                            rsx! {
                                p { class: "text-sm text-muted-foreground",
                                    if will_move {
                                        "Name the section. The selected item moves into it after create."
                                    } else if section_tab == "contacts" {
                                        "Create a custom Contacts section, then move roster contacts into it."
                                    } else if section_tab == "dms" {
                                        "Create a custom DMs section, then move direct threads into it."
                                    } else {
                                        "Create a custom Channels section, then move channels into it."
                                    }
                                }
                                TextInput {
                                    value: section_name(),
                                    placeholder: "Section name".to_string(),
                                    oninput: move |e: FormEvent| section_name.set(e.value()),
                                }
                                div { class: "section-emoji-row",
                                    span { class: "text-sm text-muted-foreground", "Icon" }
                                    if !section_emoji().is_empty() {
                                        span {
                                            class: "section-emoji-preview",
                                            title: "Selected section icon",
                                            "{section_emoji}"
                                        }
                                        button {
                                            r#type: "button",
                                            class: "messages-rail-item-action",
                                            title: "Clear icon",
                                            onclick: move |_| section_emoji.set(String::new()),
                                            Icon { name: IconName::Close, size: 12 }
                                        }
                                    }
                                    EmojiPickerButton {
                                        drop_down: true,
                                        on_pick: move |emoji: String| section_emoji.set(emoji),
                                    }
                                }
                                div { class: "flex justify-end gap-2",
                                    Button {
                                        kind: ButtonKind::Ghost,
                                        onclick: move |_| dialog.set(None),
                                        "Cancel"
                                    }
                                    Button {
                                        onclick: move |_| {
                                            match state().db.create_messages_custom_section(
                                                &section_tab,
                                                &section_name(),
                                                &section_emoji(),
                                            ) {
                                                Ok(id) => {
                                                    if let Some((kind, key)) = pending_move.clone() {
                                                        if let Err(err) = state()
                                                            .db
                                                            .set_messages_item_section(
                                                                &kind,
                                                                &key,
                                                                Some(id),
                                                            )
                                                        {
                                                            crate::toast::push(
                                                                crate::toast::ToastKind::Error,
                                                                err.to_string(),
                                                            );
                                                        }
                                                    }
                                                    dialog.set(None);
                                                    bump();
                                                }
                                                Err(err) => crate::toast::push(
                                                    crate::toast::ToastKind::Error,
                                                    err.to_string(),
                                                ),
                                            }
                                        },
                                        if will_move { "Create & move" } else { "Create" }
                                    }
                                }
                            }
                        },
                        Some(RailDialog::SetSectionIcon { id, current }) => rsx! {
                            p { class: "text-sm text-muted-foreground",
                                "Pick an emoji for this section, or clear it to show no icon."
                            }
                            div { class: "section-emoji-row",
                                span { class: "text-sm text-muted-foreground", "Icon" }
                                if !section_emoji().is_empty() {
                                    span {
                                        class: "section-emoji-preview",
                                        title: "Selected section icon",
                                        "{section_emoji}"
                                    }
                                } else if !current.is_empty() {
                                    span {
                                        class: "section-emoji-preview section-emoji-preview-muted",
                                        title: "Current icon",
                                        "{current}"
                                    }
                                }
                                EmojiPickerButton {
                                    drop_down: true,
                                    on_pick: move |emoji: String| section_emoji.set(emoji),
                                }
                            }
                            div { class: "flex justify-end gap-2",
                                Button {
                                    kind: ButtonKind::Ghost,
                                    onclick: move |_| {
                                        let _ = state().db.set_messages_section_icon(id, "");
                                        dialog.set(None);
                                        bump();
                                    },
                                    "Clear"
                                }
                                Button {
                                    kind: ButtonKind::Ghost,
                                    onclick: move |_| dialog.set(None),
                                    "Cancel"
                                }
                                Button {
                                    onclick: move |_| {
                                        match state().db.set_messages_section_icon(id, &section_emoji()) {
                                            Ok(()) => {
                                                dialog.set(None);
                                                bump();
                                            }
                                            Err(err) => crate::toast::push(
                                                crate::toast::ToastKind::Error,
                                                err.to_string(),
                                            ),
                                        }
                                    },
                                    "Save"
                                }
                            }
                        },
                        Some(RailDialog::AddContact) => rsx! {
                            p { class: "text-sm text-muted-foreground",
                                "Paste a 64-character public key. Optional label for the Contacts list."
                            }
                            TextInput {
                                value: contact_key(),
                                placeholder: "Public key (hex)".to_string(),
                                oninput: move |e: FormEvent| contact_key.set(e.value()),
                            }
                            TextInput {
                                value: contact_label(),
                                placeholder: "Label (optional)".to_string(),
                                oninput: move |e: FormEvent| contact_label.set(e.value()),
                            }
                            div { class: "flex justify-end gap-2",
                                Button {
                                    kind: ButtonKind::Ghost,
                                    onclick: move |_| dialog.set(None),
                                    "Cancel"
                                }
                                Button {
                                    onclick: move |_| {
                                        match state().db.pin_contact(&contact_key(), &contact_label()) {
                                            Ok(()) => {
                                                let _ = state().db.set_messages_favorite(
                                                    "dm",
                                                    &contact_key().trim().to_ascii_lowercase(),
                                                    true,
                                                );
                                                dialog.set(None);
                                                bump();
                                            }
                                            Err(err) => crate::toast::push(
                                                crate::toast::ToastKind::Error,
                                                err.to_string(),
                                            ),
                                        }
                                    },
                                    "Add contact"
                                }
                            }
                        },
                        None => rsx! {},
                    }
                }
            }
        }
        RailDetailsSlide {
            target: details(),
            tick: details_tick(),
            on_close: move |_| details.set(None),
            on_refreshed: move |_| {
                details_tick.with_mut(|n| *n = n.wrapping_add(1));
                bump();
            },
        }
    }
}

fn rail_format_time(seconds: Option<i64>) -> String {
    let Some(seconds) = seconds else {
        return "—".into();
    };
    timefmt::format_stamp(&state().db, seconds)
}

fn contact_type_label(contact_type: u8) -> &'static str {
    match contact_type {
        2 => "Repeater",
        3 => "Room server",
        4 => "Sensor",
        1 => "Chat",
        _ => "Unknown",
    }
}

#[component]
fn RailDetailField(label: String, children: Element) -> Element {
    rsx! {
        div { class: "min-w-0",
            p { class: "mb-0.5 text-[0.65rem] uppercase tracking-[0.14em] text-muted-foreground", "{label}" }
            div { class: "text-sm text-foreground", {children} }
        }
    }
}

#[component]
fn RailDetailsSlide(
    target: Option<RailDetails>,
    tick: u64,
    on_close: EventHandler<()>,
    on_refreshed: EventHandler<()>,
) -> Element {
    let _ = tick;
    let open = target.is_some();
    let (title, subtitle, icon) = match target.as_ref() {
        Some(RailDetails::Channel(name)) => (
            name.clone(),
            "Channel details stored on this HQ.".to_string(),
            IconName::Radio,
        ),
        Some(RailDetails::Contact(key)) if key.starts_with("name:") => (
            key.trim_start_matches("name:").to_string(),
            "Name-only pin — waiting for a radio advert.".to_string(),
            IconName::Users,
        ),
        Some(RailDetails::Contact(_)) => (
            "Contact".into(),
            "Everything HQ has stored for this radio.".to_string(),
            IconName::Users,
        ),
        None => (String::new(), String::new(), IconName::Help),
    };

    let channel = match target.as_ref() {
        Some(RailDetails::Channel(name)) => state()
            .db
            .list_channels()
            .ok()
            .and_then(|rows| rows.into_iter().find(|c| &c.name == name)),
        _ => None,
    };
    let contact_key = match target.as_ref() {
        Some(RailDetails::Contact(key)) => Some(key.clone()),
        _ => None,
    };
    let node = contact_key
        .as_ref()
        .filter(|k| !k.starts_with("name:"))
        .and_then(|k| state().db.get_node(k).ok().flatten());
    let title = if let Some(ref n) = node {
        let label = node_label(n);
        if label.is_empty() {
            title
        } else {
            label
        }
    } else {
        title
    };
    let path = contact_key
        .as_ref()
        .and_then(|k| state().db.latest_path_for_pubkey(k).ok().flatten());
    let status = contact_key
        .as_ref()
        .and_then(|k| state().db.get_infra_status_latest(k).ok().flatten());
    let rf = contact_key
        .as_ref()
        .and_then(|k| state().db.rf_samples_for_pubkey(k, 6).ok())
        .unwrap_or_default();
    let messages = contact_key
        .as_ref()
        .filter(|k| !k.starts_with("name:"))
        .and_then(|k| state().db.messages_for_pubkey(k, 6).ok())
        .unwrap_or_default();
    let dm_thread = contact_key.as_ref().and_then(|k| {
        state()
            .db
            .list_dm_threads()
            .ok()
            .and_then(|rows| rows.into_iter().find(|t| &t.pubkey == k))
    });
    let raw_pretty = contact_key
        .as_ref()
        .and_then(|k| state().db.node_raw_json(k).ok().flatten())
        .and_then(|raw| {
            serde_json::from_str::<serde_json::Value>(&raw)
                .ok()
                .and_then(|v| serde_json::to_string_pretty(&v).ok())
        });
    let refresh_key = contact_key
        .clone()
        .filter(|k| !k.starts_with("name:") && k.len() == 64);
    let name_only = contact_key
        .as_ref()
        .is_some_and(|k| k.starts_with("name:"));

    rsx! {
        SlideDownPanel {
            open,
            title: title.clone(),
            subtitle: subtitle.clone(),
            icon: Some(icon),
            on_close: move |_| on_close.call(()),
            if let Some(ch) = channel.as_ref() {
                div { class: "flex flex-col gap-4 text-sm",
                    div { class: "grid gap-3 sm:grid-cols-2",
                        RailDetailField { label: "Slot".to_string(), "{ch.slot}" }
                        RailDetailField { label: "Visibility".to_string(), "{ch.visibility}" }
                        RailDetailField { label: "Role".to_string(), "{ch.role}" }
                        RailDetailField { label: "Active".to_string(),
                            if ch.active { "Yes" } else { "No" }
                        }
                        RailDetailField { label: "Messages".to_string(), "{ch.message_count}" }
                        RailDetailField { label: "Unread".to_string(), "{ch.unread_count}" }
                        RailDetailField { label: "Last message".to_string(),
                            "{rail_format_time(ch.last_message_at)}"
                        }
                        RailDetailField { label: "Keep".to_string(),
                            { ch.message_keep.clone().unwrap_or_else(|| "default".into()) }
                        }
                        RailDetailField { label: "Region".to_string(),
                            { ch.region.clone().unwrap_or_else(|| "—".into()) }
                        }
                        RailDetailField { label: "Appearance".to_string(),
                            if ch.has_appearance_override { "Custom" } else { "Default" }
                        }
                    }
                    if let Some(preview) = ch.last_text.as_ref() {
                        div { class: "rounded-md border border-border bg-background/60 p-3",
                            p { class: "mb-2 text-xs uppercase tracking-[0.16em] text-muted-foreground",
                                "Last text"
                            }
                            p { class: "text-xs break-words text-muted-foreground", "{preview}" }
                        }
                    }
                }
            } else if name_only {
                div { class: "flex flex-col gap-3 text-sm",
                    p { class: "text-muted-foreground",
                        "This pin is name-only (from channel chat). HQ has no public key yet, so status, path, and telemetry cannot be requested. It upgrades automatically when the companion hears their advert, or when you Add their key."
                    }
                    RailDetailField { label: "Bookmark".to_string(),
                        span { class: "break-all font-mono text-xs",
                            { contact_key.clone().unwrap_or_default() }
                        }
                    }
                }
            } else if let Some(node) = node.as_ref() {
                div { class: "flex flex-col gap-4 text-sm",
                    div { class: "flex flex-wrap gap-2",
                        if let Some(key) = refresh_key.clone() {
                            Button {
                                kind: ButtonKind::Ghost,
                                onclick: move |_| {
                                    if state().radio.snapshot().state != "connected" {
                                        crate::toast::push(
                                            crate::toast::ToastKind::Warn,
                                            "Connect a companion before refreshing.",
                                        );
                                        return;
                                    }
                                    crate::toast::push(
                                        crate::toast::ToastKind::Info,
                                        "Refreshing contact…",
                                    );
                                    let rx = state().radio.refresh_contact(key.clone());
                                    spawn(async move {
                                        match rx.await.unwrap_or_else(|_| Err("Radio task ended.".into())) {
                                            Ok(refresh) => {
                                                crate::toast::push(
                                                    crate::toast::ToastKind::Success,
                                                    refresh.summary,
                                                );
                                                on_refreshed.call(());
                                            }
                                            Err(err) => crate::toast::push(
                                                crate::toast::ToastKind::Error,
                                                err,
                                            ),
                                        }
                                    });
                                },
                                Icon { name: IconName::Signal, size: 14 }
                                "Refresh contact"
                            }
                        }
                    }
                    div { class: "grid gap-3 sm:grid-cols-2",
                        RailDetailField { label: "Type".to_string(),
                            "{contact_type_label(node.contact_type)}"
                        }
                        RailDetailField { label: "Enrollment".to_string(),
                            "{node.enrollment.label()}"
                        }
                        RailDetailField { label: "Prefix".to_string(),
                            span { class: "font-mono text-xs", "{node.prefix}" }
                        }
                        RailDetailField { label: "Last heard".to_string(),
                            "{rail_format_time(node.last_heard)}"
                        }
                        RailDetailField { label: "Last advert".to_string(),
                            "{rail_format_time(node.last_advert)}"
                        }
                        RailDetailField { label: "Path length".to_string(),
                            if node.path_len >= 0 { "{node.path_len}" } else { "—" }
                        }
                    }
                    div { class: "rounded-md border border-border bg-background/60 p-3",
                        p { class: "mb-2 text-xs uppercase tracking-[0.16em] text-muted-foreground",
                            "Identity"
                        }
                        p { class: "break-all font-mono text-xs leading-relaxed", "{node.pubkey}" }
                        if let Some(req) = node.requested_name.as_ref() {
                            if !req.is_empty() {
                                p { class: "mt-2 text-xs text-muted-foreground",
                                    "Requested name: {req}"
                                }
                            }
                        }
                    }
                    div { class: "rounded-md border border-border bg-background/60 p-3",
                        p { class: "mb-2 text-xs uppercase tracking-[0.16em] text-muted-foreground",
                            "Location"
                        }
                        if let (Some(lat), Some(lon)) = (node.lat, node.lon) {
                            p { class: "font-mono text-xs", "{lat:.6}, {lon:.6}" }
                        } else {
                            p { class: "text-muted-foreground", "No GPS fix stored (or 0,0 ignored)." }
                        }
                    }
                    div { class: "rounded-md border border-border bg-background/60 p-3",
                        p { class: "mb-2 text-xs uppercase tracking-[0.16em] text-muted-foreground",
                            "Path"
                        }
                        if let Some(p) = path.as_ref() {
                            p { class: "text-xs",
                                "Source {p.source} · hash size {p.hash_size} · {rail_format_time(Some(p.created_at))}"
                            }
                            if !p.path.is_empty() {
                                p { class: "mt-1 break-all font-mono text-[0.7rem] text-muted-foreground",
                                    "{hex::encode(&p.path)}"
                                }
                            }
                        } else if !node.hop_bytes.is_empty() {
                            p { class: "break-all font-mono text-[0.7rem] text-muted-foreground",
                                "{hex::encode(&node.hop_bytes)}"
                            }
                        } else {
                            p { class: "text-muted-foreground", "No path stored yet." }
                        }
                    }
                    div { class: "rounded-md border border-border bg-background/60 p-3",
                        p { class: "mb-2 text-xs uppercase tracking-[0.16em] text-muted-foreground",
                            "Last status probe"
                        }
                        if let Some(s) = status.as_ref() {
                            ul { class: "space-y-1 text-xs text-muted-foreground",
                                li { "Probed {rail_format_time(Some(s.probed_at))}" }
                                li { class: "tabular-nums text-foreground",
                                    "Battery {s.battery_mv} mV · noise {s.noise_floor} · RSSI {s.last_rssi} · SNR {s.snr:.1}"
                                }
                                li { class: "tabular-nums text-foreground",
                                    "Uptime {s.uptime_s}s · RX {s.nb_recv} · TX {s.nb_sent}"
                                }
                            }
                        } else {
                            p { class: "text-muted-foreground",
                                "No status sample yet. Use Refresh contact (uses airtime)."
                            }
                        }
                    }
                    div { class: "rounded-md border border-border bg-background/60 p-3",
                        p { class: "mb-2 text-xs uppercase tracking-[0.16em] text-muted-foreground",
                            "RF samples"
                        }
                        if rf.is_empty() {
                            p { class: "text-muted-foreground", "No noise / RSSI / SNR samples yet." }
                        } else {
                            ul { class: "space-y-1 text-xs",
                                for (noise, rssi, snr, at) in rf.iter() {
                                    li { class: "flex flex-wrap gap-x-3 text-muted-foreground",
                                        span { "{rail_format_time(Some(*at))}" }
                                        span { class: "tabular-nums text-foreground", "noise {noise}" }
                                        span { class: "tabular-nums text-foreground", "rssi {rssi}" }
                                        span { class: "tabular-nums text-foreground", "snr {snr:.1}" }
                                    }
                                }
                            }
                        }
                    }
                    if let Some(thread) = dm_thread.as_ref() {
                        div { class: "rounded-md border border-border bg-background/60 p-3",
                            p { class: "mb-2 text-xs uppercase tracking-[0.16em] text-muted-foreground",
                                "Direct messages"
                            }
                            p { class: "text-xs text-muted-foreground",
                                "{thread.message_count} message(s) · unread {thread.unread_count} · last {rail_format_time(thread.last_message_at)}"
                            }
                        }
                    }
                    div { class: "rounded-md border border-border bg-background/60 p-3",
                        p { class: "mb-2 text-xs uppercase tracking-[0.16em] text-muted-foreground",
                            "Recent messages"
                        }
                        if messages.is_empty() {
                            p { class: "text-muted-foreground", "No messages attributed to this pubkey yet." }
                        } else {
                            ul { class: "space-y-2",
                                for msg in messages.iter() {
                                    li { class: "rounded border border-border/70 bg-card/40 px-2.5 py-2",
                                        div { class: "flex flex-wrap gap-2 text-[0.7rem] text-muted-foreground",
                                            span { "{rail_format_time(Some(msg.received_at))}" }
                                            span { class: "uppercase", "{msg.direction}" }
                                            if let Some(hops) = msg.hop_count {
                                                span { "{hops} hop(s)" }
                                            }
                                        }
                                        p { class: "mt-1 text-xs break-words",
                                            {
                                                let t = msg.text.clone();
                                                if t.chars().count() > 160 {
                                                    format!("{}…", t.chars().take(160).collect::<String>())
                                                } else {
                                                    t
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if let Some(raw) = raw_pretty.as_ref() {
                        details { class: "rounded-md border border-border bg-background/60 p-3",
                            summary { class: "cursor-pointer text-xs uppercase tracking-[0.16em] text-muted-foreground",
                                "Raw contact snapshot (JSON)"
                            }
                            pre { class: "mt-2 max-h-48 overflow-auto whitespace-pre-wrap break-all font-mono text-[0.7rem] text-muted-foreground",
                                "{raw}"
                            }
                        }
                    }
                }
            } else if contact_key.is_some() {
                p { class: "text-sm text-muted-foreground",
                    "No roster row for this key yet. Refresh after the companion hears them, or Add the public key."
                }
            }
        }
    }
}

#[component]
fn RailSectionHeader(
    title: String,
    #[props(default)] icon: String,
    #[props(default)] section_id: Option<i64>,
    collapsed: bool,
    can_delete: bool,
    on_toggle: EventHandler<()>,
    on_delete: EventHandler<()>,
    on_edit_icon: EventHandler<i64>,
) -> Element {
    rsx! {
        div { class: "messages-rail-section-head",
            button {
                r#type: "button",
                class: "messages-rail-section-title",
                onclick: move |_| on_toggle.call(()),
                span { class: "messages-rail-chevron",
                    Icon {
                        name: if collapsed { IconName::ChevronRight } else { IconName::ChevronDown },
                        size: 12,
                    }
                }
                if !icon.is_empty() {
                    span { class: "messages-rail-section-emoji", "{icon}" }
                }
                "{title}"
            }
            if let Some(id) = section_id {
                button {
                    r#type: "button",
                    class: "messages-rail-item-action",
                    title: "Set section emoji",
                    onclick: move |event: MouseEvent| {
                        event.prevent_default();
                        event.stop_propagation();
                        on_edit_icon.call(id);
                    },
                    Icon { name: IconName::Emoji, size: 11 }
                }
            }
            if can_delete {
                button {
                    r#type: "button",
                    class: "messages-rail-item-action",
                    title: "Delete section",
                    onclick: move |event: MouseEvent| {
                        event.prevent_default();
                        event.stop_propagation();
                        on_delete.call(());
                    },
                    Icon { name: IconName::Trash, size: 11 }
                }
            }
        }
    }
}

#[component]
fn RailItemMenuPanel(
    anchor: RailItemMenuAnchor,
    favorite: bool,
    item_kind: String,
    item_key: String,
    sections: Vec<MessagesRailSection>,
    #[props(default = false)] show_appearance: bool,
    #[props(default = false)] can_delete: bool,
    #[props(default = false)] show_clear: bool,
    #[props(default = false)] is_contact_pin: bool,
    /// Contact/DM with a real pubkey — offer companion advert-path fetch.
    #[props(default = false)] show_fetch_path: bool,
    on_close: EventHandler<()>,
    on_changed: EventHandler<()>,
    on_appearance: EventHandler<()>,
    on_delete: EventHandler<()>,
    on_clear: EventHandler<()>,
    on_add_section: EventHandler<(String, String)>,
    on_details: EventHandler<()>,
) -> Element {
    let mut move_open = use_signal(|| false);
    let fav_kind = item_kind.clone();
    let fav_key = item_key.clone();
    let move_kind = item_kind.clone();
    let move_key = item_key.clone();
    let add_kind = item_kind.clone();
    let add_key = item_key.clone();
    let fetch_key = item_key.clone();
    // Host on a full-viewport fixed shell so WebKit overflow on the rail cannot
    // clip the panel. Prefer opening into the convo pane; clamp near edges.
    let panel_class = "rail-item-menu rail-item-menu-context";
    let (panel_style, submenu_side_class) = match anchor {
        RailItemMenuAnchor::Context { x, y } => {
            // Keep the primary panel on-screen; room for ~4 rows (~10rem).
            let left = x.min(920.0).max(8.0);
            let top = y.min(720.0).max(8.0);
            let style = format!("left:{left}px;top:{top}px;");
            let mut side = String::from("rail-item-menu-submenu");
            if y > 420.0 {
                side.push_str(" rail-item-menu-submenu-up");
            }
            if x > 720.0 {
                side.push_str(" rail-item-menu-submenu-left");
            }
            (style, side)
        }
    };
    rsx! {
        div { class: "rail-item-menu-shell",
            button {
                r#type: "button",
                class: "rail-item-menu-dismiss",
                "aria-label": "Dismiss menu",
                onclick: move |event: MouseEvent| {
                    event.prevent_default();
                    event.stop_propagation();
                    on_close.call(());
                },
            }
            div {
                class: "{panel_class}",
                style: "{panel_style}",
                role: "menu",
                onclick: move |event: MouseEvent| event.stop_propagation(),
                button {
                    r#type: "button",
                    class: "rail-item-menu-item",
                    role: "menuitem",
                    onclick: move |_| {
                        if is_contact_pin {
                            if favorite {
                                let _ = state().db.unpin_contact(&fav_key);
                                let _ = state().db.set_messages_favorite("dm", &fav_key, false);
                            } else {
                                let _ = state().db.pin_contact(&fav_key, "");
                                let _ = state().db.set_messages_favorite("dm", &fav_key, true);
                            }
                        } else {
                            let _ = state().db.set_messages_favorite(&fav_kind, &fav_key, !favorite);
                        }
                        on_changed.call(());
                        on_close.call(());
                    },
                    Icon { name: IconName::Star, size: 12 }
                    if favorite { "Remove favorite" } else { "Add to favorites" }
                }
            button {
                r#type: "button",
                class: "rail-item-menu-item",
                role: "menuitem",
                title: "Open details for this item",
                onclick: move |_| {
                    on_details.call(());
                    on_close.call(());
                },
                Icon { name: IconName::Help, size: 12 }
                "Details"
            }
            if show_fetch_path {
                button {
                    r#type: "button",
                    class: "rail-item-menu-item",
                    role: "menuitem",
                    title: "Sync companion contact, fetch path, then request status and telemetry (uses airtime)",
                    onclick: move |_| {
                        on_close.call(());
                        if fetch_key.starts_with("name:") {
                            crate::toast::push(
                                crate::toast::ToastKind::Warn,
                                "No public key yet — wait for their advert, or Add their key.",
                            );
                            return;
                        }
                        if state().radio.snapshot().state != "connected" {
                            crate::toast::push(
                                crate::toast::ToastKind::Warn,
                                "Connect a companion before refreshing a contact.",
                            );
                            return;
                        }
                        crate::toast::push(
                            crate::toast::ToastKind::Info,
                            "Refreshing contact (path, status, telemetry)…",
                        );
                        let key = fetch_key.clone();
                        let rx = state().radio.refresh_contact(key);
                        spawn(async move {
                            let result = rx
                                .await
                                .unwrap_or_else(|_| Err("Radio task ended.".into()));
                            match result {
                                Ok(refresh) => crate::toast::push(
                                    crate::toast::ToastKind::Success,
                                    refresh.summary,
                                ),
                                Err(err) => crate::toast::push(
                                    crate::toast::ToastKind::Error,
                                    err,
                                ),
                            }
                        });
                    },
                    Icon { name: IconName::Signal, size: 12 }
                    "Refresh contact"
                }
            }
            div { class: "rail-item-menu-flyout-wrap",
                button {
                    r#type: "button",
                    class: if move_open() {
                        "rail-item-menu-item rail-item-menu-item-active"
                    } else {
                        "rail-item-menu-item"
                    },
                    role: "menuitem",
                    onclick: move |event: MouseEvent| {
                        event.prevent_default();
                        event.stop_propagation();
                        move_open.set(!move_open());
                    },
                    Icon { name: IconName::Menu, size: 12 }
                    "Move to section"
                    span { class: "rail-item-menu-caret",
                        Icon { name: IconName::ChevronRight, size: 11 }
                    }
                }
                if move_open() {
                    div { class: "{submenu_side_class}", role: "menu",
                        button {
                            r#type: "button",
                            class: "rail-item-menu-item",
                            role: "menuitem",
                            onclick: move |_| {
                                let _ = state().db.set_messages_item_section(&move_kind, &move_key, None);
                                on_changed.call(());
                                on_close.call(());
                            },
                            "Default list"
                        }
                        for section in sections.iter() {
                            {
                                let sid = section.id;
                                let name = section.name.clone();
                                let icon = section.icon.clone();
                                let kind = move_kind.clone();
                                let key = move_key.clone();
                                rsx! {
                                    button {
                                        r#type: "button",
                                        class: "rail-item-menu-item",
                                        role: "menuitem",
                                        onclick: move |_| {
                                            let _ = state().db.set_messages_item_section(&kind, &key, Some(sid));
                                            on_changed.call(());
                                            on_close.call(());
                                        },
                                        if !icon.is_empty() {
                                            span { class: "messages-rail-section-emoji", "{icon}" }
                                        }
                                        "{name}"
                                    }
                                }
                            }
                        }
                        button {
                            r#type: "button",
                            class: "rail-item-menu-item rail-item-menu-item-accent",
                            role: "menuitem",
                            onclick: move |_| {
                                on_add_section.call((add_kind.clone(), add_key.clone()));
                                on_close.call(());
                            },
                            Icon { name: IconName::Plus, size: 12 }
                            "Add section…"
                        }
                    }
                }
            }
            if show_appearance {
                button {
                    r#type: "button",
                    class: "rail-item-menu-item",
                    role: "menuitem",
                    onclick: move |_| {
                        on_appearance.call(());
                        on_close.call(());
                    },
                    Icon { name: IconName::Settings, size: 12 }
                    "Appearance"
                }
            }
            if can_delete {
                button {
                    r#type: "button",
                    class: "rail-item-menu-item rail-item-menu-item-danger",
                    role: "menuitem",
                    onclick: move |_| {
                        on_delete.call(());
                        on_close.call(());
                    },
                    Icon { name: IconName::Trash, size: 12 }
                    "Delete"
                }
            }
            if show_clear {
                button {
                    r#type: "button",
                    class: "rail-item-menu-item rail-item-menu-item-danger",
                    role: "menuitem",
                    onclick: move |_| {
                        on_clear.call(());
                        on_close.call(());
                    },
                    Icon { name: IconName::Trash, size: 12 }
                    "Clear conversation"
                }
            }
            }
        }
    }
}

#[component]
fn ChannelRailItem(
    channel: ChannelRow,
    active: bool,
    favorite: bool,
    sections: Vec<MessagesRailSection>,
    details: Signal<Option<RailDetails>>,
    on_select: EventHandler<String>,
    on_appearance: EventHandler<String>,
    on_delete: EventHandler<String>,
    on_changed: EventHandler<()>,
    on_add_section: EventHandler<(String, String)>,
) -> Element {
    let name = channel.name.clone();
    let can_delete = !name.eq_ignore_ascii_case("Public")
        && !state().db.is_reserved_private(&name);
    let unread = channel.unread_count;
    let has_look = channel.has_appearance_override;
    let select_name = name.clone();
    let delete_name = name.clone();
    let appear_name = name.clone();
    let fav_name = name.clone();
    let menu_key = name.clone();
    let details_key = name.clone();
    let mut menu = use_signal(|| None::<RailItemMenuAnchor>);
    rsx! {
        div {
            key: "ch-{name}",
            class: if active {
                "messages-rail-item messages-rail-item-active"
            } else {
                "messages-rail-item"
            },
            oncontextmenu: move |event: MouseEvent| {
                event.prevent_default();
                event.stop_propagation();
                let pt = event.data().client_coordinates();
                menu.set(Some(RailItemMenuAnchor::Context { x: pt.x, y: pt.y }));
            },
            button {
                r#type: "button",
                class: "messages-rail-item-main",
                onclick: move |_| on_select.call(select_name.clone()),
                ConversationAvatar {
                    label: name.clone(),
                    seed: name.clone(),
                }
                span { class: "messages-rail-item-label", "{name}" }
                if has_look {
                    span {
                        class: "text-[0.6rem] uppercase tracking-wide text-muted-foreground",
                        title: "Custom appearance",
                        "Aa"
                    }
                }
                if unread > 0 {
                    span { class: "{unread_badge_class(channel.unread_mentions)}",
                        if unread > 99 { "99+" } else { "{unread}" }
                    }
                }
            }
            button {
                r#type: "button",
                class: if favorite {
                    "messages-rail-item-action messages-rail-item-action-fav-on"
                } else {
                    "messages-rail-item-action"
                },
                title: if favorite { "Remove favorite" } else { "Favorite" },
                onclick: move |event: MouseEvent| {
                    event.prevent_default();
                    event.stop_propagation();
                    let _ = state().db.set_messages_favorite("channel", &fav_name, !favorite);
                    on_changed.call(());
                },
                Icon { name: IconName::Star, size: 12 }
            }
            div { class: "messages-rail-item-settings",
                button {
                    r#type: "button",
                    class: "messages-rail-item-action",
                    title: "Channel settings",
                    onclick: move |event: MouseEvent| {
                        event.prevent_default();
                        event.stop_propagation();
                        if menu().is_some() {
                            menu.set(None);
                        } else {
                            let pt = event.data().client_coordinates();
                            // Open just under the gear, into the convo pane.
                            menu.set(Some(RailItemMenuAnchor::Context {
                                x: pt.x - 12.0,
                                y: pt.y + 8.0,
                            }));
                        }
                    },
                    Icon { name: IconName::Settings, size: 12 }
                }
                if let Some(anchor) = menu() {
                    RailItemMenuPanel {
                        anchor,
                        favorite,
                        item_kind: "channel".to_string(),
                        item_key: menu_key.clone(),
                        sections: sections.clone(),
                        show_appearance: true,
                        can_delete,
                        on_close: move |_| menu.set(None),
                        on_changed: move |_| on_changed.call(()),
                        on_appearance: move |_| on_appearance.call(appear_name.clone()),
                        on_delete: move |_| on_delete.call(delete_name.clone()),
                        on_clear: move |_| {},
                        on_add_section: move |pair| on_add_section.call(pair),
                        on_details: move |_| {
                            details.set(Some(RailDetails::Channel(details_key.clone())));
                        },
                    }
                }
            }
        }
    }
}

#[component]
fn DmRailItem(
    thread: DmThreadRow,
    active: bool,
    favorite: bool,
    sections: Vec<MessagesRailSection>,
    details: Signal<Option<RailDetails>>,
    on_select: EventHandler<String>,
    on_clear: EventHandler<(String, String)>,
    on_changed: EventHandler<()>,
    on_add_section: EventHandler<(String, String)>,
) -> Element {
    let pubkey = thread.pubkey.clone();
    let label = thread.name.clone();
    let unread = thread.unread_count;
    let mention_tone = thread.unread_mentions;
    let select_key = pubkey.clone();
    let clear_key = pubkey.clone();
    let clear_label = label.clone();
    let fav_key = pubkey.clone();
    let menu_key = pubkey.clone();
    let details_key = pubkey.clone();
    let hover_title = rail_item_hover_title(&label, &pubkey, "");
    let mut menu = use_signal(|| None::<RailItemMenuAnchor>);
    rsx! {
        div {
            key: "dm-{pubkey}",
            class: if active {
                "messages-rail-item messages-rail-item-active"
            } else {
                "messages-rail-item"
            },
            oncontextmenu: move |event: MouseEvent| {
                event.prevent_default();
                event.stop_propagation();
                let pt = event.data().client_coordinates();
                menu.set(Some(RailItemMenuAnchor::Context { x: pt.x, y: pt.y }));
            },
            button {
                r#type: "button",
                class: "messages-rail-item-main",
                title: "{hover_title}",
                onclick: move |_| on_select.call(select_key.clone()),
                ConversationAvatar {
                    label: label.clone(),
                    seed: pubkey.clone(),
                }
                span { class: "messages-rail-item-label", "{label}" }
                if unread > 0 {
                    span { class: "{unread_badge_class(mention_tone)}",
                        if unread > 99 { "99+" } else { "{unread}" }
                    }
                }
            }
            button {
                r#type: "button",
                class: if favorite {
                    "messages-rail-item-action messages-rail-item-action-fav-on"
                } else {
                    "messages-rail-item-action"
                },
                title: if favorite { "Remove favorite" } else { "Favorite" },
                onclick: move |event: MouseEvent| {
                    event.prevent_default();
                    event.stop_propagation();
                    let _ = state().db.set_messages_favorite("dm", &fav_key, !favorite);
                    on_changed.call(());
                },
                Icon { name: IconName::Star, size: 12 }
            }
            div { class: "messages-rail-item-settings",
                button {
                    r#type: "button",
                    class: "messages-rail-item-action",
                    title: "Conversation settings",
                    onclick: move |event: MouseEvent| {
                        event.prevent_default();
                        event.stop_propagation();
                        if menu().is_some() {
                            menu.set(None);
                        } else {
                            let pt = event.data().client_coordinates();
                            // Open just under the gear, into the convo pane.
                            menu.set(Some(RailItemMenuAnchor::Context {
                                x: pt.x - 12.0,
                                y: pt.y + 8.0,
                            }));
                        }
                    },
                    Icon { name: IconName::Settings, size: 12 }
                }
                if let Some(anchor) = menu() {
                    RailItemMenuPanel {
                        anchor,
                        favorite,
                        item_kind: "dm".to_string(),
                        item_key: menu_key.clone(),
                        sections: sections.clone(),
                        show_clear: true,
                        show_fetch_path: true,
                        on_close: move |_| menu.set(None),
                        on_changed: move |_| on_changed.call(()),
                        on_appearance: move |_| {},
                        on_delete: move |_| {},
                        on_clear: move |_| on_clear.call((clear_key.clone(), clear_label.clone())),
                        on_add_section: move |pair| on_add_section.call(pair),
                        on_details: move |_| {
                            details.set(Some(RailDetails::Contact(details_key.clone())));
                        },
                    }
                }
            }
        }
    }
}

#[component]
fn ContactNodeItem(
    pubkey: String,
    label: String,
    contact_type: u8,
    active: bool,
    pinned: bool,
    sections: Vec<MessagesRailSection>,
    details: Signal<Option<RailDetails>>,
    on_select: EventHandler<String>,
    on_changed: EventHandler<()>,
    on_add_section: EventHandler<(String, String)>,
) -> Element {
    let select_key = pubkey.clone();
    let pin_key = pubkey.clone();
    let pin_label = label.clone();
    let menu_key = pubkey.clone();
    let details_key = pubkey.clone();
    let mut menu = use_signal(|| None::<RailItemMenuAnchor>);
    let kind_note = match contact_type {
        2 => "Repeater",
        3 => "Room",
        4 => "Sensor",
        _ => "",
    };
    let hover_title = rail_item_hover_title(&label, &pubkey, kind_note);
    rsx! {
        div {
            key: "node-{pubkey}",
            class: if active {
                "messages-rail-item messages-rail-item-active"
            } else {
                "messages-rail-item"
            },
            oncontextmenu: move |event: MouseEvent| {
                event.prevent_default();
                event.stop_propagation();
                let pt = event.data().client_coordinates();
                menu.set(Some(RailItemMenuAnchor::Context { x: pt.x, y: pt.y }));
            },
            button {
                r#type: "button",
                class: "messages-rail-item-main",
                title: "{hover_title}",
                onclick: move |_| on_select.call(select_key.clone()),
                ConversationAvatar {
                    label: label.clone(),
                    seed: pubkey.clone(),
                }
                span { class: "messages-rail-item-label", "{label}" }
                if !kind_note.is_empty() {
                    span { class: "messages-rail-kind", "{kind_note}" }
                }
            }
            button {
                r#type: "button",
                class: if pinned {
                    "messages-rail-item-action messages-rail-item-action-fav-on"
                } else {
                    "messages-rail-item-action"
                },
                title: if pinned {
                    "Remove from Pinned"
                } else {
                    "Favorite — add to Pinned contacts"
                },
                onclick: move |event: MouseEvent| {
                    event.prevent_default();
                    event.stop_propagation();
                    if pinned {
                        let _ = state().db.unpin_contact(&pin_key);
                        let _ = state().db.set_messages_favorite("dm", &pin_key, false);
                    } else {
                        let _ = state().db.pin_contact(&pin_key, &pin_label);
                        let _ = state().db.set_messages_favorite("dm", &pin_key, true);
                    }
                    on_changed.call(());
                },
                Icon { name: IconName::Star, size: 12 }
            }
            div { class: "messages-rail-item-settings",
                button {
                    r#type: "button",
                    class: "messages-rail-item-action",
                    title: "Contact settings",
                    onclick: move |event: MouseEvent| {
                        event.prevent_default();
                        event.stop_propagation();
                        if menu().is_some() {
                            menu.set(None);
                        } else {
                            let pt = event.data().client_coordinates();
                            // Open just under the gear, into the convo pane.
                            menu.set(Some(RailItemMenuAnchor::Context {
                                x: pt.x - 12.0,
                                y: pt.y + 8.0,
                            }));
                        }
                    },
                    Icon { name: IconName::Settings, size: 12 }
                }
                if let Some(anchor) = menu() {
                    RailItemMenuPanel {
                        anchor,
                        favorite: pinned,
                        item_kind: "contact".to_string(),
                        item_key: menu_key.clone(),
                        sections: sections.clone(),
                        is_contact_pin: true,
                        show_fetch_path: true,
                        on_close: move |_| menu.set(None),
                        on_changed: move |_| on_changed.call(()),
                        on_appearance: move |_| {},
                        on_delete: move |_| {},
                        on_clear: move |_| {},
                        on_add_section: move |pair| on_add_section.call(pair),
                        on_details: move |_| {
                            details.set(Some(RailDetails::Contact(details_key.clone())));
                        },
                    }
                }
            }
        }
    }
}

/// Short hover title — never dump a full 64-char pubkey into the native tooltip.
fn rail_item_hover_title(label: &str, pubkey: &str, kind_note: &str) -> String {
    let name = label.trim();
    if pubkey.starts_with("name:") {
        return if name.is_empty() {
            "Awaiting advert".into()
        } else {
            format!("Awaiting advert · {name}")
        };
    }
    let prefix: String = pubkey.chars().take(8).collect();
    let mut parts = Vec::new();
    if !kind_note.is_empty() {
        parts.push(kind_note.to_string());
    }
    if !name.is_empty() {
        parts.push(format!("Open {name}"));
    } else {
        parts.push("Open chat".into());
    }
    if !prefix.is_empty() {
        parts.push(format!("{prefix}…"));
    }
    parts.join(" · ")
}

/// Resolve a full 64-char pubkey from an optional key/prefix and/or display name.
///
/// Channel frames often have only a display name. When no hex key exists yet,
/// returns a `name:…` bookmark key (same shape as channel name blocks) so the
/// operator can still Favorite them; the pin promotes when an advert arrives.
pub fn resolve_contact_pubkey(
    known: Option<&str>,
    display_name: &str,
) -> Result<(String, String), String> {
    let label = display_name.trim();
    if let Some(raw) = known.map(str::trim).filter(|s| !s.is_empty()) {
        let key = raw.to_ascii_lowercase();
        if key.len() == 64 && key.chars().all(|c| c.is_ascii_hexdigit()) {
            let nice = state()
                .db
                .get_node(&key)
                .ok()
                .flatten()
                .map(|n| node_label(&n))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| {
                    if label.is_empty() {
                        key.chars().take(12).collect()
                    } else {
                        label.to_string()
                    }
                });
            return Ok((key, nice));
        }
        // Prefix / partial key → unique roster match.
        let matches: Vec<_> = state()
            .db
            .list_nodes()
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter(|n| {
                n.pubkey.to_ascii_lowercase().starts_with(&key)
                    || n.prefix.to_ascii_lowercase() == key
            })
            .collect();
        if matches.len() == 1 {
            let n = &matches[0];
            return Ok((n.pubkey.clone(), node_label(n)));
        }
    }
    if !label.is_empty() {
        if let Some(node) = state()
            .db
            .unique_node_by_display_name(label)
            .map_err(|e| e.to_string())?
        {
            return Ok((node.pubkey.clone(), node_label(&node)));
        }
        // Fall back to a unique DM thread title (full pubkey already known there).
        let dm_hits: Vec<_> = state()
            .db
            .list_dm_threads()
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter(|t| t.name.eq_ignore_ascii_case(label))
            .collect();
        if dm_hits.len() == 1 {
            let t = &dm_hits[0];
            let key = t.pubkey.trim().to_ascii_lowercase();
            if key.len() == 64 && key.chars().all(|c| c.is_ascii_hexdigit()) {
                let nice = if t.name.trim().is_empty() {
                    key.chars().take(12).collect()
                } else {
                    t.name.clone()
                };
                return Ok((key, nice));
            }
        }
        // Any prior channel/DM row that already stored a pubkey for this label.
        let from_msgs = state()
            .db
            .pubkeys_for_sender_label(label)
            .map_err(|e| e.to_string())?;
        if from_msgs.len() == 1 {
            let key = from_msgs[0].clone();
            let nice = state()
                .db
                .get_node(&key)
                .ok()
                .flatten()
                .map(|n| node_label(&n))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| label.to_string());
            return Ok((key, nice));
        }
        // Channel-only sender: bookmark by stable name key until an advert lands.
        if let Some(nk) = crate::db::name_block_key(label) {
            return Ok((nk, label.to_string()));
        }
    }
    Err(format!(
        "Could not resolve “{label}”. Channel messages need a roster advert before HQ has a public key."
    ))
}

fn is_hex_pubkey(key: &str) -> bool {
    key.len() == 64 && key.chars().all(|c| c.is_ascii_hexdigit())
}

pub fn pin_contact_from_message(pubkey: &str, label: &str) -> Result<bool, String> {
    let (key, nice) = resolve_contact_pubkey(Some(pubkey), label)?;
    state()
        .db
        .pin_contact(&key, &nice)
        .map_err(|e| e.to_string())?;
    let name_only = !is_hex_pubkey(&key);
    if !name_only {
        let _ = state().db.set_messages_favorite("dm", &key, true);
    }
    let _ = unix_now();
    Ok(name_only)
}

/// Toggle Pinned Contacts (+ DM favorite flag) for a sender.
/// Returns `(now_pinned, name_only)` — `name_only` when the sender has no pubkey yet.
pub fn toggle_contact_favorite(pubkey: &str, label: &str) -> Result<(bool, bool), String> {
    let (key, nice) = resolve_contact_pubkey(Some(pubkey), label)?;
    let name_only = !is_hex_pubkey(&key);
    let pinned = state()
        .db
        .list_pinned_contacts()
        .map_err(|e| e.to_string())?
        .iter()
        .any(|row| row.pubkey.eq_ignore_ascii_case(&key));
    if pinned {
        state()
            .db
            .unpin_contact(&key)
            .map_err(|e| e.to_string())?;
        let _ = state().db.set_messages_favorite("dm", &key, false);
        Ok((false, name_only))
    } else {
        state()
            .db
            .pin_contact(&key, &nice)
            .map_err(|e| e.to_string())?;
        // Only star the DMs list when we have a real pubkey (name bookmarks stay in Contacts → Pinned).
        if !name_only {
            let _ = state().db.set_messages_favorite("dm", &key, true);
        }
        Ok((true, name_only))
    }
}

/// Same as [`toggle_contact_favorite`], but accepts optional known key + display name.
pub fn toggle_contact_favorite_from_bubble(
    known: Option<&str>,
    display_name: &str,
) -> Result<(bool, bool), String> {
    let (key, nice) = resolve_contact_pubkey(known, display_name)?;
    toggle_contact_favorite(&key, &nice)
}


